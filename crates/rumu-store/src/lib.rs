//! rumu-store: per-session JSONL persistence for the rumu kernel.
//!
//! Every session is one append-only `<session-id>.jsonl` file inside the
//! directory given to [`Store::open`] (the server passes `~/.rumu/sessions`).
//! Each line is one tagged JSON record, event-sourced style:
//!
//! ```text
//! {"t":"session","id":"7c9e…","title":"","main_branch":"7c9e…","created_at":1,"updated_at":1}
//! {"t":"branch","id":"7c9e…","session":"7c9e…","head":"7c9e…","forked_from":null,"kind":{"type":"main"},"status":"active","created_at":1}
//! {"t":"node","id":"7c9e…","parent":null,"session":"7c9e…","at":1,"body":{"type":"note","source":"system","content":{"text":"","data":{"tag":"session.root","v":1,"data":{}}}}}
//! {"t":"node","branch":"7c9e…","id":"5d1a…","parent":"7c9e…","session":"7c9e…","at":2,"body":{"type":"user","content":{"text":"hi"}}}
//! {"t":"branch_update","id":"7c9e…","status":"done"}
//! {"t":"branch_update","id":"7c9e…","head":"5d1a…"}
//! {"t":"task","id":"t-1a2b3c4d","session":"7c9e…","branch":"7c9e…","status":"running",…}
//! {"t":"meta","title":"refactor","updated_at":3}
//! ```
//!
//! When the user last looked at each session (`SessionInfo::seen_at`) is
//! this machine's view state, not part of the journal: it lives in one
//! `seen.json` map (`{session id: timestamp}`) beside the sessions
//! directory, rewritten whole on every change.
//!
//! Loading a file folds the records into memory: `session` is the header
//! (the initial [`SessionInfo`]), `meta` patches it (title / project /
//! updated_at — whatever the mutating call changed),
//! `branch` adds a branch and `branch_update` moves its head/status, `node`
//! adds a node — with `branch`, it is also that branch's new head — and `task`
//! adds or updates one background task (the kernel writes one when a task
//! starts and when it ends, so a restart knows what it lost).
//!
//! Older journals gave every edit and regenerate a main branch of its own and
//! named the current one `active_branch` (in the header, then in `meta`).
//! Folding such a file keeps that one as the session's main line and drops
//! the others: their nodes stay in the tree as variants, and whatever named
//! them (a sub-agent's or side question's parent, a task) now names the main
//! line (see [`SessionState::merge_main_lines`]).
//! A session's `updated_at` is never older than its newest node; its
//! `touched_at` follows the user's prompts on main lines and project moves.
//! Optional fields are `#[serde(default)]` and
//! skipped when unset, so the format can grow without migrations; records
//! with an unknown tag (e.g. a retired record type) are ignored.
//!
//! [`Store::open`] scans the directory once and folds every file, but keeps
//! only each session's skeleton: its info, branches and tasks, the blobs its
//! nodes reference, and whether its main line ended in an error. The nodes
//! are read from the journal again when something first needs them (outside
//! the store's lock), and an idle session's nodes are dropped again,
//! least recently used first, while the journals of the sessions in memory
//! exceed the store's budget ([`DEFAULT_BUDGET`]). A session that is doing
//! something ([`SessionStatus`] other than idle or failed) keeps its nodes.
//! Appending a node loads them first, so the nodes in memory always match the
//! journal. The state is authoritative otherwise (single process): each
//! mutation appends exactly one line with a single `write_all` (the file is
//! reopened in append mode per write; no fsync — a crash loses at most the
//! tail). A node is looked up within its session; `get_branch` routes through
//! a branch→session index built during the scan and maintained on inserts.
//!
//! Every node is also in a full-text index ([`rumu_search`]), derived from
//! the journals: [`Store::open_indexed`] keeps it on disk and re-indexes, on
//! a background thread, each session whose journal is not what the index
//! covers; [`Store::open`] and [`Store::in_memory`] build it in memory.
//!
//! A crash mid-append can tear the last line; on load it is dropped with a
//! warning and truncated so the next append starts on a clean line. Any other
//! malformed line — including a record rumu cannot deserialize, e.g. a node
//! body written by a newer build — marks just that session unreadable: every
//! read of it is a clear error naming the serde problem, while the other
//! sessions keep loading and listing.

mod error;
pub use error::{Error, Result};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rumu_core::{
    Artifact, Branch, BranchId, BranchKind, BranchStatus, IoError, McpApp, Node, NodeBody, NodeId,
    SessionId, SessionInfo, SessionStatus, TaskId, TaskInfo, Timestamp, TurnError,
};
use rumu_search::{Found, SessionIndex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

pub struct Store {
    /// Directory of `<session-id>.jsonl` files; `None` for in-memory stores
    /// (which keep every node).
    root: Option<PathBuf>,
    /// The `seen.json` file; `None` for in-memory stores.
    seen_path: Option<PathBuf>,
    /// The most journal bytes whose nodes idle sessions keep in memory.
    budget: u64,
    index: Arc<SessionIndex>,
    state: Mutex<StoreState>,
}

/// The default [`Store::with_budget`]: about 100 MB of nodes in memory.
pub const DEFAULT_BUDGET: u64 = 64 << 20;

/// Reads a present field as its inner `Option`: an absent field (handled by
/// `#[serde(default)]`) means "unchanged", an explicit `null` means "clear
/// it". Needed to persist a nullable field through a sparse patch record.
fn double_option<'de, D>(d: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
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
        /// Older journals only: the main branch the user switched to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_branch: Option<BranchId>,
        /// The session's project: an absent field leaves it unchanged, an
        /// explicit `null` clears it.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "double_option"
        )]
        project: Option<Option<String>>,
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
    /// A node; with `branch`, it was appended there and is that branch's
    /// new head.
    Node {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<BranchId>,
        // Boxed: `Node` dwarfs the other variants and every record is built
        // once per mutation.
        #[serde(flatten)]
        node: Box<Node>,
    },
    /// A background task's start or a later status change; the latest record
    /// per id wins. Written by the kernel so a restart can tell what it lost
    /// (the runtime table itself is not persisted).
    Task(Box<TaskInfo>),
    /// A record tag this build does not know (e.g. a retired record type):
    /// parsed and dropped so old journals still load.
    #[serde(other)]
    Unknown,
}

#[derive(Default)]
struct StoreState {
    sessions: HashMap<SessionId, Entry>,
    /// `get_branch` routes through this; built during the open scan and kept
    /// current on inserts.
    branch_owner: HashMap<BranchId, SessionId>,
    /// Journal bytes of the sessions whose nodes are in memory.
    loaded_bytes: u64,
    /// Ticks on every use of a session's nodes: the eviction order.
    clock: u64,
    /// When the user last looked at each session (`seen.json`).
    seen: BTreeMap<SessionId, Timestamp>,
    /// What each session not idle is doing (`SessionInfo::status`); set by
    /// the kernel, never written.
    status: HashMap<SessionId, SessionStatus>,
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
    /// Background tasks by id, oldest first when read (see
    /// [`Store::tasks_of`]); their status is the latest record's.
    tasks: HashMap<TaskId, TaskInfo>,
    /// Every blob a node references (see [`Store::referenced_blobs`]).
    blobs: HashSet<String>,
    /// The nodes; `None` while only the skeleton is in memory.
    nodes: Option<HashMap<NodeId, Node>>,
    /// The journal's size: roughly what its nodes cost in memory.
    bytes: u64,
    /// Bumped on every node append, so a load that raced one is dropped.
    node_gen: u64,
    /// [`StoreState::clock`] when the nodes were last used.
    used: u64,
    /// [`Store::line_failed`] of the main line when the nodes were dropped:
    /// the branch, its head then, and the answer.
    failed: Option<(BranchId, NodeId, bool)>,
}

