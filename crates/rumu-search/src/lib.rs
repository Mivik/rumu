//! rumu-search: the full-text index of session history.
//!
//! One tantivy document per node: its session, id, time, kind, the text a
//! search matches (see [`searchable`]) and a score boost (tool results count
//! half). Text is segmented by charabia (only its Chinese segmenter is
//! built in; other scripts split on word boundaries) and lowercased.
//!
//! The index is derived data. Its commit payload records, per session, the
//! journal length it covers; a session whose journal differs (a crash before
//! a commit, an index from another build) is re-indexed from the journal,
//! and an index of another [`VERSION`] or one that does not open is rebuilt
//! from scratch. Writes go through a channel to one indexing thread, which
//! commits once writes pause for [`COMMIT_AFTER`] (at most [`COMMIT_EVERY`]
//! after the first), so the store's appends never wait on the index; a
//! search commits whatever is pending first.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use charabia::Tokenize;
use rumu_core::{HistoryHit, HistoryHits, Node, NodeBody, SessionId};
use serde::{Deserialize, Serialize};
use tantivy::collector::{Count, DocSetCollector, TopDocs};
use tantivy::query::{BooleanQuery, ConstScoreQuery, Occur, Query, TermQuery, TermSetQuery};
use tantivy::schema::{
    FAST, Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use tracing::warn;

/// Bumped whenever what is indexed or how it is tokenized changes: an index
/// of another version is rebuilt.
pub const VERSION: u32 = 1;
/// Commit once writes have paused this long ...
pub const COMMIT_AFTER: Duration = Duration::from_secs(2);
/// ... or this long after the first uncommitted one.
pub const COMMIT_EVERY: Duration = Duration::from_secs(30);
/// The tool results' score multiplier: a hit in what the model said or was
/// told outranks one in what a tool printed.
const TOOL_RESULT_BOOST: f64 = 0.5;
/// The indexing thread's memory budget (tantivy's minimum is 15 MB).
const WRITER_BUDGET: usize = 20 << 20;
const TOKENIZER: &str = "charabia";

pub type Error = tantivy::TantivyError;
pub type Result<T> = std::result::Result<T, Error>;

/// Tools whose call args are not indexed: they only look things up, so their
/// args just echo earlier queries and paths back as noise hits.
const LOOKUP_TOOLS: [&str; 3] = ["read", "list", "search"];

/// The text a search matches against, with the node's kind label: user
/// text, assistant text + tool-call args (bar [`LOOKUP_TOOLS`]), tool-result
/// text, note text — images as their `[image: …]` placeholder. Markers
/// (empty-text notes) carry nothing searchable.
pub fn searchable(body: &NodeBody) -> Option<(&'static str, String)> {
    match body {
        NodeBody::User { content } => Some(("user", content.rendered())),
        NodeBody::Assistant {
            content,
            tool_calls,
            ..
        } => {
            let mut text = content.rendered();
            for call in tool_calls {
                if LOOKUP_TOOLS.contains(&call.name.as_str()) {
                    continue;
                }
                text.push('\n');
                text.push_str(&call.name);
                text.push(' ');
                text.push_str(&serde_json::to_string(&call.args).unwrap_or_default());
            }
            Some(("assistant", text))
        }
        NodeBody::ToolResult { content, .. } => Some(("tool_result", content.rendered())),
        NodeBody::Note { content, .. } if !content.text.is_empty() => {
            Some(("note", content.rendered()))
        }
        NodeBody::Note { .. } => None,
    }
}

/// The words of `text` as indexed: charabia's word tokens, lowercased, with
/// their byte range in `text`.
fn words(text: &str) -> Vec<(String, usize, usize)> {
    text.tokenize()
        .filter(|t| t.is_word())
        .map(|t| (t.lemma().to_lowercase(), t.byte_start, t.byte_end))
        .collect()
}

/// The pieces of `query` a search matches, as written but lowercased: for
/// pointing at a match in a hit's text.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    words(query)
        .into_iter()
        .map(|(_, start, end)| query[start..end].to_lowercase())
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

#[derive(Clone)]
struct Charabia;

impl Tokenizer for Charabia {
    type TokenStream<'a> = Tokens;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Tokens {
        let tokens = words(text)
            .into_iter()
            .enumerate()
            .map(|(position, (text, offset_from, offset_to))| Token {
                offset_from,
                offset_to,
                position,
                text,
                position_length: 1,
            })
            .collect();
        Tokens { tokens, at: None }
    }
}

