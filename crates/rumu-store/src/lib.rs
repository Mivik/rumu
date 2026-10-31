//! rumu-store: per-session JSONL persistence for the rumu kernel.
//!
//! Every session is one append-only `<session-id>.jsonl` file inside the
//! directory given to [`Store::open`] (the server passes `~/.rumu/sessions`).
//! Each line is one tagged JSON record, event-sourced style:
//!
//! ```text
//! {"t":"session","id":"7c9e…","title":"","active_branch":"7c9e…","created_at":1,"updated_at":1}
//! {"t":"branch","id":"7c9e…","session":"7c9e…","head":"7c9e…","forked_from":null,"kind":{"type":"main"},"status":"active","created_at":1}
//! {"t":"node","id":"7c9e…","parent":null,"session":"7c9e…","at":1,"body":{"type":"note","source":"system","content":{"text":"","data":{"tag":"root","v":1,"data":{}}}}}
//! {"t":"branch_update","id":"7c9e…","head":"7c9e…","status":"done"}
//! {"t":"meta","title":"refactor","updated_at":2}
//! {"t":"code_call","branch":"7c9e…","parent_call":"c1","seq":1,"name":"read",…}
//! ```
//!
//! Loading a file folds the records into memory: `session` is the header
//! (the initial [`SessionInfo`]), `meta` patches it (title /
//! active_branch / updated_at — whatever the mutating call changed),
//! `branch` adds a branch and `branch_update` moves its head/status, `node`
//! appends a node, and a `code_call` with an already-seen `(parent_call,
//! seq)` replaces the earlier one. Optional fields are `#[serde(default)]`
//! and skipped when unset, so the format can grow without migrations.
//!
//! [`Store::open`] scans the directory once and folds every file up front —
//! the simple robust option at this scale (a session is a few hundred lines).
//! The cache is authoritative afterwards (single process): reads never touch
//! the disk again, and each mutation appends exactly one line with a single
//! `write_all` (the file is reopened in append mode per write; no fsync — a
//! crash loses at most the tail). Id-only lookups (`get_node`, `get_branch`)
//! route through node→session / branch→session indexes built during the scan
//! and maintained on inserts.
//!
//! A crash mid-append can tear the last line; on load it is dropped with a
//! warning and truncated so the next append starts on a clean line. Any other
//! malformed line — including a record rumu cannot deserialize, e.g. a node
//! body written by a newer build — marks just that session unreadable: every
//! read of it is a clear error naming the serde problem, while the other
//! sessions keep loading and listing.

mod error;
pub use error::{Error, Result};

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rumu_core::{
    Branch, BranchId, BranchStatus, CodeCall, IoError, Node, NodeBody, NodeId, SessionId,
    SessionInfo, Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

pub struct Store {
    /// Directory of `<session-id>.jsonl` files; `None` for in-memory stores.
    root: Option<PathBuf>,
    state: Mutex<StoreState>,
}

/// One JSONL line. Internally tagged by `"t"` so lines stay flat and readable.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Record {
    Session(SessionInfo),
    /// Partial session update: only the fields a mutating call changed.
    Meta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_branch: Option<BranchId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_at: Option<Timestamp>,
    },
    Branch(Branch),
    BranchUpdate {
        id: BranchId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<BranchStatus>,
    },
    // Boxed: `Node` dwarfs the other variants and every record is built once
    // per mutation.
    Node(Box<Node>),
    CodeCall(CodeCall),
}

#[derive(Default)]
struct StoreState {
    sessions: HashMap<SessionId, Entry>,
    /// Id-only lookups (`get_node` / `get_branch`) route through these; they
    /// are built during the open scan and kept current on inserts.
    node_owner: HashMap<NodeId, SessionId>,
    branch_owner: HashMap<BranchId, SessionId>,
}

enum Entry {
    Loaded(Box<SessionState>),
    /// The file failed to fold; every read of this session is this error.
    Corrupt(String),
}

/// Folded contents of one session file.
struct SessionState {
    info: SessionInfo,
    branches: HashMap<BranchId, Branch>,
    nodes: HashMap<NodeId, Node>,
    /// Keyed by (parent_call, seq): a later record with the same key
    /// replaces the earlier one.
    code_calls: HashMap<(String, u32), CodeCall>,
}