impl Store {
    /// Open (creating if needed) the directory of per-session `.jsonl` files
    /// and the `seen` map file. Every file is scanned once, up front; the
    /// search index is built in memory before this returns.
    pub fn open(root: &Path, seen: &Path) -> Result<Self> {
        let store = Self::scan(root, seen, Arc::new(SessionIndex::in_memory()))?;
        let stale = store.stale_sessions(&BTreeMap::new());
        store.reindex(stale);
        Ok(store)
    }

    /// [`open`](Self::open) with the search index kept in `index_dir`: the
    /// sessions it does not cover are re-indexed on a background thread.
    pub fn open_indexed(root: &Path, seen: &Path, index_dir: &Path) -> Result<Self> {
        let (index, covered) = SessionIndex::open(index_dir).map_err(index_error)?;
        let store = Self::scan(root, seen, Arc::new(index))?;
        let stale = store.stale_sessions(&covered);
        for gone in covered.keys().filter(|id| {
            !store
                .state
                .lock()
                .unwrap()
                .sessions
                .contains_key(&SessionId::from(id.as_str()))
        }) {
            store.index.delete_session(&SessionId::from(gone.as_str()));
        }
        if !stale.is_empty() {
            let index = store.index.clone();
            std::thread::Builder::new()
                .name("session-reindex".into())
                .spawn(move || reindex(&index, stale))
                .map_err(IoError::at("spawning the re-index of", index_dir))?;
        }
        Ok(store)
    }

    /// The readable sessions whose journal length is not the one `covered`
    /// records, with their journals, marked stale in the index.
    fn stale_sessions(&self, covered: &BTreeMap<String, u64>) -> Vec<(SessionId, PathBuf)> {
        let st = self.state.lock().unwrap();
        let mut stale: Vec<(SessionId, PathBuf)> = st
            .sessions
            .iter()
            .filter_map(|(id, e)| match e {
                Entry::Loaded(s) if covered.get(&id.0) != Some(&s.bytes) => {
                    Some((id.clone(), self.journal(id)?))
                }
                _ => None,
            })
            .collect();
        stale.sort();
        self.index
            .stale(stale.iter().map(|(id, _)| id.0.clone()).collect());
        stale
    }

    /// Re-index `stale` here and now.
    fn reindex(&self, stale: Vec<(SessionId, PathBuf)>) {
        reindex(&self.index, stale);
    }

