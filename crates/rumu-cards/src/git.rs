//! Git access behind deck history, using `gix` (gitoxide) instead of the git
//! binary.
//!
//! A deck is a bare repository: its content is the `HEAD` tree, so reading a
//! card is a tree lookup and writing one is a commit. Commits are built
//! structurally — the new tree is the `HEAD` tree edited with exactly the
//! paths the operation touched — and there is no worktree or index to keep in
//! step.
//!
//! Reading goes through `gix`. The objects a commit creates are serialized by
//! `gix` too, but written to disk here: gix renames a loose object file into
//! place while it is still open, and a filesystem that refuses to move an open
//! file to another directory (yrfs, behind `/home` on the training cluster,
//! answers `EBUSY`) would fail every commit.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use gix::objs::WriteTo;
use gix::objs::tree::{EntryKind, EntryMode};
use gix::refs::transaction::PreviousValue;

use crate::{Error, Result};

/// The identity recorded on a commit.
#[derive(Debug, Clone)]
pub struct GitAuthor {
    pub name: String,
    pub email: String,
}

/// One parsed commit.
#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    /// Author display name (`user` or a deck name).
    pub author: String,
    pub email: String,
    /// Unix seconds of the committer time.
    pub at: i64,
    pub subject: String,
    pub body: String,
    /// Card paths (deck-local, no `.md`) changed relative to the parent.
    pub changed: Vec<String>,
    pub session: Option<String>,
    pub node: Option<String>,
}

impl Commit {
    /// The operation and path encoded in the subject (`update notes/storage`,
    /// `move a -> b`, `delete c`).
    pub fn op_path(&self) -> (String, String) {
        let mut parts = self.subject.splitn(2, ' ');
        let op = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().trim().to_owned();
        (op, path)
    }

    /// The author's identity (`user` or a deck id), from the commit email.
    pub fn author_id(&self) -> &str {
        self.email.strip_suffix(EMAIL_DOMAIN).unwrap_or(&self.email)
    }
}

/// The domain of a rumu commit email: `<user or deck id>@rumu`.
pub const EMAIL_DOMAIN: &str = "@rumu";

/// Trailers attached to a rumu commit.
pub struct Trailers<'a> {
    /// Files to replace in the tree; `None` removes the path.
    pub changes: &'a BTreeMap<String, Option<String>>,
    pub session: Option<&'a str>,
    pub node: Option<&'a str>,
}

/// The git repository at one deck directory.
#[derive(Debug, Clone)]
pub struct Repo {
    dir: PathBuf,
}

impl Repo {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Create the bare repository if the directory does not hold one yet.
    pub fn init(&self) -> Result<()> {
        if self.dir.join("HEAD").is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(rumu_core::IoError::at("creating", &self.dir))?;
        gix::init_bare(&self.dir).map_err(|e| error("init", &self.dir, e))?;
        Ok(())
    }

    /// The `HEAD` commit id, if the repository has one.
    pub fn head(&self) -> Result<Option<String>> {
        let repo = self.open()?;
        let head = repo.head().map_err(|e| error("head", &self.dir, e))?;
        Ok(head.id().map(|id| id.to_string()))
    }

    /// [`head`](Self::head) read straight from the ref files when they are
    /// plain (`HEAD` → a loose or packed branch ref), without opening the
    /// repository: cheap enough to check on every recall.
    pub fn head_id(&self) -> Result<Option<String>> {
        match self.head_from_files() {
            Some(id) => Ok(Some(id)),
            None => self.head(),
        }
    }

    fn head_from_files(&self) -> Option<String> {
        let object_id = |s: &str| {
            (matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| s.to_ascii_lowercase())
        };
        let head = std::fs::read_to_string(self.dir.join("HEAD")).ok()?;
        let Some(name) = head.trim().strip_prefix("ref: ") else {
            return object_id(head.trim());
        };
        let name = name.trim();
        if !name.starts_with("refs/") || name.split('/').any(|s| s.is_empty() || s == "..") {
            return None;
        }
        if let Ok(id) = std::fs::read_to_string(self.dir.join(name)) {
            return object_id(id.trim());
        }
        let packed = std::fs::read_to_string(self.dir.join("packed-refs")).ok()?;
        packed.lines().find_map(|line| {
            let (id, n) = line.split_once(' ')?;
            (n.trim() == name).then(|| object_id(id)).flatten()
        })
    }