impl Store {
    /// Open (creating if needed) the directory of per-session `.jsonl` files.
    /// Every file is scanned and folded into memory once, up front.
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root).map_err(IoError::at("creating sessions dir", root))?;
        let mut paths: Vec<PathBuf> = std::fs::read_dir(root)
            .map_err(IoError::at("reading sessions dir", root))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        paths.sort(); // deterministic load order
        let mut state = StoreState::default();
        for path in &paths {
            state.load_file(path)?;
        }
        Ok(Self {
            root: Some(root.to_path_buf()),
            state: Mutex::new(state),
        })
    }

    /// A store with no files behind it (used by tests).
    pub fn in_memory() -> Self {
        Self {
            root: None,
            state: Mutex::new(StoreState::default()),
        }
    }

    /// Append one record as a single line (one buffered `write_all`).
    /// In-memory stores have no files.
    fn append(&self, session: &SessionId, rec: &Record) -> Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let mut line = serde_json::to_string(rec).map_err(|source| Error::Serialize {
            session: session.clone(),
            source,
        })?;
        line.push('\n');
        let path = root.join(format!("{session}.jsonl"));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(IoError::at("opening", &path))?;
        file.write_all(line.as_bytes())
            .map_err(IoError::at("appending to", &path))?;
        Ok(())
    }

    // ---- sessions ----

    pub fn insert_session(&self, s: &SessionInfo) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.sessions.contains_key(&s.id) {
            return Err(Error::SessionExists { id: s.id.clone() });
        }
        let rec = Record::Session(s.clone());
        self.append(&s.id, &rec)?;
        st.sessions.insert(
            s.id.clone(),
            Entry::Loaded(Box::new(SessionState::new(s.clone()))),
        );
        Ok(())
    }

    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionInfo>> {
        let st = self.state.lock().unwrap();
        Ok(match st.sessions.get(id) {
            Some(Entry::Loaded(s)) => Some(s.info.clone()),
            Some(Entry::Corrupt(msg)) => return Err(unreadable(id, msg)),
            None => None,
        })
    }

    /// All sessions, most recently updated first. Unreadable sessions get a
    /// stderr warning at open and stay out of the list.
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let st = self.state.lock().unwrap();
        let mut infos: Vec<SessionInfo> = st
            .sessions
            .values()
            .filter_map(|e| match e {
                Entry::Loaded(s) => Some(s.info.clone()),
                Entry::Corrupt(_) => None,
            })
            .collect();
        infos.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(infos)
    }

    pub fn update_session(&self, s: &SessionInfo) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.loaded(&s.id)?.is_none() {
            return Ok(()); // a no-op on unknown sessions
        }
        let rec = Record::Meta {
            title: Some(s.title.clone()),
            active_branch: Some(s.active_branch.clone()),
            updated_at: Some(s.updated_at),
        };
        self.append(&s.id, &rec)?;
        st.apply(&s.id, rec);
        Ok(())
    }

    /// Bump `updated_at` (used on every node append).
    pub fn touch_session(&self, id: &SessionId, at: Timestamp) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.loaded(id)?.is_none() {
            return Ok(());
        }
        let rec = Record::Meta {
            title: None,
            active_branch: None,
            updated_at: Some(at),
        };
        self.append(id, &rec)?;
        st.apply(id, rec);
        Ok(())
    }

    // ---- branches ----

    pub fn insert_branch(&self, b: &Branch) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.loaded(&b.session)?;
        let rec = Record::Branch(b.clone());
        self.append(&b.session, &rec)?;
        st.apply(&b.session, rec);
        Ok(())
    }

    pub fn get_branch(&self, id: &BranchId) -> Result<Option<Branch>> {
        let st = self.state.lock().unwrap();
        let Some(sid) = st.branch_owner.get(id) else {
            return Ok(None);
        };
        Ok(match st.sessions.get(sid) {
            Some(Entry::Loaded(s)) => s.branches.get(id).cloned(),
            Some(Entry::Corrupt(msg)) => return Err(unreadable(sid, msg)),
            None => None,
        })
    }

    pub fn update_branch(&self, b: &Branch) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let Some(sid) = st.branch_owner.get(&b.id).cloned() else {
            return Ok(()); // a no-op on unknown branches
        };
        if st.loaded(&sid)?.is_none() {
            return Ok(());
        }
        let rec = Record::BranchUpdate {
            id: b.id.clone(),
            head: Some(b.head.clone()),
            status: Some(b.status),
        };
        self.append(&sid, &rec)?;
        st.apply(&sid, rec);
        Ok(())
    }

    pub fn branches_of_session(&self, session: &SessionId) -> Result<Vec<Branch>> {
        let st = self.state.lock().unwrap();
        Ok(match st.sessions.get(session) {
            Some(Entry::Loaded(s)) => {
                let mut v: Vec<Branch> = s.branches.values().cloned().collect();
                v.sort_by(|a, b| {
                    a.created_at
                        .cmp(&b.created_at)
                        .then_with(|| a.id.cmp(&b.id))
                });
                v
            }
            Some(Entry::Corrupt(msg)) => return Err(unreadable(session, msg)),
            None => vec![],
        })
    }

    // ---- nodes ----

    pub fn insert_node(&self, n: &Node) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.loaded(&n.session)?;
        let rec = Record::Node(Box::new(n.clone()));
        self.append(&n.session, &rec)?;
        st.apply(&n.session, rec);
        Ok(())
    }

    pub fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let st = self.state.lock().unwrap();
        let Some(sid) = st.node_owner.get(id) else {
            return Ok(None);
        };
        Ok(match st.sessions.get(sid) {
            Some(Entry::Loaded(s)) => s.nodes.get(id).cloned(),
            Some(Entry::Corrupt(msg)) => return Err(unreadable(sid, msg)),
            None => None,
        })
    }

    /// All nodes of a session, oldest first.
    pub fn nodes_of_session(&self, session: &SessionId) -> Result<Vec<Node>> {
        let st = self.state.lock().unwrap();
        Ok(match st.sessions.get(session) {
            Some(Entry::Loaded(s)) => {
                let mut v: Vec<Node> = s.nodes.values().cloned().collect();
                v.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
                v
            }
            Some(Entry::Corrupt(msg)) => return Err(unreadable(session, msg)),
            None => vec![],
        })
    }

    // ---- internal `code` calls (trace only) ----

    /// Record one tool call made from inside a `code` script. `output` is
    /// already capped by the caller (20k chars). A later call with the same
    /// `(parent_call, seq)` replaces this one.
    pub fn insert_code_call(&self, session: &SessionId, c: &CodeCall) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.loaded(session)?;
        let rec = Record::CodeCall(c.clone());
        self.append(session, &rec)?;
        st.apply(session, rec);
        Ok(())
    }

    /// Every stored internal call of a session, oldest first.
    pub fn code_calls_of_session(&self, session: &SessionId) -> Result<Vec<CodeCall>> {
        let st = self.state.lock().unwrap();
        Ok(match st.sessions.get(session) {
            Some(Entry::Loaded(s)) => {
                let mut v: Vec<CodeCall> = s.code_calls.values().cloned().collect();
                v.sort_by(|a, b| {
                    a.at.cmp(&b.at)
                        .then_with(|| a.parent_call.cmp(&b.parent_call))
                        .then_with(|| a.seq.cmp(&b.seq))
                });
                v
            }
            Some(Entry::Corrupt(msg)) => return Err(unreadable(session, msg)),
            None => vec![],
        })
    }

    /// Every blob hash referenced by a node of any session — every `Content`
    /// of every body variant, on every branch, referenced or not. This is the
    /// live set the blob store's GC keeps. An unreadable session is an error
    /// (its references are unknowable, so GC must not run past it).
    pub fn referenced_blobs(&self) -> Result<HashSet<String>> {
        let st = self.state.lock().unwrap();
        let mut hashes = HashSet::new();
        for (sid, entry) in &st.sessions {
            match entry {
                Entry::Loaded(s) => {
                    for node in s.nodes.values() {
                        let content = match &node.body {
                            NodeBody::User { content }
                            | NodeBody::Assistant { content, .. }
                            | NodeBody::ToolResult { content, .. }
                            | NodeBody::Note { content, .. } => content,
                        };
                        hashes.extend(content.attachments.iter().map(|a| a.hash.clone()));
                    }
                }
                Entry::Corrupt(msg) => return Err(unreadable(sid, msg)),
            }
        }
        Ok(hashes)
    }
}