    fn scan(root: &Path, seen: &Path, index: Arc<SessionIndex>) -> Result<Self> {
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
        state.seen = match std::fs::read(seen) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!("{}: {e}; starting with every session seen", seen.display());
                BTreeMap::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(IoError::at("reading", seen)(e).into()),
        };
        Ok(Self {
            root: Some(root.to_path_buf()),
            seen_path: Some(seen.to_path_buf()),
            budget: DEFAULT_BUDGET,
            index,
            state: Mutex::new(state),
        })
    }

    /// A store with no files behind it (used by tests).
    pub fn in_memory() -> Self {
        Self {
            root: None,
            seen_path: None,
            budget: DEFAULT_BUDGET,
            index: Arc::new(SessionIndex::in_memory()),
            state: Mutex::new(StoreState::default()),
        }
    }

    /// Keep the nodes of idle sessions in memory only while their journals
    /// add up to at most `bytes`.
    pub fn with_budget(mut self, bytes: u64) -> Self {
        self.budget = bytes;
        self
    }

    fn journal(&self, session: &SessionId) -> Option<PathBuf> {
        Some(self.root.as_ref()?.join(format!("{session}.jsonl")))
    }

    /// Lock the state with `session`'s nodes in memory, reading them from its
    /// journal first (outside the lock) when they are not. `Ok(None)`: no
    /// such session.
    fn lock_nodes(&self, session: &SessionId) -> Result<Option<MutexGuard<'_, StoreState>>> {
        loop {
            let (path, generation) = {
                let mut st = self.state.lock().unwrap();
                st.clock += 1;
                let clock = st.clock;
                match st.sessions.get_mut(session) {
                    None => return Ok(None),
                    Some(Entry::Corrupt(msg)) => return Err(unreadable(session, msg)),
                    Some(Entry::Loaded(s)) if s.nodes.is_some() => {
                        s.used = clock;
                        return Ok(Some(st));
                    }
                    Some(Entry::Loaded(s)) => match self.journal(session) {
                        Some(path) => (path, s.node_gen),
                        None => {
                            s.nodes = Some(HashMap::new());
                            return Ok(Some(st));
                        }
                    },
                }
            };
            let nodes = read_nodes(session, &path)?;
            let mut guard = self.state.lock().unwrap();
            let st = &mut *guard;
            match st.sessions.get_mut(session) {
                None => return Ok(None),
                Some(Entry::Corrupt(msg)) => return Err(unreadable(session, msg)),
                Some(Entry::Loaded(s)) if s.nodes.is_some() => return Ok(Some(guard)),
                // A node landed while the journal was read: read it again.
                Some(Entry::Loaded(s)) if s.node_gen != generation => continue,
                Some(Entry::Loaded(s)) => {
                    s.nodes = Some(nodes);
                    st.loaded_bytes += s.bytes;
                    self.evict(st, Some(session));
                    return Ok(Some(guard));
                }
            }
        }
    }

    /// Drop the nodes of idle sessions, least recently used first, until
    /// the ones in memory fit the budget. `keep` and busy sessions stay.
    fn evict(&self, st: &mut StoreState, keep: Option<&SessionId>) {
        if self.root.is_none() {
            return;
        }
        while st.loaded_bytes > self.budget {
            let StoreState {
                sessions, status, ..
            } = &*st;
            let busy = |id: &SessionId| {
                !matches!(
                    status.get(id).copied().unwrap_or_default(),
                    SessionStatus::Idle | SessionStatus::Failed
                )
            };
            let victim = sessions
                .iter()
                .filter_map(|(id, e)| match e {
                    Entry::Loaded(s) if s.nodes.is_some() && Some(id) != keep && !busy(id) => {
                        Some((s.used, id))
                    }
                    _ => None,
                })
                .min_by_key(|(used, _)| *used)
                .map(|(_, id)| id.clone());
            let Some(id) = victim else {
                break;
            };
            if let Some(Entry::Loaded(s)) = st.sessions.get_mut(&id) {
                s.unload();
                st.loaded_bytes -= s.bytes;
            }
        }
    }

    /// Append one record as a single line (one buffered `write_all`),
    /// returning its length. In-memory stores have no files.
    fn append(&self, session: &SessionId, rec: &Record) -> Result<u64> {
        let Some(root) = &self.root else {
            return Ok(0);
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
        Ok(line.len() as u64)
    }

    /// Rewrite `seen.json` from `seen` (a temp file renamed over it).
    fn save_seen(&self, seen: &BTreeMap<SessionId, Timestamp>) -> Result<()> {
        let Some(path) = &self.seen_path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(seen).expect("a map of strings to numbers serializes");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(IoError::at("writing", &tmp))?;
        std::fs::rename(&tmp, path).map_err(IoError::at("renaming", &tmp))?;
        Ok(())
    }

    // ---- sessions ----

    pub fn insert_session(&self, s: &SessionInfo) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.sessions.contains_key(&s.id) {
            return Err(Error::SessionExists { id: s.id.clone() });
        }
        let info = SessionInfo {
            seen_at: None,
            status: SessionStatus::Idle,
            ..s.clone()
        };
        let bytes = self.append(&s.id, &Record::Session(info.clone()))?;
        let mut state = SessionState::new(info);
        state.bytes = bytes;
        st.clock += 1;
        state.used = st.clock;
        st.loaded_bytes += bytes;
        st.sessions
            .insert(s.id.clone(), Entry::Loaded(Box::new(state)));
        self.index.touch(&s.id, bytes);
        if let Some(at) = s.seen_at {
            st.seen.insert(s.id.clone(), at);
            self.save_seen(&st.seen)?;
        }
        Ok(())
    }

    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionInfo>> {
        let st = self.state.lock().unwrap();
        Ok(match st.sessions.get(id) {
            Some(Entry::Loaded(s)) => Some(st.info(s)),
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
                Entry::Loaded(s) => Some(st.info(s)),
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
            active_branch: None,
            project: Some(s.project.clone()),
            updated_at: Some(s.updated_at),
        };
        let bytes = self.append(&s.id, &rec)?;
        self.applied(&mut st, &s.id, rec, bytes);
        Ok(())
    }

    /// Record that the user looked at the session at `at` (a no-op on
    /// unknown sessions).
    pub fn mark_seen(&self, id: &SessionId, at: Timestamp) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.loaded(id)?.is_none() {
            return Ok(());
        }
        st.seen.insert(id.clone(), at);
        self.save_seen(&st.seen)
    }

    /// Delete a session: its journal file and every in-memory record and
    /// index entry. A missing session is a no-op. A torn write racing the
    /// delete cannot resurrect the file: later appends to an unknown session
    /// are dropped (see the insert methods below).
    pub fn delete_session(&self, id: &SessionId) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if !st.sessions.contains_key(id) {
            return Ok(());
        }
        if let Some(root) = &self.root {
            let path = root.join(format!("{id}.jsonl"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                // Already gone (e.g. an in-memory-only session's file was
                // removed by hand): the state is what matters now.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(IoError::at("removing", &path)(e).into()),
            }
        }
        if let Some(Entry::Loaded(s)) = st.sessions.remove(id) {
            for branch in s.branches.keys() {
                st.branch_owner.remove(branch);
            }
            if s.nodes.is_some() {
                st.loaded_bytes -= s.bytes;
            }
        }
        st.status.remove(id);
        self.index.delete_session(id);
        if st.seen.remove(id).is_some() {
            self.save_seen(&st.seen)?;
        }
        Ok(())
    }

    /// Record what `id` is doing; true when that changed.
    /// A session that settles may have its nodes dropped now.
    pub fn set_status(&self, id: &SessionId, status: SessionStatus) -> bool {
        let mut st = self.state.lock().unwrap();
        let old = if status.is_idle() || !st.sessions.contains_key(id) {
            st.status.remove(id)
        } else {
            st.status.insert(id.clone(), status)
        };
        if matches!(status, SessionStatus::Idle | SessionStatus::Failed) {
            self.evict(&mut st, None);
        }
        old.unwrap_or_default() != status
    }

    // ---- branches ----

    pub fn insert_branch(&self, b: &Branch) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        // The session was deleted while this write was in flight: drop it
        // rather than recreate the file.
        if st.loaded(&b.session)?.is_none() {
            return Ok(());
        }
        let rec = Record::Branch(b.clone());
        let bytes = self.append(&b.session, &rec)?;
        self.applied(&mut st, &b.session, rec, bytes);
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

    pub fn set_branch_status(&self, id: &BranchId, status: BranchStatus) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let Some(sid) = st.branch_owner.get(id).cloned() else {
            return Ok(()); // a no-op on unknown branches
        };
        if st.loaded(&sid)?.is_none() {
            return Ok(());
        }
        let rec = Record::BranchUpdate {
            id: id.clone(),
            head: None,
            status: Some(status),
        };
        let bytes = self.append(&sid, &rec)?;
        self.applied(&mut st, &sid, rec, bytes);
        Ok(())
    }

    /// Move a branch's head to `head`, a node already in its session (an
    /// edit, a regenerate, another variant picked).
    pub fn set_branch_head(&self, id: &BranchId, head: &NodeId) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let Some(sid) = st.branch_owner.get(id).cloned() else {
            return Ok(()); // a no-op on unknown branches
        };
        if st.loaded(&sid)?.is_none() {
            return Ok(());
        }
        let rec = Record::BranchUpdate {
            id: id.clone(),
            head: Some(head.clone()),
            status: None,
        };
        let bytes = self.append(&sid, &rec)?;
        self.applied(&mut st, &sid, rec, bytes);
        Ok(())
    }

    /// Record a background task's state (`TaskInfo` without its result and
    /// output, which the journal keeps as ordinary notes instead). The latest
    /// record per id wins.
    pub fn append_task(&self, task: &TaskInfo) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.loaded(&task.session)?.is_none() {
            return Ok(()); // a no-op on unknown sessions
        }
        let rec = Record::Task(Box::new(TaskInfo {
            result: None,
            data: None,
            ..task.clone()
        }));
        let bytes = self.append(&task.session, &rec)?;
        self.applied(&mut st, &task.session, rec, bytes);
        Ok(())
    }

    /// Every task recorded in `session`'s journal, oldest first.
    pub fn tasks_of(&self, session: &SessionId) -> Result<Vec<TaskInfo>> {
        let st = self.state.lock().unwrap();
        Ok(match st.sessions.get(session) {
            Some(Entry::Loaded(s)) => {
                let mut v: Vec<TaskInfo> = s.tasks.values().cloned().collect();
                v.sort_by(|a, b| {
                    a.started_at
                        .cmp(&b.started_at)
                        .then_with(|| a.id.cmp(&b.id))
                });
                v
            }
            Some(Entry::Corrupt(msg)) => return Err(unreadable(session, msg)),
            None => vec![],
        })
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

    /// Store a node without moving any branch head (session roots and
    /// copies, whose branch record already names its head).
    pub fn insert_node(&self, n: &Node) -> Result<()> {
        self.put_node(None, n)
    }

    /// Append `n` to `branch`: one record that stores the node and makes it
    /// the branch's head.
    pub fn append_node(&self, branch: &BranchId, n: &Node) -> Result<()> {
        self.put_node(Some(branch.clone()), n)
    }

    fn put_node(&self, branch: Option<BranchId>, n: &Node) -> Result<()> {
        // With the nodes in memory, so they keep matching the journal. See
        // `insert_branch`: a deleted session stays deleted.
        let Some(mut st) = self.lock_nodes(&n.session)? else {
            return Ok(());
        };
        let rec = Record::Node {
            branch,
            node: Box::new(n.clone()),
        };
        let bytes = self.append(&n.session, &rec)?;
        self.applied(&mut st, &n.session, rec, bytes);
        Ok(())
    }

    /// Fold a record just appended into the state and tell the index.
    fn applied(&self, st: &mut StoreState, sid: &SessionId, rec: Record, bytes: u64) {
        let node = match &rec {
            Record::Node { node, .. } => Some((**node).clone()),
            _ => None,
        };
        st.apply(sid, rec, bytes);
        let Some(Entry::Loaded(s)) = st.sessions.get(sid) else {
            return;
        };
        match node {
            Some(node) => self.index.upsert(&node, s.bytes),
            None => self.index.touch(sid, s.bytes),
        }
    }

    /// Full-text search over the nodes of `sessions` (see
    /// [`SessionIndex::search`]).
    pub fn search(&self, query: &str, sessions: &[SessionId], limit: usize) -> Result<Found> {
        self.index
            .search(query, sessions, limit)
            .map_err(index_error)
    }

    /// Every node id of `session` the search index knows.
    pub fn indexed_nodes(&self, session: &SessionId) -> Result<Vec<String>> {
        self.index.node_ids(session).map_err(index_error)
    }

    /// The node `id` of `session`.
    pub fn get_node(&self, session: &SessionId, id: &NodeId) -> Result<Option<Node>> {
        let Some(st) = self.lock_nodes(session)? else {
            return Ok(None);
        };
        Ok(st.nodes_of(session).get(id).cloned())
    }

    /// Did the last turn on `branch` end in an error? Its [`TurnError`]
    /// marker is the last node of the line, but for notes written after it.
    /// An idle session whose main line has not moved since its nodes were
    /// dropped answers without reading them. `false` for an unknown branch.
    pub fn line_failed(&self, branch: &BranchId) -> Result<bool> {
        let session = {
            let st = self.state.lock().unwrap();
            let Some(sid) = st.branch_owner.get(branch) else {
                return Ok(false);
            };
            let s = match st.sessions.get(sid) {
                Some(Entry::Loaded(s)) => s,
                Some(Entry::Corrupt(msg)) => return Err(unreadable(sid, msg)),
                None => return Ok(false),
            };
            let Some(b) = s.branches.get(branch) else {
                return Ok(false);
            };
            match (&s.nodes, &s.failed) {
                (Some(nodes), _) => return Ok(line_failed(nodes, &b.head)),
                (None, Some((id, head, failed))) if id == branch && *head == b.head => {
                    return Ok(*failed);
                }
                _ => sid.clone(),
            }
        };
        let Some(st) = self.lock_nodes(&session)? else {
            return Ok(false);
        };
        let Some(Entry::Loaded(s)) = st.sessions.get(&session) else {
            return Ok(false);
        };
        Ok(s.branches
            .get(branch)
            .is_some_and(|b| line_failed(st.nodes_of(&session), &b.head)))
    }

    /// The nodes from a session's root down to `head`, root first, or `None`
    /// if the chain breaks. Clones only the path, not the whole session
    /// ([`nodes_of_session`] sorts and clones every node — too slow to run on
    /// every event of a long session).
    ///
    /// [`nodes_of_session`]: Self::nodes_of_session
    pub fn path_to(&self, session: &SessionId, head: &NodeId) -> Result<Option<Vec<Node>>> {
        let Some(st) = self.lock_nodes(session)? else {
            return Ok(None);
        };
        let nodes = st.nodes_of(session);
        let mut path = vec![];
        let mut cur = Some(head);
        while let Some(id) = cur {
            let Some(n) = nodes.get(id) else {
                return Ok(None);
            };
            path.push(n.clone());
            cur = n.parent.as_ref();
        }
        path.reverse();
        Ok(Some(path))
    }

    /// Run `f` over a session's info, nodes and branches, borrowed: for reads
    /// that pick a few nodes out of a long session without cloning the rest.
    /// `None` when there is no such session.
    pub fn with_tree<R>(
        &self,
        session: &SessionId,
        f: impl FnOnce(&SessionInfo, &HashMap<NodeId, Node>, &HashMap<BranchId, Branch>) -> R,
    ) -> Result<Option<R>> {
        let Some(st) = self.lock_nodes(session)? else {
            return Ok(None);
        };
        let Some(Entry::Loaded(s)) = st.sessions.get(session) else {
            return Ok(None);
        };
        Ok(Some(f(&s.info, st.nodes_of(session), &s.branches)))
    }

    /// All nodes of a session, oldest first.
    pub fn nodes_of_session(&self, session: &SessionId) -> Result<Vec<Node>> {
        let Some(st) = self.lock_nodes(session)? else {
            return Ok(vec![]);
        };
        let mut v: Vec<Node> = st.nodes_of(session).values().cloned().collect();
        v.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        Ok(v)
    }

    /// Every blob hash referenced by a node of any session — the attachments
    /// and `show` snapshot of every `Content` of every body variant, on every
    /// branch, referenced or not. This is the
    /// live set the blob store's GC keeps. An unreadable session is an error
    /// (its references are unknowable, so GC must not run past it).
    pub fn referenced_blobs(&self) -> Result<HashSet<String>> {
        let st = self.state.lock().unwrap();
        let mut hashes = HashSet::new();
        for (sid, entry) in &st.sessions {
            match entry {
                Entry::Loaded(s) => hashes.extend(s.blobs.iter().cloned()),
                Entry::Corrupt(msg) => return Err(unreadable(sid, msg)),
            }
        }
        Ok(hashes)
    }
}