    /// Every blob in the `HEAD` tree: path → blob id.
    pub fn files(&self) -> Result<BTreeMap<String, String>> {
        let repo = self.open()?;
        let mut head = repo.head().map_err(|e| error("head", &self.dir, e))?;
        if head.is_unborn() {
            return Ok(BTreeMap::new());
        }
        let commit = head
            .peel_to_commit()
            .map_err(|e| error("head commit", &self.dir, e))?;
        let tree = commit
            .tree()
            .map_err(|e| error("head tree", &self.dir, e))?;
        tree_files(&tree).map_err(|e| error("tree walk", &self.dir, e))
    }

    /// The `HEAD` commit and the blobs of its tree (path → blob id), read
    /// together so they always agree.
    pub fn head_files(&self) -> Result<(Option<String>, BTreeMap<String, String>)> {
        let repo = self.open()?;
        let mut head = repo.head().map_err(|e| error("head", &self.dir, e))?;
        let Some(id) = head.id().map(|id| id.to_string()) else {
            return Ok((None, BTreeMap::new()));
        };
        let commit = head
            .peel_to_commit()
            .map_err(|e| error("head commit", &self.dir, e))?;
        let tree = commit
            .tree()
            .map_err(|e| error("head tree", &self.dir, e))?;
        let files = tree_files(&tree).map_err(|e| error("tree walk", &self.dir, e))?;
        Ok((Some(id), files))
    }

    /// The text of each blob in `ids`, in order.
    pub fn blobs(&self, ids: &[&str]) -> Result<Vec<Result<String>>> {
        let repo = self.open()?;
        Ok(ids
            .iter()
            .map(|id| {
                let oid = gix::hash::ObjectId::from_hex(id.as_bytes())
                    .map_err(|e| error("blob id", &self.dir, e))?;
                let object = repo
                    .find_object(oid)
                    .map_err(|e| error("read object", &self.dir, e))?;
                Ok(String::from_utf8_lossy(&object.data).into_owned())
            })
            .collect())
    }

    /// The text of `path` in the `HEAD` tree, if it exists there.
    pub fn read(&self, path: &str) -> Result<Option<String>> {
        self.show("HEAD", path)
    }

    /// Commit exactly the given changes on top of `HEAD`. Returns whether a
    /// commit was made.
    pub fn commit(&self, author: &GitAuthor, subject: &str, trailers: &Trailers) -> Result<bool> {
        let repo = self.open()?;
        let mut head = repo.head().map_err(|e| error("head", &self.dir, e))?;
        let mut node = Node::default();
        let (parent, base) = match head.id() {
            Some(id) => {
                let commit = head
                    .peel_to_commit()
                    .map_err(|e| error("head commit", &self.dir, e))?;
                let tree = commit
                    .tree_id()
                    .map_err(|e| error("head tree", &self.dir, e))?
                    .detach();
                self.read_tree(&repo, tree, &mut node)?;
                (Some(id.detach()), tree)
            }
            None => (None, repo.empty_tree().id().detach()),
        };
        for (path, content) in trailers.changes {
            match content {
                Some(text) => {
                    let blob = self.write_object(gix::objs::Blob {
                        data: text.as_bytes().to_vec(),
                    })?;
                    node.insert(path, EntryMode::from(EntryKind::Blob), blob);
                }
                None => {
                    node.remove(path);
                }
            }
        }
        let tree = match self.write_tree(&node)? {
            Some(tree) => tree,
            None => repo.empty_tree().id().detach(),
        };
        if tree == base {
            return Ok(false);
        }
        let signature = gix::actor::Signature {
            name: author.name.clone().into(),
            email: author.email.clone().into(),
            time: gix::date::Time::now_local_or_utc(),
        };
        let mut commit = gix::objs::Commit {
            tree,
            parents: Default::default(),
            author: signature.clone(),
            committer: signature,
            encoding: None,
            message: commit_message(subject, trailers).into(),
            extra_headers: Default::default(),
        };
        if let Some(parent) = parent {
            commit.parents.push(parent);
        }
        let commit = self.write_object(commit)?;
        let Some(name) = repo
            .head_name()
            .map_err(|e| error("head name", &self.dir, e))?
        else {
            return Err(Error::Git {
                message: format!("HEAD is detached in {}", self.dir.display()),
            });
        };
        // Only over the `HEAD` this commit was built on: a deck moved by
        // anyone else meanwhile fails the commit instead of losing theirs.
        let expected = match parent {
            Some(parent) => PreviousValue::ExistingMustMatch(gix::refs::Target::Object(parent)),
            None => PreviousValue::MustNotExist,
        };
        repo.reference(name.as_bstr().to_string(), commit, expected, "commit")
            .map_err(|e| error("update head", &self.dir, e))?;
        Ok(true)
    }