impl StoreState {
    /// The folded state of `sid`, if it loaded cleanly. `Ok(None)` when no
    /// such session exists.
    fn loaded<'a>(&'a mut self, sid: &SessionId) -> Result<Option<&'a mut SessionState>> {
        match self.sessions.get_mut(sid) {
            Some(Entry::Loaded(s)) => Ok(Some(s)),
            Some(Entry::Corrupt(msg)) => Err(unreadable(sid, msg)),
            None => Ok(None),
        }
    }

    /// Fold one record into `sid`'s state, refreshing the id→session indexes.
    fn apply(&mut self, sid: &SessionId, rec: Record) {
        match &rec {
            Record::Branch(b) => {
                self.branch_owner.insert(b.id.clone(), sid.clone());
            }
            Record::Node(n) => {
                self.node_owner.insert(n.id.clone(), sid.clone());
            }
            _ => {}
        }
        if let Some(Entry::Loaded(s)) = self.sessions.get_mut(sid) {
            s.apply(rec);
        }
    }

    /// Scan one session file and fold it into memory. A torn last line (the
    /// unterminated tail of a crash mid-append) is dropped and truncated;
    /// anything else unreadable marks the session corrupt — with a warning,
    /// since it then disappears from listings — without failing the whole
    /// open.
    fn load_file(&mut self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path).map_err(IoError::at("reading", path))?;
        if content.is_empty() {
            return Ok(()); // e.g. a crash before the first record landed
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        // A crash mid-append leaves a final line without its newline.
        let mut lines: Vec<&str> = content.split('\n').collect();
        let tail = if content.ends_with('\n') {
            lines.pop(); // the empty artifact after the final newline
            None
        } else {
            lines.pop() // the unterminated last line
        };

        let mut sid: Option<SessionId> = None;
        let mut state: Option<SessionState> = None;
        let mut corrupt: Option<String> = None;
        // Owners are collected even from a file that ends up corrupt, so
        // id-only lookups still route here — and fail loudly.
        let mut node_owner: Vec<(NodeId, SessionId)> = vec![];
        let mut branch_owner: Vec<(BranchId, SessionId)> = vec![];
        let mut offset = 0; // byte offset of the current line

        for line in lines.iter().copied() {
            if !line.trim().is_empty() {
                match serde_json::from_str::<Record>(line) {
                    Ok(rec) => {
                        match &rec {
                            Record::Branch(b) => {
                                branch_owner.push((b.id.clone(), b.session.clone()));
                            }
                            Record::Node(node) => {
                                node_owner.push((node.id.clone(), node.session.clone()));
                            }
                            _ => {}
                        }
                        if let Err(e) = fold_record(&mut sid, &mut state, rec) {
                            corrupt = Some(e.to_string());
                            break;
                        }
                    }
                    // A newline-terminated line can only be real corruption:
                    // the writer emits line + '\n' in one `write_all`, so a
                    // torn write always lacks the newline (the tail below).
                    Err(e) => {
                        corrupt = Some(e.to_string());
                        index_unreadable_line(line, &sid, &mut node_owner, &mut branch_owner);
                        break;
                    }
                }
            }
            offset += line.len() + 1;
        }

        // The unterminated tail: accept it if it is a complete record (and
        // re-terminate the file), drop it if it is torn.
        if let Some(line) = tail.filter(|t| !t.trim().is_empty())
            && corrupt.is_none()
        {
            match serde_json::from_str::<Record>(line) {
                Ok(rec) => {
                    match &rec {
                        Record::Branch(b) => branch_owner.push((b.id.clone(), b.session.clone())),
                        Record::Node(node) => {
                            node_owner.push((node.id.clone(), node.session.clone()))
                        }
                        _ => {}
                    }
                    if let Err(e) = fold_record(&mut sid, &mut state, rec) {
                        corrupt = Some(e.to_string());
                    } else {
                        ensure_trailing_newline(path)?;
                    }
                }
                Err(e) if is_torn(&e) => {
                    warn!(path = %path.display(), error = %e, "dropping torn last line");
                    truncate(path, offset)?;
                }
                Err(e) => {
                    corrupt = Some(e.to_string());
                    index_unreadable_line(line, &sid, &mut node_owner, &mut branch_owner);
                }
            }
        }

        self.node_owner.extend(node_owner);
        self.branch_owner.extend(branch_owner);
        let key = sid.clone().unwrap_or(SessionId(stem));
        match (state, corrupt) {
            (Some(state), None) => {
                self.sessions.insert(key, Entry::Loaded(Box::new(state)));
            }
            (_, Some(msg)) => {
                warn!(
                    path = %path.display(),
                    error = %msg,
                    "session unreadable, hidden from listings"
                );
                self.sessions.insert(key, Entry::Corrupt(msg));
            }
            // Nothing but blank lines: no session to register yet.
            (None, None) => {}
        }
        Ok(())
    }
}