/// The blobs `node` references: its content's attachments, `show` snapshot
/// and MCP app.
fn blobs_of(node: &Node) -> impl Iterator<Item = String> {
    let content = match &node.body {
        NodeBody::User { content }
        | NodeBody::Assistant { content, .. }
        | NodeBody::ToolResult { content, .. }
        | NodeBody::Note { content, .. } => content,
    };
    let attachments = content.attachments.iter().map(|a| a.hash.clone());
    let artifact = content.data.as_ref().and_then(|d| d.decode::<Artifact>());
    let app = content.data.as_ref().and_then(|d| d.decode::<McpApp>());
    attachments
        .collect::<Vec<_>>()
        .into_iter()
        .chain(artifact.map(|a| a.hash))
        .chain(app.into_iter().flat_map(|a| [a.html, a.call]))
}

/// Index the nodes of each `stale` session's journal, then record how much
/// of it that covers.
fn reindex(index: &SessionIndex, stale: Vec<(SessionId, PathBuf)>) {
    let (count, started) = (stale.len(), std::time::Instant::now());
    for (session, path) in stale {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "re-indexing a session failed");
                continue;
            }
        };
        let complete = content.rfind('\n').map_or("", |end| &content[..=end]);
        for line in complete.lines() {
            if let Ok(Record::Node { node, .. }) = serde_json::from_str::<Record>(line) {
                index.reindex_node(&node);
            }
        }
        index.reindexed(&session, complete.len() as u64);
    }
    if count > 0 {
        info!(sessions = count, elapsed = ?started.elapsed(), "sessions re-indexed");
    }
}