    /// The repository's commits, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let repo = self.open()?;
        let mut head = match repo.head() {
            Ok(head) if !head.is_unborn() => head,
            _ => return Ok(Vec::new()),
        };
        let commit = head
            .peel_to_commit()
            .map_err(|e| error("head commit", &self.dir, e))?;
        let walk = commit
            .ancestors()
            .all()
            .map_err(|e| error("rev walk", &self.dir, e))?;
        let mut out = Vec::new();
        for info in walk.take(limit) {
            let info = info.map_err(|e| error("rev walk", &self.dir, e))?;
            let object = info
                .object()
                .map_err(|e| error("read commit", &self.dir, e))?;
            out.push(parse_commit(&repo, &object)?);
        }
        Ok(out)
    }

    /// Commits that change the file of card `path` (compared with the parent
    /// tree), newest first. `since` is an exclusive commit boundary.
    pub fn log_touching(&self, path: &str, since: Option<&str>) -> Result<Vec<Commit>> {
        let repo = self.open()?;
        let mut head = match repo.head() {
            Ok(head) if !head.is_unborn() => head,
            _ => return Ok(Vec::new()),
        };
        let commit = head
            .peel_to_commit()
            .map_err(|e| error("head commit", &self.dir, e))?;
        let walk = commit
            .ancestors()
            .all()
            .map_err(|e| error("rev walk", &self.dir, e))?;
        let mut out = Vec::new();
        for info in walk {
            let info = info.map_err(|e| error("rev walk", &self.dir, e))?;
            if since == Some(info.id().to_string().as_str()) {
                break;
            }
            let object = info
                .object()
                .map_err(|e| error("read commit", &self.dir, e))?;
            let parsed = parse_commit(&repo, &object)?;
            if parsed.changed.iter().any(|c| c == path) {
                out.push(parsed);
            }
        }
        Ok(out)
    }

    /// The newest commit that changed card `path`, if any.
    pub fn log_path(&self, path: &str) -> Result<Option<Commit>> {
        let repo = self.open()?;
        let mut head = match repo.head() {
            Ok(head) if !head.is_unborn() => head,
            _ => return Ok(None),
        };
        let commit = head
            .peel_to_commit()
            .map_err(|e| error("head commit", &self.dir, e))?;
        let walk = commit
            .ancestors()
            .all()
            .map_err(|e| error("rev walk", &self.dir, e))?;
        for info in walk {
            let info = info.map_err(|e| error("rev walk", &self.dir, e))?;
            let object = info
                .object()
                .map_err(|e| error("read commit", &self.dir, e))?;
            let parsed = parse_commit(&repo, &object)?;
            if parsed.changed.iter().any(|c| c == path) {
                return Ok(Some(parsed));
            }
        }
        Ok(None)
    }

    /// The full text of `path` at `commit`, if it exists there.
    pub fn show(&self, commit: &str, path: &str) -> Result<Option<String>> {
        let repo = self.open()?;
        let id = if commit == "HEAD" {
            let head = repo.head().map_err(|e| error("head", &self.dir, e))?;
            match head.id() {
                Some(id) => id.detach(),
                None => return Ok(None),
            }
        } else {
            match gix::hash::ObjectId::from_hex(commit.as_bytes()) {
                Ok(id) => id,
                Err(_) => return Ok(None),
            }
        };
        let commit = repo
            .find_commit(id)
            .map_err(|e| error("find commit", &self.dir, e))?;
        let tree = commit
            .tree()
            .map_err(|e| error("commit tree", &self.dir, e))?;
        let Some(entry) = tree
            .lookup_entry_by_path(path)
            .map_err(|e| error("lookup path", &self.dir, e))?
        else {
            return Ok(None);
        };
        let object = entry
            .object()
            .map_err(|e| error("read object", &self.dir, e))?;
        Ok(Some(String::from_utf8_lossy(&object.data).into_owned()))
    }

    /// Write one loose object into the deck repository and return its id.
    ///
    /// The temporary file is closed before it is renamed into `objects/<shard>/`,
    /// so the write works on filesystems that refuse to move an open file to
    /// another directory.
    fn write_object(&self, object: impl WriteTo) -> Result<gix::hash::ObjectId> {
        let kind = object.kind();
        let context = format!("{kind} object");
        let mut data = Vec::new();
        object
            .write_to(&mut data)
            .map_err(|e| error(&format!("serializing {context}"), &self.dir, e))?;
        let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, &data)
            .map_err(|e| error(&format!("hashing {context}"), &self.dir, e))?;
        let hex = id.to_string();
        let (shard, name) = hex.split_at(2);
        let dir = self.dir.join("objects").join(shard);
        let path = dir.join(name);
        if path.is_file() {
            return Ok(id);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| error("creating object directory", &self.dir, e))?;
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(&gix::objs::encode::loose_header(kind, data.len() as u64))
            .map_err(|e| error(&format!("compressing {context}"), &self.dir, e))?;
        zlib.write_all(&data)
            .map_err(|e| error(&format!("compressing {context}"), &self.dir, e))?;
        let compressed = zlib
            .finish()
            .map_err(|e| error(&format!("compressing {context}"), &self.dir, e))?;
        let mut file = tempfile::Builder::new()
            .prefix("obj-")
            .tempfile_in(&dir)
            .map_err(|e| error(&format!("creating {context} file"), &self.dir, e))?;
        file.write_all(&compressed)
            .map_err(|e| error(&format!("writing {context}"), &self.dir, e))?;
        let file = file.into_temp_path();
        std::fs::rename(&file, &path)
            .map_err(|e| error(&format!("renaming {context}"), &self.dir, e))?;
        Ok(id)
    }

    /// Read every file of `tree` into `node`, keyed by its deck-local path.
    fn read_tree(
        &self,
        repo: &gix::Repository,
        tree: gix::hash::ObjectId,
        node: &mut Node,
    ) -> Result<()> {
        let tree = repo
            .find_tree(tree)
            .map_err(|e| error("read tree", &self.dir, e))?;
        for entry in tree.iter() {
            let entry = entry
                .map_err(|e| error("read tree entry", &self.dir, e))?
                .inner;
            let name = entry.filename.to_string();
            if entry.mode.is_tree() {
                self.read_tree(
                    repo,
                    entry.oid.to_owned(),
                    node.dirs.entry(name).or_default(),
                )?;
            } else {
                node.files.insert(name, (entry.mode, entry.oid.to_owned()));
            }
        }
        Ok(())
    }

    /// Write every tree `node` holds, bottom-up, and return the root tree's id.
    /// `None` for a directory without files, which git has no object for.
    fn write_tree(&self, node: &Node) -> Result<Option<gix::hash::ObjectId>> {
        let mut entries = Vec::with_capacity(node.files.len() + node.dirs.len());
        for (name, (mode, oid)) in &node.files {
            entries.push(gix::objs::tree::Entry {
                mode: *mode,
                filename: name.as_str().into(),
                oid: *oid,
            });
        }
        for (name, dir) in &node.dirs {
            if let Some(oid) = self.write_tree(dir)? {
                entries.push(gix::objs::tree::Entry {
                    mode: EntryMode::from(EntryKind::Tree),
                    filename: name.as_str().into(),
                    oid,
                });
            }
        }
        if entries.is_empty() {
            return Ok(None);
        }
        entries.sort();
        Ok(Some(self.write_object(gix::objs::Tree { entries })?))
    }

    fn open(&self) -> Result<gix::Repository> {
        gix::open(&self.dir).map_err(|e| error("open", &self.dir, e))
    }
}