impl SessionState {
    fn new(info: SessionInfo) -> Self {
        Self {
            info,
            branches: HashMap::new(),
            nodes: HashMap::new(),
            code_calls: HashMap::new(),
        }
    }

    fn apply(&mut self, rec: Record) {
        match rec {
            Record::Session(info) => self.info = info,
            Record::Meta {
                title,
                active_branch,
                updated_at,
            } => {
                if let Some(v) = title {
                    self.info.title = v;
                }
                if let Some(v) = active_branch {
                    self.info.active_branch = v;
                }
                if let Some(v) = updated_at {
                    self.info.updated_at = v;
                }
            }
            Record::Branch(b) => {
                self.branches.insert(b.id.clone(), b);
            }
            Record::BranchUpdate { id, head, status } => {
                if let Some(b) = self.branches.get_mut(&id) {
                    if let Some(head) = head {
                        b.head = head;
                    }
                    if let Some(status) = status {
                        b.status = status;
                    }
                }
            }
            Record::Node(n) => {
                self.nodes.insert(n.id.clone(), *n);
            }
            Record::CodeCall(c) => {
                self.code_calls.insert((c.parent_call.clone(), c.seq), c);
            }
        }
    }
}

/// Fold one record into the accumulating state. The session header must come
/// first — every writer path appends it before anything else.
fn fold_record(
    sid: &mut Option<SessionId>,
    state: &mut Option<SessionState>,
    rec: Record,
) -> Result<()> {
    if let Record::Session(info) = rec {
        *sid = Some(info.id.clone());
        match state {
            Some(st) => st.info = info,
            None => *state = Some(SessionState::new(info)),
        }
        return Ok(());
    }
    let Some(st) = state else {
        return Err(Error::RecordBeforeHeader);
    };
    st.apply(rec);
    Ok(())
}