struct Tokens {
    tokens: Vec<Token>,
    at: Option<usize>,
}

impl TokenStream for Tokens {
    fn advance(&mut self) -> bool {
        let next = self.at.map_or(0, |i| i + 1);
        self.at = Some(next);
        next < self.tokens.len()
    }

    fn token(&self) -> &Token {
        &self.tokens[self.at.unwrap_or(0)]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.at.unwrap_or(0)]
    }
}

#[derive(Clone, Copy)]
struct Fields {
    /// `<session>/<node>`: what a node's document is replaced by.
    key: Field,
    session: Field,
    node: Field,
    at: Field,
    kind: Field,
    boost: Field,
    text: Field,
}

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let text = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqs),
        )
        .set_stored();
    let fields = Fields {
        key: b.add_text_field("key", STRING),
        session: b.add_text_field("session", STRING | STORED),
        node: b.add_text_field("node", STRING | STORED | FAST),
        at: b.add_i64_field("at", STORED | FAST),
        kind: b.add_text_field("kind", STORED),
        boost: b.add_f64_field("boost", FAST),
        text: b.add_text_field("text", text),
    };
    (b.build(), fields)
}

/// The commit payload.
#[derive(Default, Serialize, Deserialize)]
struct Payload {
    version: u32,
    /// Session id → the journal length its nodes are indexed up to.
    sessions: BTreeMap<String, u64>,
}

/// One search hit.
pub type Hit = HistoryHit;
/// What a search found: the best hits, and how many match in all.
pub type Found = HistoryHits;

enum Op {
    Upsert(Box<Node>, u64),
    Touch(String, u64),
    Delete(String),
    /// These sessions are being re-indexed: until [`Op::Reindexed`], their
    /// recorded lengths stay out of the payload.
    Stale(Vec<String>),
    Reindexed(String, u64),
    Flush(Sender<()>),
}

struct Shared {
    fields: Fields,
    reader: IndexReader,
    writer: RwLock<IndexWriter>,
}

impl Shared {
    fn upsert(&self, node: &Node) {
        let f = self.fields;
        let key = format!("{}/{}", node.session, node.id);
        let mut doc = TantivyDocument::new();
        doc.add_text(f.key, &key);
        doc.add_text(f.session, &node.session.0);
        doc.add_text(f.node, &node.id.0);
        doc.add_i64(f.at, node.at);
        let (kind, text) = searchable(&node.body).unwrap_or(("marker", String::new()));
        doc.add_text(f.kind, kind);
        doc.add_f64(
            f.boost,
            if kind == "tool_result" {
                TOOL_RESULT_BOOST
            } else {
                1.0
            },
        );
        doc.add_text(f.text, text);
        let writer = self.writer.read().unwrap();
        // A node indexed twice (live and by a re-index) keeps one document.
        writer.delete_term(Term::from_field_text(f.key, &key));
        if let Err(e) = writer.add_document(doc) {
            warn!(error = %e, "indexing a node failed");
        }
    }
}

/// The session history index (see the crate docs).
pub struct SessionIndex {
    shared: Arc<Shared>,
    /// `None` once dropping: the worker then commits and stops.
    tx: Option<Sender<Op>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SessionIndex {
    /// Wait for the last commit, so the directory can be opened again.
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl SessionIndex {
    /// Open the index in `dir`, or start a new one there when there is none,
    /// it is of another [`VERSION`] or it does not open. Also returns the
    /// journal length each session is indexed up to.
    pub fn open(dir: &Path) -> Result<(Self, BTreeMap<String, u64>)> {
        let (schema, fields) = schema();
        let opened = Index::open_in_dir(dir).and_then(|index| {
            let payload = index.load_metas()?.payload;
            let payload = payload
                .and_then(|p| serde_json::from_str::<Payload>(&p).ok())
                .unwrap_or_default();
            if payload.version != VERSION || index.schema() != schema {
                return Err(Error::SchemaError(format!(
                    "index version {} (this build: {VERSION})",
                    payload.version
                )));
            }
            Ok((index, payload.sessions))
        });
        let (index, sessions) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                if dir.exists() {
                    warn!(dir = %dir.display(), error = %e, "rebuilding the session index");
                    std::fs::remove_dir_all(dir)?;
                }
                std::fs::create_dir_all(dir)?;
                (Index::create_in_dir(dir, schema)?, BTreeMap::new())
            }
        };
        Ok((Self::start(index, fields, sessions.clone())?, sessions))
    }

