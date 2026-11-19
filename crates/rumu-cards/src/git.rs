//! Git access behind deck history, using `gix` (gitoxide) instead of the git
//! binary.
//!
//! A deck is a bare repository: its content is the `HEAD` tree, so reading a
//! card is a tree lookup and writing one is a commit. Commits are built
//! structurally — the new tree is the `HEAD` tree edited with exactly the
//! paths the operation touched — and there is no worktree or index to keep in
//! step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gix::objs::tree::EntryKind;

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

    /// The text of `path` in the `HEAD` tree, if it exists there.
    pub fn read(&self, path: &str) -> Result<Option<String>> {
        self.show("HEAD", path)
    }

    /// Commit exactly the given changes on top of `HEAD`. Returns whether a
    /// commit was made.
    pub fn commit(&self, author: &GitAuthor, subject: &str, trailers: &Trailers) -> Result<bool> {
        let repo = self.open()?;
        let mut head = repo.head().map_err(|e| error("head", &self.dir, e))?;
        let (parent, base) = if head.is_unborn() {
            (None, repo.empty_tree().id())
        } else {
            let commit = head
                .peel_to_commit()
                .map_err(|e| error("head commit", &self.dir, e))?;
            (
                Some(commit.id().detach()),
                commit
                    .tree_id()
                    .map_err(|e| error("head tree", &self.dir, e))?,
            )
        };
        let mut editor = repo
            .edit_tree(base.detach())
            .map_err(|e| error("edit tree", &self.dir, e))?;
        for (path, content) in trailers.changes {
            match content {
                Some(text) => {
                    let blob = repo
                        .write_blob(text.as_bytes())
                        .map_err(|e| error("write blob", &self.dir, e))?;
                    editor
                        .upsert(path.as_str(), EntryKind::Blob, blob.detach())
                        .map_err(|e| error("tree upsert", &self.dir, e))?;
                }
                None => {
                    editor
                        .remove(path.as_str())
                        .map_err(|e| error("tree remove", &self.dir, e))?;
                }
            }
        }
        let tree = editor
            .write()
            .map_err(|e| error("write tree", &self.dir, e))?
            .detach();
        if tree == base.detach() {
            return Ok(false);
        }
        let message = commit_message(subject, trailers);
        let signature = gix::actor::Signature {
            name: author.name.clone().into(),
            email: author.email.clone().into(),
            time: gix::date::Time::now_local_or_utc(),
        };
        let mut time_buf = gix::date::parse::TimeBuf::default();
        let signature = signature.to_ref(&mut time_buf);
        repo.commit_as(signature, signature, "HEAD", message, tree, parent)
            .map_err(|e| error("commit", &self.dir, e))?;
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

    fn open(&self) -> Result<gix::Repository> {
        gix::open(&self.dir).map_err(|e| error("open", &self.dir, e))
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