/// A serde error that means "the JSON itself is broken" (a torn write), as
/// opposed to a semantic problem like an unknown variant (a clear error).
fn is_torn(e: &serde_json::Error) -> bool {
    use serde_json::error::Category;
    matches!(e.classify(), Category::Eof | Category::Syntax)
}

/// A line rumu cannot fold still reveals its ids: route id-only lookups to
/// its session so `get_node`/`get_branch` fail loudly instead of returning
/// `None` for a node that is right there on disk.
fn index_unreadable_line(
    line: &str,
    sid: &Option<SessionId>,
    node_owner: &mut Vec<(NodeId, SessionId)>,
    branch_owner: &mut Vec<(BranchId, SessionId)>,
) {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
        return; // not even JSON: nothing to route
    };
    let owner = obj
        .get("session")
        .and_then(Value::as_str)
        .map(SessionId::from)
        .or_else(|| sid.clone());
    let Some(owner) = owner else {
        return;
    };
    let Some(id) = obj.get("id").and_then(Value::as_str) else {
        return;
    };
    match obj.get("t").and_then(Value::as_str) {
        Some("node") => node_owner.push((NodeId(id.to_owned()), owner)),
        Some("branch") => branch_owner.push((BranchId(id.to_owned()), owner)),
        _ => {}
    }
}

fn unreadable(sid: &SessionId, msg: &str) -> Error {
    Error::Unreadable {
        session: sid.clone(),
        message: msg.to_owned(),
    }
}

/// Cut `path` back to its first `len` bytes (drops a torn trailing line).
fn truncate(path: &Path, len: usize) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(IoError::at("reopening", path))?
        .set_len(len as u64)
        .map_err(IoError::at("truncating", path))?;
    Ok(())
}