    /// An index in memory (for in-memory stores and tests).
    pub fn in_memory() -> Self {
        let (schema, fields) = schema();
        Self::start(Index::create_in_ram(schema), fields, BTreeMap::new())
            .expect("an index in memory starts")
    }

    fn start(index: Index, fields: Fields, sessions: BTreeMap<String, u64>) -> Result<Self> {
        index.tokenizers().register(TOKENIZER, Charabia);
        let writer = index.writer_with_num_threads(1, WRITER_BUDGET)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let shared = Arc::new(Shared {
            fields,
            reader,
            writer: RwLock::new(writer),
        });
        let (tx, rx) = mpsc::channel();
        let worker = Worker {
            shared: shared.clone(),
            sessions,
            stale: HashMap::new(),
            dirty: None,
        };
        let worker = std::thread::Builder::new()
            .name("session-index".into())
            .spawn(move || worker.run(rx))?;
        Ok(Self {
            shared,
            tx: Some(tx),
            worker: Some(worker),
        })
    }

    fn send(&self, op: Op) {
        // The worker only stops when this sender is dropped.
        if let Some(tx) = &self.tx {
            let _ = tx.send(op);
        }
    }

    /// Index (or re-index) `node`, whose journal is now `len` bytes long.
    pub fn upsert(&self, node: &Node, len: u64) {
        self.send(Op::Upsert(Box::new(node.clone()), len));
    }

    /// `session`'s journal grew to `len` bytes by a record that is no node.
    pub fn touch(&self, session: &SessionId, len: u64) {
        self.send(Op::Touch(session.0.clone(), len));
    }

    /// Drop every node of `session`.
    pub fn delete_session(&self, session: &SessionId) {
        self.send(Op::Delete(session.0.clone()));
    }

    /// `sessions` are about to be re-indexed from their journals: index
    /// each node with [`reindex_node`](Self::reindex_node), then call
    /// [`reindexed`](Self::reindexed).
    pub fn stale(&self, sessions: Vec<String>) {
        self.send(Op::Stale(sessions));
    }

    /// Index a node read back from a journal being re-indexed. Blocks while
    /// the index commits, so it is for the re-indexing thread, not the store.
    pub fn reindex_node(&self, node: &Node) {
        self.shared.upsert(node);
    }

    /// `session` is re-indexed from its first `len` journal bytes.
    pub fn reindexed(&self, session: &SessionId, len: u64) {
        self.send(Op::Reindexed(session.0.clone(), len));
    }

    /// Commit what is pending and wait for it.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        self.send(Op::Flush(tx));
        let _ = rx.recv();
    }

    /// The nodes of `sessions` that match `query` (any word), best first by
    /// BM25 (tool results count half; ties newest first), at most `limit`.
    pub fn search(&self, query: &str, sessions: &[SessionId], limit: usize) -> Result<Found> {
        self.flush();
        let f = self.shared.fields;
        let written = query_terms(query);
        let terms: BTreeSet<String> = words(query).into_iter().map(|(t, ..)| t).collect();
        if terms.is_empty() || sessions.is_empty() || limit == 0 {
            return Ok(Found::default());
        }
        let any: Vec<(Occur, Box<dyn Query>)> = terms
            .iter()
            .map(|t| {
                let term = Term::from_field_text(f.text, t);
                let q: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs));
                (Occur::Should, q)
            })
            .collect();
        let within = TermSetQuery::new(
            sessions
                .iter()
                .map(|s| Term::from_field_text(f.session, &s.0)),
        );
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(BooleanQuery::new(any)) as Box<dyn Query>,
            ),
            (
                Occur::Must,
                Box::new(ConstScoreQuery::new(Box::new(within), 0.0)),
            ),
        ]);
        let top =
            TopDocs::with_limit(limit).tweak_score(move |segment: &tantivy::SegmentReader| {
                let fast = segment.fast_fields();
                let boost = fast.f64("boost").ok();
                let at = fast.i64("at").ok();
                move |doc, score| {
                    let boost = boost.as_ref().and_then(|c| c.first(doc)).unwrap_or(1.0);
                    let at = at.as_ref().and_then(|c| c.first(doc)).unwrap_or(0);
                    (f64::from(score) * boost, at)
                }
            });
        let searcher = self.shared.reader.searcher();
        let (top, total) = searcher.search(&query, &(top, Count))?;
        let mut hits = Vec::with_capacity(top.len());
        for ((score, _), address) in top {
            let doc: TantivyDocument = searcher.doc(address)?;
            let text = |field| {
                doc.get_first(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            hits.push(Hit {
                session: SessionId(text(f.session)),
                node: text(f.node),
                at: doc.get_first(f.at).and_then(|v| v.as_i64()).unwrap_or(0),
                kind: text(f.kind),
                text: text(f.text),
                score,
            });
        }
        Ok(Found {
            hits,
            total,
            terms: written,
        })
    }

    /// Every node id of `session` in the index (committed ones).
    pub fn node_ids(&self, session: &SessionId) -> Result<Vec<String>> {
        let f = self.shared.fields;
        let searcher = self.shared.reader.searcher();
        let query = TermQuery::new(
            Term::from_field_text(f.session, &session.0),
            IndexRecordOption::Basic,
        );
        let docs = searcher.search(&query, &DocSetCollector)?;
        let mut columns = HashMap::new();
        let mut out = Vec::with_capacity(docs.len());
        for address in docs {
            let column = match columns.entry(address.segment_ord) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(
                    searcher
                        .segment_reader(address.segment_ord)
                        .fast_fields()
                        .str("node")?,
                ),
            };
            let Some(column) = column else { continue };
            let mut id = String::new();
            if let Some(ord) = column.term_ords(address.doc_id).next()
                && column.ord_to_str(ord, &mut id)?
            {
                out.push(id);
            }
        }
        Ok(out)
    }
}