/// The files of a deck tree, grouped by directory. Writing a commit edits this
/// instead of a git index.
#[derive(Default)]
struct Node {
    /// The files directly in this directory: name, mode and blob id.
    files: BTreeMap<String, (EntryMode, gix::hash::ObjectId)>,
    dirs: BTreeMap<String, Node>,
}

impl Node {
    fn insert(&mut self, path: &str, mode: EntryMode, oid: gix::hash::ObjectId) {
        match path.split_once('/') {
            Some((dir, rest)) => self
                .dirs
                .entry(dir.to_owned())
                .or_default()
                .insert(rest, mode, oid),
            None => {
                self.files.insert(path.to_owned(), (mode, oid));
            }
        }
    }

    fn remove(&mut self, path: &str) {
        match path.split_once('/') {
            Some((dir, rest)) => {
                let Some(node) = self.dirs.get_mut(dir) else {
                    return;
                };
                node.remove(rest);
                if node.files.is_empty() && node.dirs.is_empty() {
                    self.dirs.remove(dir);
                }
            }
            None => {
                self.files.remove(path);
            }
        }
    }
}

/// Parse one commit: subject, body, author, trailers and the card paths it
/// changed. `at` is the committer time.
fn parse_commit(repo: &gix::Repository, commit: &gix::Commit<'_>) -> Result<Commit> {
    let message = commit
        .message_raw()
        .map_err(|e| Error::Git {
            message: format!("reading commit message: {e}"),
        })?
        .to_string();
    let author = commit.author().map_err(|e| Error::Git {
        message: format!("reading commit author: {e}"),
    })?;
    let parent = commit.time().map_err(|e| Error::Git {
        message: format!("reading commit time: {e}"),
    })?;
    let hash = commit.id().to_string();
    let subject = message.lines().next().unwrap_or_default().to_owned();
    let (session, node) = parse_trailers(&message);
    Ok(Commit {
        short: hash.chars().take(7).collect(),
        hash,
        author: author.name.to_string(),
        email: author.email.to_string(),
        at: parent.seconds,
        subject,
        body: message,
        changed: changed_cards(repo, commit)?,
        session,
        node,
    })
}