/// Terminate a file whose last complete record lost its newline, so the next
/// append starts on a fresh line.
fn ensure_trailing_newline(path: &Path) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(IoError::at("reopening", path))?;
    file.write_all(b"\n")
        .map_err(IoError::at("extending", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::{BranchKind, NodeBody, now};

    fn mk_session(store: &Store) -> (SessionInfo, Branch, Node) {
        let root = Node {
            id: NodeId::new(),
            parent: None,
            session: SessionId::new(),
            at: now(),
            dur_ms: None,
            body: NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::SessionRoot {}),
            },
        };
        let branch = Branch {
            id: BranchId::new(),
            session: root.session.clone(),
            head: root.id.clone(),
            forked_from: None,
            kind: BranchKind::Main,
            status: BranchStatus::Active,
            created_at: now(),
        };
        let session = SessionInfo {
            id: root.session.clone(),
            title: String::new(),
            active_branch: branch.id.clone(),
            created_at: root.at,
            updated_at: root.at,
        };
        store.insert_session(&session).unwrap();
        store.insert_branch(&branch).unwrap();
        store.insert_node(&root).unwrap();
        (session, branch, root)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rumu-store-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_and_persistence() {
        let store = Store::in_memory();
        let (session, branch, root) = mk_session(&store);

        let child = Node {
            id: NodeId::new(),
            parent: Some(root.id.clone()),
            session: session.id.clone(),
            at: now(),
            dur_ms: Some(1234),
            body: NodeBody::User {
                content: rumu_core::Content::text("hi"),
            },
        };
        store.insert_node(&child).unwrap();
        let mut b = store.get_branch(&branch.id).unwrap().unwrap();
        b.head = child.id.clone();
        store.update_branch(&b).unwrap();

        let got = store.get_node(&child.id).unwrap().unwrap();
        assert_eq!(got.dur_ms, Some(1234));
        assert_eq!(
            serde_json::to_value(&got.body).unwrap(),
            serde_json::to_value(&child.body).unwrap()
        );
        assert_eq!(store.nodes_of_session(&session.id).unwrap().len(), 2);
        assert_eq!(
            store.get_branch(&branch.id).unwrap().unwrap().head,
            child.id
        );

        // The same data survives reopening a file-backed store.
        let dir = temp_dir("roundtrip");
        {
            let store = Store::open(&dir).unwrap();
            mk_session(&store);
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(store.list_sessions().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Unknown session/meta fields (e.g. a stale `tags`) are ignored, not a
    /// load error.
    #[test]
    fn unknown_session_fields_load_cleanly() {
        let dir = temp_dir("unknown-fields");
        let sid = "01UNKNOWNFIELDS000000000000";
        let line = |s: &str| format!("{s}\n");
        let content = [
            line(&format!(
                r#"{{"t":"session","id":"{sid}","title":"","tags":["infra"],"active_branch":"B1","created_at":1,"updated_at":1}}"#
            )),
            line(&format!(
                r#"{{"t":"branch","id":"B1","session":"{sid}","head":"N0","forked_from":null,"kind":{{"type":"main"}},"status":"active","created_at":1}}"#
            )),
            line(&format!(
                r#"{{"t":"node","id":"N0","parent":null,"session":"{sid}","at":1,"body":{{"type":"note","source":"system","content":{{"text":"","data":{{"tag":"root","v":1,"data":{{}}}}}}}}}}"#
            )),
            line(&format!(
                r#"{{"t":"node","id":"N1","parent":"N0","session":"{sid}","at":2,"body":{{"type":"note","source":"system","content":{{"text":"","data":{{"tag":"tag","v":1,"data":{{"name":"infra"}}}}}}}}}}"#
            )),
            line(r#"{"t":"meta","title":"meta","tags":["infra"],"updated_at":3}"#),
        ]
        .concat();
        std::fs::write(dir.join(format!("{sid}.jsonl")), content).unwrap();

        let store = Store::open(&dir).unwrap();
        let session = store
            .get_session(&SessionId::from(sid))
            .unwrap()
            .expect("session loads");
        assert_eq!(session.title, "meta");
        assert_eq!(session.id, SessionId::from(sid));
        assert_eq!(session.updated_at, 3);
        assert_eq!(store.nodes_of_session(&session.id).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A node body rumu cannot deserialize (e.g. a `summary` node written by
    /// another build) is a clear error, never a silent placeholder — and one
    /// bad session does not stop the others from loading and listing.
    #[test]
    fn unknown_node_type_is_a_clear_error() {
        let dir = temp_dir("ghost");
        let sid;
        {
            let store = Store::open(&dir).unwrap();
            let (session, _, _) = mk_session(&store);
            sid = session.id.0.clone();
        }
        let ghost = format!(
            r#"{{"t":"node","id":"ghost","parent":null,"session":"{sid}","at":1,"body":{{"type":"summary","origin":"b","text":"t"}}}}"#
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join(format!("{sid}.jsonl")))
            .unwrap();
        file.write_all(format!("{ghost}\n").as_bytes()).unwrap();
        drop(file);

        let store = Store::open(&dir).unwrap();
        let sid = SessionId(sid);
        let err = store.get_node(&NodeId("ghost".into())).unwrap_err();
        assert!(
            err.to_string().contains("unknown variant"),
            "names the serde problem: {err}"
        );
        assert!(
            store.nodes_of_session(&sid).is_err(),
            "listings fail the same way, not silently"
        );
        assert!(store.get_session(&sid).is_err());

        // A second, healthy session is unaffected.
        let (other, _, _) = mk_session(&store);
        let listed = store.list_sessions().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, other.id);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn code_calls_roundtrip() {
        let store = Store::in_memory();
        let (session, branch, _) = mk_session(&store);
        let call = CodeCall {
            branch: branch.id.clone(),
            parent_call: "c1".into(),
            seq: 1,
            name: "read".into(),
            args: serde_json::json!({ "path": "a.rs" }),
            output: "contents".into(),
            is_error: false,
            blocked: None,
            dur_ms: 12,
            at: now(),
        };
        store.insert_code_call(&session.id, &call).unwrap();
        let blocked = CodeCall {
            seq: 2,
            name: "bash".into(),
            args: serde_json::json!({ "command": "rm -rf /" }),
            output: "Blocked by hook: nope".into(),
            is_error: true,
            blocked: Some("nope".into()),
            ..call.clone()
        };
        store.insert_code_call(&session.id, &blocked).unwrap();
        // The same (parent_call, seq) again: the later record replaces it.
        let mut rerun = call.clone();
        rerun.output = "contents v2".into();
        rerun.at = now();
        store.insert_code_call(&session.id, &rerun).unwrap();

        let got = store.code_calls_of_session(&session.id).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "read");
        assert_eq!(got[0].args["path"], "a.rs");
        assert_eq!(got[0].seq, 1);
        assert_eq!(got[0].output, "contents v2");
        assert!(got[0].blocked.is_none());
        assert_eq!(got[1].seq, 2);
        assert_eq!(got[1].blocked.as_deref(), Some("nope"));
        assert!(got[1].is_error);
        // Another session sees nothing.
        let (other, _, _) = mk_session(&store);
        assert!(store.code_calls_of_session(&other.id).unwrap().is_empty());
    }

    fn attachment(hash: &str) -> rumu_core::Attachment {
        rumu_core::Attachment {
            hash: hash.to_owned(),
            mime: "image/png".into(),
            name: Some(format!("{hash}.png")),
            width: 4,
            height: 3,
            size: 10,
        }
    }

    /// The blob reference set covers every content of every node of every
    /// session — all four body variants — and survives a reopen.
    #[test]
    fn referenced_blobs_covers_every_body_variant() {
        let dir = temp_dir("blobs");
        let sid = {
            let store = Store::open(&dir).unwrap();
            let (session, _, root) = mk_session(&store);
            let node = |session: &SessionId, parent: &NodeId, body| Node {
                id: NodeId::new(),
                parent: Some(parent.clone()),
                session: session.clone(),
                at: now(),
                dur_ms: None,
                body,
            };
            store
                .insert_node(&node(
                    &session.id,
                    &root.id,
                    NodeBody::User {
                        content: rumu_core::Content::text("look [Image #1]")
                            .with_attachments([attachment("aa"), attachment("bb")]),
                    },
                ))
                .unwrap();
            store
                .insert_node(&node(
                    &session.id,
                    &root.id,
                    NodeBody::Assistant {
                        content: rumu_core::Content::text("").with_attachments([attachment("ee")]),
                        reasoning: vec![],
                        reason_ms: None,
                        reasoning_reset: false,
                        tool_calls: vec![],
                        model: "m".into(),
                        usage: Default::default(),
                    },
                ))
                .unwrap();
            store
                .insert_node(&node(
                    &session.id,
                    &root.id,
                    NodeBody::ToolResult {
                        call_id: "c1".into(),
                        content: rumu_core::Content::text("shot")
                            .with_attachments([attachment("cc")]),
                        is_error: false,
                    },
                ))
                .unwrap();
            // A second session references the same blob plus one of its own.
            let (other, _, other_root) = mk_session(&store);
            store
                .insert_node(&node(
                    &other.id,
                    &other_root.id,
                    NodeBody::Note {
                        source: "system".into(),
                        content: rumu_core::Content::text("note")
                            .with_attachments([attachment("aa"), attachment("dd")]),
                    },
                ))
                .unwrap();
            let hashes = store.referenced_blobs().unwrap();
            assert_eq!(
                hashes,
                ["aa", "bb", "cc", "dd", "ee"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<HashSet<_>>()
            );
            session.id
        };

        // The same set comes back from a reopened (re-scanned) store.
        let store = Store::open(&dir).unwrap();
        assert_eq!(store.referenced_blobs().unwrap().len(), 5);
        assert!(store.get_session(&sid).unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A crash mid-append leaves a torn last line: it is dropped with a
    /// warning and truncated, and later appends land cleanly.
    #[test]
    fn torn_last_line_is_tolerated_and_truncated() {
        let dir = temp_dir("torn");
        let (sid, root_id) = {
            let store = Store::open(&dir).unwrap();
            let (session, _, root) = mk_session(&store);
            (session.id, root.id)
        };
        let path = dir.join(format!("{sid}.jsonl"));
        let clean_len = std::fs::metadata(&path).unwrap().len();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"t":"node","id":"01TORN"#).unwrap(); // cut mid-line
        drop(file);

        let store = Store::open(&dir).unwrap();
        assert_eq!(store.list_sessions().unwrap().len(), 1);
        assert_eq!(store.nodes_of_session(&sid).unwrap().len(), 1); // just the root
        assert_eq!(std::fs::metadata(&path).unwrap().len(), clean_len);
        assert!(std::fs::read_to_string(&path).unwrap().ends_with('\n'));

        // The next append starts on a clean line and survives a reopen.
        let child = Node {
            id: NodeId::new(),
            parent: Some(root_id.clone()),
            session: sid.clone(),
            at: now(),
            dur_ms: None,
            body: NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content {
                    text: String::new(),
                    attachments: Vec::new(),
                    data: Some(rumu_core::Tagged {
                        tag: "x".into(),
                        v: 1,
                        data: serde_json::json!({}),
                    }),
                },
            },
        };
        store.insert_node(&child).unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        assert_eq!(store.nodes_of_session(&sid).unwrap().len(), 2);
        assert!(store.get_node(&child.id).unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A newline-terminated line is never a torn write (each append is one
    /// `write_all` of line + '\n'): garbage there is real corruption. The
    /// session errors, and the file is left untouched.
    #[test]
    fn broken_complete_line_is_corruption_not_torn() {
        let dir = temp_dir("broken");
        let sid = {
            let store = Store::open(&dir).unwrap();
            let (session, _, _) = mk_session(&store);
            session.id
        };
        let path = dir.join(format!("{sid}.jsonl"));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{oops\n").unwrap();
        drop(file);
        let len_before = std::fs::metadata(&path).unwrap().len();

        let store = Store::open(&dir).unwrap();
        assert!(store.get_session(&sid).is_err());
        assert!(store.nodes_of_session(&sid).is_err());
        assert!(store.list_sessions().unwrap().is_empty());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), len_before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every mutation is a folded record: all of them survive a reopen, and
    /// id-only lookups keep working straight from the scan.
    #[test]
    fn mutations_survive_reopen() {
        let dir = temp_dir("mutations");
        let (sid, branch_id, child_id) = {
            let store = Store::open(&dir).unwrap();
            let (session, branch, root) = mk_session(&store);
            let child = Node {
                id: NodeId::new(),
                parent: Some(root.id.clone()),
                session: session.id.clone(),
                at: now(),
                dur_ms: Some(5),
                body: NodeBody::User {
                    content: rumu_core::Content::text("hi"),
                },
            };
            store.insert_node(&child).unwrap();
            store.touch_session(&session.id, 77).unwrap();

            let mut b = branch.clone();
            b.head = child.id.clone();
            b.status = BranchStatus::Done;
            store.update_branch(&b).unwrap();

            let mut s = session.clone();
            s.title = "t1".into();
            store.update_session(&s).unwrap();
            (session.id, branch.id, child.id)
        };

        let store = Store::open(&dir).unwrap();
        let b = store.get_branch(&branch_id).unwrap().unwrap();
        assert_eq!(b.head, child_id);
        assert_eq!(b.status, BranchStatus::Done);

        let s = store.get_session(&sid).unwrap().unwrap();
        assert_eq!(s.title, "t1");

        // Id-only lookups work off the indexes built during the scan.
        assert!(store.get_node(&child_id).unwrap().is_some());
        assert!(store.get_branch(&branch_id).unwrap().is_some());
        assert_eq!(store.nodes_of_session(&sid).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