fn index_error(e: rumu_search::Error) -> Error {
    Error::Index(e.to_string())
}

/// See [`Store::line_failed`].
fn line_failed(nodes: &HashMap<NodeId, Node>, head: &NodeId) -> bool {
    let mut id = Some(head);
    while let Some(node) = id.and_then(|id| nodes.get(id)) {
        match &node.body {
            NodeBody::Note { content, .. } if content.decode::<TurnError>().is_some() => {
                return true;
            }
            NodeBody::Note { .. } => id = node.parent.as_ref(),
            _ => return false,
        }
    }
    false
}

/// Read the nodes of `session`'s journal at `path`. The scan at open already
/// vetted it; an unterminated last line is an append still being written,
/// and never a node (those are appended with the nodes in memory).
fn read_nodes(session: &SessionId, path: &Path) -> Result<HashMap<NodeId, Node>> {
    let content = std::fs::read_to_string(path).map_err(IoError::at("reading", path))?;
    let complete = content.rfind('\n').map_or("", |end| &content[..end]);
    let mut nodes = HashMap::new();
    for line in complete.split('\n').filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Record>(line) {
            Ok(Record::Node { node, .. }) => {
                nodes.insert(node.id.clone(), *node);
            }
            Ok(_) => {}
            Err(e) => return Err(unreadable(session, &e.to_string())),
        }
    }
    Ok(nodes)
}

impl StoreState {
    /// The folded state of `sid`, if it loaded cleanly. `Ok(None)` when no
    /// such session exists.
    /// A session's info, with when it was last seen.
    fn info(&self, s: &SessionState) -> SessionInfo {
        SessionInfo {
            seen_at: self.seen.get(&s.info.id).copied(),
            status: self.status.get(&s.info.id).copied().unwrap_or_default(),
            ..s.info.clone()
        }
    }

    /// The nodes of `sid`, which [`Store::lock_nodes`] put in memory.
    fn nodes_of(&self, sid: &SessionId) -> &HashMap<NodeId, Node> {
        match self.sessions.get(sid) {
            Some(Entry::Loaded(s)) => s.nodes.as_ref().expect("the nodes are in memory"),
            _ => unreachable!("lock_nodes found the session"),
        }
    }