/// The card paths (deck-local, no `.md`) whose blob differs between `commit`
/// and its parent. A root commit changes everything it contains.
fn changed_cards(repo: &gix::Repository, commit: &gix::Commit<'_>) -> Result<Vec<String>> {
    let current = tree_files(&commit.tree().map_err(|e| Error::Git {
        message: format!("reading commit tree: {e}"),
    })?)?;
    let parent = match commit.parent_ids().next() {
        Some(id) => {
            let parent = repo.find_commit(id).map_err(|e| Error::Git {
                message: format!("reading parent commit: {e}"),
            })?;
            tree_files(&parent.tree().map_err(|e| Error::Git {
                message: format!("reading parent tree: {e}"),
            })?)?
        }
        None => BTreeMap::new(),
    };
    let mut changed = Vec::new();
    for (path, oid) in &current {
        if parent.get(path) != Some(oid)
            && let Some(card) = path.strip_suffix(".md")
        {
            changed.push(card.to_owned());
        }
    }
    for path in parent.keys() {
        if !current.contains_key(path)
            && let Some(card) = path.strip_suffix(".md")
        {
            changed.push(card.to_owned());
        }
    }
    changed.sort();
    changed.dedup();
    Ok(changed)
}

/// Every blob in a tree: path → blob id.
fn tree_files(tree: &gix::Tree<'_>) -> Result<BTreeMap<String, String>> {
    let files = tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(|e| Error::Git {
            message: format!("tree walk: {e}"),
        })?;
    let mut map = BTreeMap::new();
    for entry in files {
        if entry.mode.is_blob() {
            map.insert(entry.filepath.to_string(), entry.oid.to_string());
        }
    }
    Ok(map)
}

type ParsedTrailers = (Option<String>, Option<String>);

fn parse_trailers(body: &str) -> ParsedTrailers {
    let mut session = None;
    let mut node = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Session: ") {
            session = Some(v.trim().to_owned());
        } else if let Some(v) = line.strip_prefix("Node: ") {
            node = Some(v.trim().to_owned());
        }
    }
    (session, node)
}

fn commit_message(subject: &str, trailers: &Trailers) -> String {
    let mut message = subject.to_owned();
    message.push_str("\n\n");
    if let Some(session) = trailers.session {
        message.push_str(&format!("Session: {session}\n"));
    }
    if let Some(node) = trailers.node {
        message.push_str(&format!("Node: {node}\n"));
    }
    message
}

fn error(context: &str, dir: &Path, source: impl std::fmt::Display) -> Error {
    Error::Git {
        message: format!("{context} in {}: {source}", dir.display()),
    }
}

/// The repository for a deck directory.
pub fn repo(dir: &Path) -> Repo {
    Repo::new(dir)
}