/// The indexing thread's state.
struct Worker {
    shared: Arc<Shared>,
    /// Session → the journal length its nodes are indexed up to.
    sessions: BTreeMap<String, u64>,
    /// Sessions being re-indexed → the longest length seen meanwhile.
    stale: HashMap<String, u64>,
    /// When the first uncommitted write came.
    dirty: Option<Instant>,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Op>) {
        loop {
            let wait = match self.dirty {
                Some(since) => COMMIT_AFTER.min(COMMIT_EVERY.saturating_sub(since.elapsed())),
                None => Duration::from_secs(3600),
            };
            match rx.recv_timeout(wait) {
                Ok(op) => self.apply(op),
                Err(RecvTimeoutError::Timeout) => self.commit(),
                Err(RecvTimeoutError::Disconnected) => {
                    self.commit();
                    return;
                }
            }
            if self
                .dirty
                .is_some_and(|since| since.elapsed() >= COMMIT_EVERY)
            {
                self.commit();
            }
        }
    }

    fn apply(&mut self, op: Op) {
        match op {
            Op::Upsert(node, len) => {
                self.shared.upsert(&node);
                self.grew(&node.session.0, len);
            }
            Op::Touch(session, len) => self.grew(&session, len),
            Op::Delete(session) => {
                let term = Term::from_field_text(self.shared.fields.session, &session);
                self.shared.writer.read().unwrap().delete_term(term);
                self.sessions.remove(&session);
                self.stale.remove(&session);
            }
            Op::Stale(sessions) => {
                for s in sessions {
                    self.sessions.remove(&s);
                    self.stale.insert(s, 0);
                }
            }
            Op::Reindexed(session, len) => {
                if let Some(seen) = self.stale.remove(&session) {
                    self.sessions.insert(session, len.max(seen));
                }
            }
            Op::Flush(done) => {
                self.commit();
                let _ = done.send(());
                return;
            }
        }
        self.dirty.get_or_insert_with(Instant::now);
    }

    fn grew(&mut self, session: &str, len: u64) {
        let at = match self.stale.get_mut(session) {
            Some(seen) => seen,
            None => self.sessions.entry(session.to_owned()).or_default(),
        };
        *at = (*at).max(len);
    }

    fn commit(&mut self) {
        if self.dirty.take().is_none() {
            return;
        }
        let payload = serde_json::to_string(&Payload {
            version: VERSION,
            sessions: self.sessions.clone(),
        })
        .expect("the payload serializes");
        let mut writer = self.shared.writer.write().unwrap();
        let committed = writer.prepare_commit().and_then(|mut prepared| {
            prepared.set_payload(&payload);
            prepared.commit()
        });
        drop(writer);
        match committed.and_then(|_| self.shared.reader.reload()) {
            Ok(()) => {}
            Err(e) => warn!(error = %e, "committing the session index failed"),
        }
    }
}

#[cfg(test)]
mod tests;