    fn loaded<'a>(&'a mut self, sid: &SessionId) -> Result<Option<&'a mut SessionState>> {
        match self.sessions.get_mut(sid) {
            Some(Entry::Loaded(s)) => Ok(Some(s)),
            Some(Entry::Corrupt(msg)) => Err(unreadable(sid, msg)),
            None => Ok(None),
        }
    }

    /// Fold one record `bytes` long into `sid`'s state, refreshing the
    /// branch→session index.
    fn apply(&mut self, sid: &SessionId, rec: Record, bytes: u64) {
        if let Record::Branch(b) = &rec {
            self.branch_owner.insert(b.id.clone(), sid.clone());
        }
        if let Some(Entry::Loaded(s)) = self.sessions.get_mut(sid) {
            if matches!(rec, Record::Node { .. }) {
                s.node_gen += 1;
            }
            s.bytes += bytes;
            if s.nodes.is_some() {
                self.loaded_bytes += bytes;
            }
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
        // branch lookups still route here — and fail loudly.
        let mut branch_owner: Vec<(BranchId, SessionId)> = vec![];
        let mut offset = 0; // byte offset of the current line

        for line in lines.iter().copied() {
            if !line.trim().is_empty() {
                match serde_json::from_str::<Record>(line) {
                    Ok(rec) => {
                        if let Record::Branch(b) = &rec {
                            branch_owner.push((b.id.clone(), b.session.clone()));
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
                        index_unreadable_line(line, &sid, &mut branch_owner);
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
                    if let Record::Branch(b) = &rec {
                        branch_owner.push((b.id.clone(), b.session.clone()));
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
                    index_unreadable_line(line, &sid, &mut branch_owner);
                }
            }
        }

        self.branch_owner.extend(branch_owner);
        let key = sid.clone().unwrap_or(SessionId(stem));
        match (state, corrupt) {
            (Some(mut state), None) => {
                state.merge_main_lines();
                // Only the skeleton stays: the nodes are read again on use.
                state.bytes = std::fs::metadata(path).map_or(content.len() as u64, |m| m.len());
                state.unload();
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
    fn new(mut info: SessionInfo) -> Self {
        // A header written before `touched_at` existed carries none.
        info.touched_at = info.touched_at.max(info.created_at);
        Self {
            info,
            branches: HashMap::new(),
            tasks: HashMap::new(),
            blobs: HashSet::new(),
            nodes: Some(HashMap::new()),
            bytes: 0,
            node_gen: 0,
            used: 0,
            failed: None,
        }
    }

    /// Drop the nodes, remembering whether the main line ended in an error.
    fn unload(&mut self) {
        let Some(nodes) = self.nodes.take() else {
            return;
        };
        self.failed = self
            .branches
            .get(&self.info.main_branch)
            .map(|b| (b.id.clone(), b.head.clone(), line_failed(&nodes, &b.head)));
    }

    fn apply(&mut self, rec: Record) {
        match rec {
            Record::Session(info) => self.info = info,
            Record::Meta {
                title,
                active_branch,
                project,
                updated_at,
            } => {
                if let Some(v) = title {
                    self.info.title = v;
                }
                if let Some(v) = active_branch {
                    self.info.main_branch = v;
                }
                if let Some(v) = updated_at {
                    self.info.updated_at = self.info.updated_at.max(v);
                }
                if let Some(v) = project
                    && v != self.info.project
                {
                    self.info.project = v;
                    self.info.touched_at = self.info.touched_at.max(self.info.updated_at);
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
            Record::Node { branch, node } => {
                if let Some(b) = branch.and_then(|id| self.branches.get_mut(&id)) {
                    b.head = node.id.clone();
                    if matches!(b.kind, BranchKind::Main)
                        && matches!(node.body, NodeBody::User { .. })
                    {
                        self.info.touched_at = self.info.touched_at.max(node.at);
                    }
                }
                self.info.updated_at = self.info.updated_at.max(node.at);
                self.blobs.extend(blobs_of(&node));
                if let Some(nodes) = &mut self.nodes {
                    nodes.insert(node.id.clone(), *node);
                }
            }
            Record::Task(info) => {
                self.tasks.insert(info.id.clone(), *info);
            }
            Record::Unknown => {}
        }
    }
}

impl SessionState {
    /// Fold an older journal's main branches into one (see the crate docs):
    /// the session's main line stays, the other main branches go, and what
    /// named them names the main line. A no-op on a journal with one.
    fn merge_main_lines(&mut self) {
        let main = self.info.main_branch.clone();
        let gone: HashSet<BranchId> = self
            .branches
            .values()
            .filter(|b| matches!(b.kind, BranchKind::Main) && b.id != main)
            .map(|b| b.id.clone())
            .collect();
        if gone.is_empty() {
            return;
        }
        self.branches.retain(|id, _| !gone.contains(id));
        for b in self.branches.values_mut() {
            if let BranchKind::Agent { parent_branch, .. } | BranchKind::Side { parent_branch, .. } =
                &mut b.kind
                && gone.contains(parent_branch)
            {
                *parent_branch = main.clone();
            }
        }
        for t in self.tasks.values_mut() {
            if gone.contains(&t.branch) {
                t.branch = main.clone();
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

/// A line rumu cannot fold still reveals its ids: route branch lookups to
/// its session so `get_branch` fails loudly instead of returning `None` for
/// a branch that is right there on disk.
fn index_unreadable_line(
    line: &str,
    sid: &Option<SessionId>,
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
    if obj.get("t").and_then(Value::as_str) == Some("branch") {
        branch_owner.push((BranchId(id.to_owned()), owner));
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
    use rumu_core::{BranchKind, NodeBody, TaskInfo, now};

    fn mk_session(store: &Store) -> (SessionInfo, Branch, Node) {
        let root = Node {
            id: NodeId::new(),
            parent: None,
            session: SessionId::new(),
            at: now(),
            dur_ms: None,
            body: NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::SessionRoot::default()),
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
            main_branch: branch.id.clone(),
            created_at: root.at,
            updated_at: root.at,
            touched_at: root.at,
            seen_at: None,
            assistant: "assistant".into(),
            project: None,
            status: SessionStatus::Idle,
        };
        store.insert_session(&session).unwrap();
        store.insert_branch(&branch).unwrap();
        store.insert_node(&root).unwrap();
        (session, branch, root)
    }

    /// A task record: the latest one per id wins, and a task that was still
    /// running when the journal was last written comes back as it was left.
    #[test]
    fn task_records_fold_by_id_and_survive_a_reopen() {
        let dir = temp_dir("tasks");
        let (sid, branch, task) = {
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
            let (session, branch, _root) = mk_session(&store);
            let task = TaskInfo {
                id: TaskId::new(),
                session: session.id.clone(),
                branch: branch.id.clone(),
                call_id: "c1".into(),
                kind: rumu_core::TaskKind::Tool {
                    name: "bash".into(),
                    args_summary: "sleep 999".into(),
                },
                summary: "sleep 999".into(),
                status: rumu_core::TaskStatus::Running,
                started_at: now(),
                ended_at: None,
                truncated: false,
                is_error: false,
                result: Some(rumu_core::Content::text("not persisted")),
                data: None,
            };
            store.append_task(&task).unwrap();
            let mut ended = task.clone();
            ended.status = rumu_core::TaskStatus::Done;
            ended.ended_at = Some(now());
            store.append_task(&ended).unwrap();
            let second = TaskInfo {
                id: TaskId::new(),
                status: rumu_core::TaskStatus::Running,
                started_at: task.started_at + 1,
                ..task.clone()
            };
            store.append_task(&second).unwrap();

            let open = store.tasks_of(&session.id).unwrap();
            assert_eq!(open.len(), 2, "one row per id");
            assert_eq!(open[0].status, rumu_core::TaskStatus::Done);
            assert_eq!(open[1].status, rumu_core::TaskStatus::Running);
            (session.id, branch.id, open[1].clone())
        };

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let tasks = store.tasks_of(&sid).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].id, task.id);
        assert_eq!(tasks[1].branch, branch);
        assert_eq!(tasks[1].status, rumu_core::TaskStatus::Running);
        assert!(tasks[1].result.is_none(), "only the note carries a result");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The on-disk index covers what was written before a reopen; a session
    /// whose journal it does not match is re-indexed, and one whose journal
    /// is gone is dropped from it.
    #[test]
    fn the_search_index_follows_the_journals_across_reopens() {
        let dir = temp_dir("index");
        let index = dir.join("index");
        let open = || Store::open_indexed(&dir, &dir.join("seen.json"), &index).unwrap();
        let hits = |store: &Store, ids: &[SessionId]| {
            let found = store.search("hello", ids, 10).unwrap();
            found.hits.into_iter().map(|h| h.node).collect::<Vec<_>>()
        };
        let (a, b, hello) = {
            let store = open();
            let (a, a_main, a_root) = mk_session(&store);
            let (b, _, _) = mk_session(&store);
            let hello = note(&a, &a_root.id, rumu_core::Content::text("hello world"));
            store.append_node(&a_main.id, &hello).unwrap();
            assert_eq!(
                hits(&store, std::slice::from_ref(&a.id)),
                vec![hello.id.0.clone()]
            );
            (a, b, hello)
        };
        // Written behind the index's back: only a re-index finds it.
        let late = note(
            &b,
            &b.main_branch.0.as_str().into(),
            rumu_core::Content::text("hello"),
        );
        let line = serde_json::to_string(&Record::Node {
            branch: None,
            node: Box::new(late.clone()),
        })
        .unwrap();
        let path = dir.join(format!("{}.jsonl", b.id));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(format!("{line}\n").as_bytes()).unwrap();
        drop(file);
        std::fs::remove_file(dir.join(format!("{}.jsonl", a.id))).unwrap();
        let store = open();
        let ids = [a.id.clone(), b.id.clone()];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while hits(&store, &ids) != [late.id.0.clone()] {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                hits(&store, &ids)
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(store.indexed_nodes(&a.id).unwrap().is_empty());
        assert!(!store.indexed_nodes(&b.id).unwrap().contains(&hello.id.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rumu-store-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn in_memory(store: &Store, id: &SessionId) -> bool {
        match store.state.lock().unwrap().sessions.get(id) {
            Some(Entry::Loaded(s)) => s.nodes.is_some(),
            _ => false,
        }
    }

    fn note(session: &SessionInfo, parent: &NodeId, content: rumu_core::Content) -> Node {
        Node {
            id: NodeId::new(),
            parent: Some(parent.clone()),
            session: session.id.clone(),
            at: now(),
            dur_ms: None,
            body: NodeBody::Note {
                source: "system".into(),
                content,
            },
        }
    }

    /// A reopened store holds only skeletons; nodes are read on use, and an
    /// idle session's are dropped again once the budget is spent, unless
    /// the session is busy. Writes and reads stay correct either way.
    #[test]
    fn nodes_load_on_use_and_idle_ones_are_evicted() {
        let dir = temp_dir("evict");
        let (a, b, a_main, a_root) = {
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
            let (a, a_main, a_root) = mk_session(&store);
            let (b, _, b_root) = mk_session(&store);
            let failed = rumu_core::Content::marker(&TurnError {
                message: "boom".into(),
            });
            store
                .append_node(&a_main.id, &note(&a, &a_root.id, failed))
                .unwrap();
            store
                .append_node(
                    &b.main_branch,
                    &note(&b, &b_root.id, rumu_core::Content::text("b")),
                )
                .unwrap();
            (a, b, a_main, a_root)
        };

        let store = Store::open(&dir, &dir.join("seen.json"))
            .unwrap()
            .with_budget(0);
        assert!(!in_memory(&store, &a.id) && !in_memory(&store, &b.id));
        // Skeleton reads need no nodes.
        assert_eq!(store.list_sessions().unwrap().len(), 2);
        assert!(store.get_branch(&a_main.id).unwrap().is_some());
        assert!(store.line_failed(&a_main.id).unwrap());
        assert!(!in_memory(&store, &a.id));

        assert_eq!(store.nodes_of_session(&a.id).unwrap().len(), 2);
        assert!(in_memory(&store, &a.id));
        // Over budget: reading `b` drops idle `a` ...
        assert_eq!(store.nodes_of_session(&b.id).unwrap().len(), 2);
        assert!(in_memory(&store, &b.id) && !in_memory(&store, &a.id));
        // ... but not a busy one.
        store.set_status(&b.id, SessionStatus::Running);
        assert!(store.get_node(&a.id, &a_root.id).unwrap().is_some());
        assert!(in_memory(&store, &a.id) && in_memory(&store, &b.id));
        // Settling lets it go.
        store.set_status(&b.id, SessionStatus::Idle);
        assert!(!in_memory(&store, &a.id) || !in_memory(&store, &b.id));

        // Appending to a dropped session reads it first, so memory and the
        // journal agree; the line no longer ends in an error.
        let reply = note(&a, &a_main.head, rumu_core::Content::text("ok"));
        let reply = Node {
            parent: Some(store.get_branch(&a_main.id).unwrap().unwrap().head),
            body: NodeBody::User {
                content: rumu_core::Content::text("hi"),
            },
            ..reply
        };
        store.nodes_of_session(&b.id).unwrap();
        assert!(!in_memory(&store, &a.id));
        store.append_node(&a_main.id, &reply).unwrap();
        assert_eq!(store.nodes_of_session(&a.id).unwrap().len(), 3);
        assert!(!store.line_failed(&a_main.id).unwrap());
        drop(store);
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        assert!(store.get_node(&a.id, &reply.id).unwrap().is_some());
        assert!(!store.line_failed(&a_main.id).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seen_at_survives_a_reload() {
        let dir = temp_dir("seen");
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let (session, branch, root) = mk_session(&store);
        store.mark_seen(&session.id, 7).unwrap();
        let child = Node {
            id: NodeId::new(),
            parent: Some(root.id.clone()),
            session: session.id.clone(),
            at: root.at + 9,
            dur_ms: None,
            body: NodeBody::User {
                content: rumu_core::Content::text("hi"),
            },
        };
        store.append_node(&branch.id, &child).unwrap();
        drop(store);
        let info = Store::open(&dir, &dir.join("seen.json"))
            .unwrap()
            .get_session(&session.id)
            .unwrap()
            .unwrap();
        // `updated_at` follows the newest node.
        assert_eq!(info.updated_at, root.at + 9);
        assert_eq!(info.seen_at, Some(7));
        // The journal holds no view state.
        let journal = std::fs::read_to_string(dir.join(format!("{}.jsonl", session.id))).unwrap();
        assert!(!journal.contains("seen_at"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn touched_at_follows_prompts_and_moves_only() {
        let dir = temp_dir("touched");
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let (session, branch, root) = mk_session(&store);
        let side = Branch {
            id: BranchId::new(),
            kind: BranchKind::Side {
                parent_branch: branch.id.clone(),
                quote: None,
            },
            ..branch.clone()
        };
        store.insert_branch(&side).unwrap();
        let prompt = |on: &Branch, at| {
            let node = Node {
                id: NodeId::new(),
                parent: Some(root.id.clone()),
                session: session.id.clone(),
                at: root.at + at,
                dur_ms: None,
                body: NodeBody::User {
                    content: rumu_core::Content::text("hi"),
                },
            };
            store.append_node(&on.id, &node).unwrap();
        };
        prompt(&branch, 5);
        prompt(&side, 7);
        let mut info = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            (info.updated_at, info.touched_at),
            (root.at + 7, root.at + 5)
        );
        // A title is not a move; a new project is.
        info.title = "t".into();
        info.updated_at = root.at + 8;
        store.update_session(&info).unwrap();
        let touched = store.get_session(&session.id).unwrap().unwrap().touched_at;
        assert_eq!(touched, root.at + 5);
        info.project = Some("p".into());
        info.updated_at = root.at + 9;
        store.update_session(&info).unwrap();
        drop(store);
        let info = Store::open(&dir, &dir.join("seen.json"))
            .unwrap()
            .get_session(&session.id)
            .unwrap()
            .unwrap();
        assert_eq!(info.touched_at, root.at + 9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn appended_nodes_read_back() {
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
        store.append_node(&branch.id, &child).unwrap();

        let got = store.get_node(&session.id, &child.id).unwrap().unwrap();
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
    }

    #[test]
    fn delete_removes_the_journal_and_indexes() {
        let dir = temp_dir("delete");
        {
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
            let (session, branch, root) = mk_session(&store);
            let path = dir.join(format!("{}.jsonl", session.id));
            assert!(path.exists());

            store.delete_session(&session.id).unwrap();
            assert!(store.list_sessions().unwrap().is_empty());
            assert!(store.get_session(&session.id).unwrap().is_none());
            assert!(store.get_branch(&branch.id).unwrap().is_none());
            assert!(store.get_node(&session.id, &root.id).unwrap().is_none());
            assert!(!path.exists());

            // Idempotent, and a late write from a cancelled turn is dropped
            // instead of recreating the file.
            store.delete_session(&session.id).unwrap();
            store
                .insert_node(&Node {
                    id: NodeId::new(),
                    parent: Some(root.id.clone()),
                    session: session.id.clone(),
                    at: now(),
                    dur_ms: None,
                    body: NodeBody::Note {
                        source: "system".into(),
                        content: rumu_core::Content::text("late"),
                    },
                })
                .unwrap();
            assert!(!path.exists());
        }
        // Reopening the directory finds nothing left behind.
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        assert!(store.list_sessions().unwrap().is_empty());
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
                r#"{{"t":"session","id":"{sid}","title":"","tags":["infra"],"active_branch":"B1","created_at":1,"updated_at":1,"assistant":"A"}}"#
            )),
            line(&format!(
                r#"{{"t":"branch","id":"B1","session":"{sid}","head":"N0","forked_from":null,"kind":{{"type":"main"}},"status":"active","created_at":1}}"#
            )),
            line(&format!(
                r#"{{"t":"node","id":"N0","parent":null,"session":"{sid}","at":1,"body":{{"type":"note","source":"system","content":{{"text":"","data":{{"tag":"root","v":1,"data":{{"assistant":"A"}}}}}}}}}}"#
            )),
            line(&format!(
                r#"{{"t":"node","id":"N1","parent":"N0","session":"{sid}","at":2,"body":{{"type":"note","source":"system","content":{{"text":"","data":{{"tag":"tag","v":1,"data":{{"name":"infra"}}}}}}}}}}"#
            )),
            line(r#"{"t":"meta","title":"meta","tags":["infra"],"updated_at":3}"#),
        ]
        .concat();
        std::fs::write(dir.join(format!("{sid}.jsonl")), content).unwrap();

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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

    /// A journal from before the main line was one branch: the branch it was
    /// last switched to is the main line, the other main branch goes (its
    /// node stays, a variant), and the sub-agent forked on it reports to the
    /// main line. A head moved afterwards survives a reopen.
    #[test]
    fn older_main_branches_fold_into_the_main_line() {
        let dir = temp_dir("main-lines");
        let sid = "01MAINLINES0000000000000000";
        let node = |id: &str, parent: &str, at: u32| {
            format!(
                r#"{{"t":"node","id":"{id}","parent":"{parent}","session":"{sid}","at":{at},"body":{{"type":"user","content":{{"text":"{id}"}}}}}}"#
            )
        };
        let content = [
            format!(
                r#"{{"t":"session","id":"{sid}","title":"","active_branch":"B1","created_at":1,"updated_at":1,"assistant":"A"}}"#
            ),
            format!(
                r#"{{"t":"branch","id":"B1","session":"{sid}","head":"N0","forked_from":null,"kind":{{"type":"main"}},"status":"active","created_at":1}}"#
            ),
            format!(
                r#"{{"t":"node","id":"N0","parent":null,"session":"{sid}","at":1,"body":{{"type":"note","source":"system","content":{{"text":"","data":{{"tag":"root","v":1,"data":{{"assistant":"A"}}}}}}}}}}"#
            ),
            node("N1", "N0", 2).replacen(r#"{"t":"node","#, r#"{"t":"node","branch":"B1","#, 1),
            format!(
                r#"{{"t":"branch","id":"G","session":"{sid}","head":"N1","forked_from":"N1","kind":{{"type":"agent","task":"t","parent_branch":"B1","call_id":"c"}},"status":"done","created_at":3}}"#
            ),
            format!(
                r#"{{"t":"branch","id":"B2","session":"{sid}","head":"N0","forked_from":"N0","kind":{{"type":"main"}},"status":"active","created_at":4}}"#
            ),
            node("N2", "N0", 5).replacen(r#"{"t":"node","#, r#"{"t":"node","branch":"B2","#, 1),
            r#"{"t":"meta","active_branch":"B2","updated_at":5}"#.to_owned(),
        ]
        .map(|l| l + "\n")
        .concat();
        std::fs::write(dir.join(format!("{sid}.jsonl")), content).unwrap();
        let sid = SessionId::from(sid);

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let session = store.get_session(&sid).unwrap().expect("session loads");
        assert_eq!(session.main_branch, BranchId::from("B2"));
        let branches = store.branches_of_session(&sid).unwrap();
        let ids: Vec<&str> = branches.iter().map(|b| b.id.0.as_str()).collect();
        assert_eq!(ids, ["G", "B2"]);
        assert!(matches!(&branches[0].kind,
            BranchKind::Agent { parent_branch, .. } if parent_branch.0 == "B2"));
        assert_eq!(branches[1].head, NodeId::from("N2"));
        assert_eq!(store.nodes_of_session(&sid).unwrap().len(), 3);

        store
            .set_branch_head(&BranchId::from("B2"), &NodeId::from("N1"))
            .unwrap();
        drop(store);
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let main = store.get_branch(&BranchId::from("B2")).unwrap().unwrap();
        assert_eq!(main.head, NodeId::from("N1"));
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
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let sid = SessionId(sid);
        let err = store.get_node(&sid, &NodeId("ghost".into())).unwrap_err();
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

    /// A retired record tag (the old internal-call side table) is ignored on
    /// load instead of making the session unreadable.
    #[test]
    fn retired_code_call_records_are_ignored() {
        let dir = temp_dir("retired-code-call");
        let sid;
        {
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
            let (session, _, _) = mk_session(&store);
            sid = session.id.0.clone();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(dir.join(format!("{sid}.jsonl")))
                .unwrap();
            file.write_all(
                br#"{"t":"code_call","branch":"b","parent_call":"c1","seq":1,"name":"read","args":{},"output":"x","is_error":false,"blocked":null,"dur_ms":0,"at":1}
"#,
            )
            .unwrap();
        }
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let session = store
            .get_session(&SessionId::from(sid.as_str()))
            .unwrap()
            .expect("old journals still load");
        assert_eq!(store.nodes_of_session(&session.id).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn attachment(hash: &str) -> rumu_core::Attachment {
        rumu_core::Attachment {
            kind: rumu_core::AttachmentKind::Image,
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
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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
                        stats: Default::default(),
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
                            .with_attachments([attachment("cc")])
                            .with_data(&Artifact {
                                path: "@scratch:a.html".into(),
                                title: "a.html".into(),
                                kind: rumu_core::ArtifactKind::Html,
                                hash: "ff".into(),
                                size: 1,
                                mime: None,
                            }),
                        is_error: false,
                        invoke: None,
                    },
                ))
                .unwrap();
            store
                .insert_node(&node(
                    &session.id,
                    &root.id,
                    NodeBody::ToolResult {
                        call_id: "c2".into(),
                        content: rumu_core::Content::text("app").with_data(&McpApp {
                            server: "s".into(),
                            tool: "t".into(),
                            uri: "ui://s/t".into(),
                            html: "gg".into(),
                            call: "hh".into(),
                            csp: Default::default(),
                            prefers_border: None,
                        }),
                        is_error: false,
                        invoke: None,
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
                ["aa", "bb", "cc", "dd", "ee", "ff", "gg", "hh"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<HashSet<_>>()
            );
            session.id
        };

        // The same set comes back from a reopened (re-scanned) store.
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        assert_eq!(store.referenced_blobs().unwrap().len(), 8);
        assert!(store.get_session(&sid).unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A crash mid-append leaves a torn last line: it is dropped with a
    /// warning and truncated, and later appends land cleanly.
    #[test]
    fn torn_last_line_is_tolerated_and_truncated() {
        let dir = temp_dir("torn");
        let (sid, root_id) = {
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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
        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        assert_eq!(store.nodes_of_session(&sid).unwrap().len(), 2);
        assert!(store.get_node(&sid, &child.id).unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A newline-terminated line is never a torn write (each append is one
    /// `write_all` of line + '\n'): garbage there is real corruption. The
    /// session errors, and the file is left untouched.
    #[test]
    fn broken_complete_line_is_corruption_not_torn() {
        let dir = temp_dir("broken");
        let sid = {
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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
            let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
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
            store.append_node(&branch.id, &child).unwrap();
            store
                .set_branch_status(&branch.id, BranchStatus::Done)
                .unwrap();

            let mut s = session.clone();
            s.title = "t1".into();
            s.project = Some("proj-deck".into());
            store.update_session(&s).unwrap();
            // A sparse clock patch must not clear the project.
            store.mark_seen(&session.id, 78).unwrap();
            (session.id, branch.id, child.id)
        };

        let store = Store::open(&dir, &dir.join("seen.json")).unwrap();
        let b = store.get_branch(&branch_id).unwrap().unwrap();
        assert_eq!(b.head, child_id);
        assert_eq!(b.status, BranchStatus::Done);

        let s = store.get_session(&sid).unwrap().unwrap();
        assert_eq!(s.title, "t1");
        assert_eq!(s.project.as_deref(), Some("proj-deck"));

        // An update may also clear it (persisted as an explicit null).
        let mut cleared = s.clone();
        cleared.project = None;
        store.update_session(&cleared).unwrap();
        assert_eq!(store.get_session(&sid).unwrap().unwrap().project, None);

        // Id-only lookups work off the indexes built during the scan.
        assert!(store.get_node(&sid, &child_id).unwrap().is_some());
        assert!(store.get_branch(&branch_id).unwrap().is_some());
        assert_eq!(store.nodes_of_session(&sid).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
