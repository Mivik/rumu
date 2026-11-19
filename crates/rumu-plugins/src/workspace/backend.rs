//! Workspace backends: the four primitives (read bytes, write bytes, list
//! dir, exec) every workspace kind is reduced to. Two real backends exist —
//! [`LocalBackend`] (tokio fs + process) and [`SshBackend`] (shells out to
//! the system `ssh` binary with connection multiplexing, no extra crates).
//! The file primitives take **machine-absolute paths** (normalized by
//! `rumu-core::addr`); `exec` still starts in the workspace root.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessSession;
use process_wrap::tokio::{ChildWrapper, CommandWrap};
use rumu_core::{IoError, Utf8TypedPathBuf, Utf8UnixPath};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// A workspace backend failure.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// A local filesystem or process operation failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// A remote command exited with an error.
    #[error("ssh {op} {path} failed (exit {code}): {stderr}")]
    Remote {
        op: &'static str,
        path: String,
        code: i32,
        stderr: String,
    },

    /// A remote path is missing.
    #[error("ssh {op} {path}: no such file or directory")]
    NotFound { op: &'static str, path: String },

    /// The ssh writer could not be spawned.
    #[error("spawning ssh write: {source}")]
    SpawnWrite {
        #[source]
        source: std::io::Error,
    },

    /// The ssh process itself failed.
    #[error("ssh {host} `{remote_cmd}`: {source}")]
    Ssh {
        host: String,
        remote_cmd: String,
        #[source]
        source: std::io::Error,
    },

    /// A non-POSIX path reached the ssh backend (a workspace-style bug).
    #[error("ssh paths must be POSIX: {0}")]
    NotPosix(String),
}

pub(crate) type Result<T> = std::result::Result<T, BackendError>;

/// The result of an exec: combined stdout+stderr and the exit code.
pub(crate) struct ExecOutput {
    pub combined: String,
    pub code: i32,
}

#[async_trait]
pub(crate) trait Backend: Send + Sync {
    /// Read a file at an absolute path on the workspace's machine.
    async fn read_bytes(&self, abs: &Utf8TypedPathBuf) -> Result<Vec<u8>>;
    /// Create/overwrite a file. Its parent directory must exist: a missing one
    /// is [`is_not_found`], never created.
    async fn write_bytes(&self, abs: &Utf8TypedPathBuf, bytes: &[u8]) -> Result<()>;
    /// Delete a file; a missing one is [`is_not_found`].
    async fn remove_file(&self, abs: &Utf8TypedPathBuf) -> Result<()>;
    /// `(name, is_dir)` entries of a directory, sorted by name.
    async fn list_dir(&self, abs: &Utf8TypedPathBuf) -> Result<Vec<(String, bool)>>;
    /// Run a bash script in the workspace root. Merging stderr into stdout is
    /// the backend's job (the `exec 2>&1` trick); timeouts are enforced by
    /// the caller. Output chunks are passed to `on_chunk` as they arrive (for
    /// live `bash` progress); the child and its descendants are killed if the
    /// future is dropped (see [`ProcessTree`]).
    async fn exec(
        &self,
        script: &str,
        on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ExecOutput>;
}

/// How long output is still read after the shell exits: a process it left
/// running (`cmd &`, a dev server) can hold the pipe open forever.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(1);

/// Read one child pipe to EOF, forwarding chunks to the aggregator. Once the
/// aggregator has stopped listening, the rest is read and discarded, so a
/// process still writing to the pipe does not die of `SIGPIPE`.
async fn read_pipe<R>(
    mut reader: R,
    is_stderr: bool,
    tx: tokio::sync::mpsc::UnboundedSender<(bool, Vec<u8>)>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = tx.send((is_stderr, buf[..n].to_vec()));
            }
        }
    }
}

/// Decodes a byte stream chunk by chunk without splitting a multi-byte
/// character across two chunks (which would turn both halves into U+FFFD).
#[derive(Default)]
struct Utf8Chunks {
    pending: Vec<u8>,
}

impl Utf8Chunks {
    /// The text of the held-back bytes plus `bytes`, minus an incomplete
    /// character at the end, which waits for the next chunk.
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let keep = incomplete_tail(&self.pending);
        let tail = self.pending.split_off(self.pending.len() - keep);
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending = tail;
        text
    }

    /// Whatever is still held back (a truncated character at EOF).
    fn finish(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        text
    }
}

/// How many trailing bytes form the start of a character that is not
/// complete yet (0 when the buffer ends on a character boundary).
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(4) {
        let b = bytes[bytes.len() - back];
        if b & 0xC0 != 0x80 {
            let len = match b {
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                _ => 1,
            };
            return if len > back { back } else { 0 };
        }
    }
    0
}

/// A spawned shell together with every process it starts. Dropping it
/// before the shell has exited (a timed-out, aborted or killed call) kills
/// the whole tree, not just the shell. Unix starts the shell in a new session
/// (`setsid`: `killpg` reaches its descendants, and none of them can grab
/// the server's terminal); Windows puts it in a job object. Descendants still
/// running after the shell exits on its own (`nohup … &`) are left alone.
struct ProcessTree {
    child: Box<dyn ChildWrapper>,
    exited: bool,
}

impl ProcessTree {
    fn spawn(command: Command) -> std::io::Result<Self> {
        let mut wrap = CommandWrap::from(command);
        #[cfg(unix)]
        wrap.wrap(ProcessSession);
        #[cfg(windows)]
        wrap.wrap(JobObject);
        Ok(Self {
            child: wrap.spawn()?,
            exited: false,
        })
    }

    /// Wait for the shell itself. The wrapper's own `wait` also waits for
    /// every descendant, which would hang on a backgrounded daemon.
    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.inner_mut().wait().await?;
        self.exited = true;
        Ok(status)
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if !self.exited {
            let _ = self.child.start_kill();
        }
    }
}

/// Pump a spawned child's stdout/stderr through `on_chunk` and aggregate them
/// until the shell exits and the pipes close, or [`DRAIN_AFTER_EXIT`] after
/// the exit when something it started still holds them.
async fn collect_stream(
    tree: &mut ProcessTree,
    on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
) -> Result<(Vec<u8>, Vec<u8>, i32)> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    if let Some(out) = tree.child.stdout().take() {
        tokio::spawn(read_pipe(out, false, tx.clone()));
    }
    if let Some(err) = tree.child.stderr().take() {
        tokio::spawn(read_pipe(err, true, tx.clone()));
    }
    drop(tx);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut stdout_text, mut stderr_text) = (Utf8Chunks::default(), Utf8Chunks::default());
    let mut status = None;
    let mut deadline = None;
    let mut lingering = false;
    loop {
        let drain = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        let (is_stderr, bytes) = tokio::select! {
            msg = rx.recv() => match msg {
                Some(msg) => msg,
                None => break,
            },
            s = tree.wait(), if status.is_none() => {
                status = Some(s.map_err(|source| IoError::op("waiting for the shell", source))?);
                deadline = Some(tokio::time::Instant::now() + DRAIN_AFTER_EXIT);
                continue;
            }
            () = drain => {
                lingering = true;
                break;
            }
        };
        let (buf, decoder) = if is_stderr {
            (&mut stderr, &mut stderr_text)
        } else {
            (&mut stdout, &mut stdout_text)
        };
        buf.extend_from_slice(&bytes);
        let text = decoder.push(&bytes);
        if !text.is_empty() {
            on_chunk(&text);
        }
    }
    for text in [stdout_text.finish(), stderr_text.finish()] {
        if !text.is_empty() {
            on_chunk(&text);
        }
    }
    let status = match status {
        Some(s) => s,
        None => tree
            .wait()
            .await
            .map_err(|source| IoError::op("waiting for the shell", source))?,
    };
    if lingering {
        let note =
            "\n[processes it started are still running; their later output is not captured]\n";
        stdout.extend_from_slice(note.as_bytes());
        on_chunk(note);
    }
    Ok((stdout, stderr, status.code().unwrap_or(-1)))
}

// ---------------------------------------------------------------------------
// local
// ---------------------------------------------------------------------------

pub(crate) struct LocalBackend {
    pub root: PathBuf,
}

/// Does a `bash` binary exist? Probed once per process; `sh` is the fallback.
fn has_bash() -> bool {
    static HAS_BASH: OnceLock<bool> = OnceLock::new();
    *HAS_BASH.get_or_init(|| {
        std::process::Command::new("bash")
            .arg("-c")
            .arg("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

#[async_trait]
impl Backend for LocalBackend {
    async fn read_bytes(&self, abs: &Utf8TypedPathBuf) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(abs.as_str())
            .await
            .map_err(|source| IoError::new("reading", abs.as_str(), source))?)
    }

    async fn write_bytes(&self, abs: &Utf8TypedPathBuf, bytes: &[u8]) -> Result<()> {
        let full = Path::new(abs.as_str());
        tokio::fs::write(full, bytes)
            .await
            .map_err(IoError::at("writing", full))?;
        Ok(())
    }

    async fn remove_file(&self, abs: &Utf8TypedPathBuf) -> Result<()> {
        Ok(tokio::fs::remove_file(abs.as_str())
            .await
            .map_err(|source| IoError::new("removing", abs.as_str(), source))?)
    }

    async fn list_dir(&self, abs: &Utf8TypedPathBuf) -> Result<Vec<(String, bool)>> {
        let mut reader = tokio::fs::read_dir(abs.as_str())
            .await
            .map_err(|source| IoError::new("listing", abs.as_str(), source))?;
        let mut entries = Vec::new();
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|source| IoError::new("reading an entry of", abs.as_str(), source))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push((name, is_dir));
        }
        entries.sort();
        Ok(entries)
    }

    async fn exec(
        &self,
        script: &str,
        on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ExecOutput> {
        // A leading `exec 2>&1` merges stderr into stdout for the whole script
        // (appending `2>&1` would lose to redirections inside the command).
        let script = format!("exec 2>&1\n{script}");
        let shell = if has_bash() { "bash" } else { "sh" };
        let mut command = Command::new(shell);
        command
            .arg("-c")
            .arg(&script)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut tree = ProcessTree::spawn(command).map_err(|source| {
            IoError::new(
                "spawning",
                format!("{shell} in {}", self.root.display()),
                source,
            )
        })?;
        let (stdout, stderr, code) = collect_stream(&mut tree, on_chunk).await?;
        let mut combined = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.trim().is_empty() {
            combined.push('\n');
            combined.push_str(&stderr);
        }
        Ok(ExecOutput { combined, code })
    }
}

// ---------------------------------------------------------------------------
// ssh
// ---------------------------------------------------------------------------

/// Runs everything through the system `ssh` binary, multiplexing connections
/// (`ControlMaster=auto` + `ControlPersist=10m`): the first command pays the
/// connection cost, later ones ride the persistent master. Read/write/list
/// are remote POSIX commands with every path single-quote-escaped; ssh opens
/// the master lazily on first use.
pub(crate) struct SshBackend {
    pub host: String,
    pub root: Utf8TypedPathBuf,
    /// ControlPath socket directory; `None` disables multiplexing.
    pub ctl_dir: Option<PathBuf>,
}

/// Single-quote a string for interpolation into a remote POSIX shell command:
/// wrap in `'…'`, escaping embedded quotes as `'\''`. Safe for any content,
/// including newlines and `$`.
pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The ssh arguments for `host` (not including the program name — callers
/// spawn `ssh` themselves). Multiplexing options are included only when a
/// ControlPath `ctl` is given; the host is always the last argument.
pub(crate) fn ssh_args(host: &str, ctl: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-o".into(), "BatchMode=yes".into()];
    if let Some(ctl) = ctl {
        args.push("-o".into());
        args.push("ControlMaster=auto".into());
        args.push("-o".into());
        args.push(format!("ControlPath={}", ctl.display()));
        args.push("-o".into());
        args.push("ControlPersist=10m".into());
    }
    args.push("-o".into());
    args.push("ConnectTimeout=10".into());
    args.push(host.into());
    args
}

/// A short, filesystem-safe ControlPath socket name for an ssh target.
pub(crate) fn ctl_path(dir: &Path, host: &str) -> PathBuf {
    let sanitized: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut hasher = DefaultHasher::new();
    host.hash(&mut hasher);
    dir.join(format!("{sanitized}-{:016x}.sock", hasher.finish()))
}

/// Exit status the remote read/list commands use for "no such path", so a
/// missing file maps to [`std::io::ErrorKind::NotFound`] like the local
/// backend (ssh itself exits 255; `cat`/`ls` use 1–2).
const REMOTE_MISSING_EXIT: i32 = 44;

/// `test -e <path> || exit 44; cat <path>` — the path is machine-absolute.
pub(crate) fn remote_read_cmd(abs: &Utf8UnixPath) -> String {
    let path = sh_quote(abs.as_str());
    format!("test -e {path} || exit {REMOTE_MISSING_EXIT}; cat {path}")
}

/// `test -d <parent> || exit 44; cat > <path>` (content arrives on stdin).
pub(crate) fn remote_write_cmd(abs: &Utf8UnixPath) -> String {
    let target = sh_quote(abs.as_str());
    match abs.parent() {
        Some(parent) if !matches!(parent.as_str(), "" | "/") => format!(
            "test -d {} || exit {REMOTE_MISSING_EXIT}; cat > {target}",
            sh_quote(parent.as_str())
        ),
        _ => format!("cat > {target}"),
    }
}

/// `test -e <path> || exit 44; rm -- <path>`.
pub(crate) fn remote_remove_cmd(abs: &Utf8UnixPath) -> String {
    let path = sh_quote(abs.as_str());
    format!("test -e {path} || exit {REMOTE_MISSING_EXIT}; rm -- {path}")
}

/// `LC_ALL=C ls -1pA <dir>` — one entry per line, dirs with a trailing `/`,
/// deterministic C-collation order, hidden entries included (matching the
/// local backend's `read_dir`).
pub(crate) fn remote_list_cmd(abs: &Utf8UnixPath) -> String {
    let path = sh_quote(abs.as_str());
    format!("test -e {path} || exit {REMOTE_MISSING_EXIT}; LC_ALL=C ls -1pA {path}")
}

/// Turn a failed remote read/write/list into an error; a missing path becomes
/// [`BackendError::NotFound`] (see [`is_not_found`]).
fn remote_failure(
    op: &'static str,
    abs: &Utf8UnixPath,
    out: &std::process::Output,
) -> BackendError {
    if out.status.code() == Some(REMOTE_MISSING_EXIT) {
        return BackendError::NotFound {
            op,
            path: abs.to_string(),
        };
    }
    BackendError::Remote {
        op,
        path: abs.to_string(),
        code: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
    }
}

/// `bash -c <quoted script>` — the script rides as one argv element, quoted
/// whole, so the remote default shell only ever sees our single-quoted string.
/// It starts in the workspace root (ssh lands in the login directory).
pub(crate) fn remote_exec_cmd(root: &Utf8UnixPath, script: &str) -> String {
    let script = format!(
        "exec 2>&1\ncd {} || exit 1\n{script}",
        sh_quote(root.as_str())
    );
    format!("bash -c {}", sh_quote(&script))
}

/// A path for the ssh backend, which speaks POSIX: a Windows path reaching
/// it is a workspace-style bug.
fn posix(abs: &Utf8TypedPathBuf) -> Result<&Utf8UnixPath> {
    match abs {
        Utf8TypedPathBuf::Unix(path) => Ok(path),
        Utf8TypedPathBuf::Windows(path) => {
            debug_assert!(false, "ssh backend received a Windows path: {path}");
            Err(BackendError::NotPosix(path.to_string()))
        }
    }
}

impl SshBackend {
    fn ctl(&self) -> Option<PathBuf> {
        self.ctl_dir.as_deref().map(|dir| ctl_path(dir, &self.host))
    }

    fn command(&self, remote_cmd: &str) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(ssh_args(&self.host, self.ctl().as_deref()))
            .arg(remote_cmd)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }

    async fn run(&self, remote_cmd: &str) -> Result<std::process::Output> {
        self.command(remote_cmd)
            .output()
            .await
            .map_err(|source| BackendError::Ssh {
                host: self.host.clone(),
                remote_cmd: remote_cmd.to_owned(),
                source,
            })
    }
}

#[async_trait]
impl Backend for SshBackend {
    async fn read_bytes(&self, abs: &Utf8TypedPathBuf) -> Result<Vec<u8>> {
        let abs = posix(abs)?;
        let out = self.run(&remote_read_cmd(abs)).await?;
        if !out.status.success() {
            return Err(remote_failure("cat", abs, &out));
        }
        Ok(out.stdout)
    }

    async fn write_bytes(&self, abs: &Utf8TypedPathBuf, bytes: &[u8]) -> Result<()> {
        let abs = posix(abs)?;
        use tokio::io::AsyncWriteExt;
        let mut child = self
            .command(&remote_write_cmd(abs))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| BackendError::SpawnWrite { source })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bytes)
                .await
                .map_err(|source| IoError::op("writing to the ssh stdin", source))?;
            stdin
                .shutdown()
                .await
                .map_err(|source| IoError::op("closing the ssh stdin", source))?;
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|source| IoError::op("waiting for the ssh write", source))?;
        if !out.status.success() {
            return Err(remote_failure("write", abs, &out));
        }
        Ok(())
    }

    async fn remove_file(&self, abs: &Utf8TypedPathBuf) -> Result<()> {
        let abs = posix(abs)?;
        let out = self.run(&remote_remove_cmd(abs)).await?;
        if !out.status.success() {
            return Err(remote_failure("rm", abs, &out));
        }
        Ok(())
    }

    async fn list_dir(&self, abs: &Utf8TypedPathBuf) -> Result<Vec<(String, bool)>> {
        let abs = posix(abs)?;
        let out = self.run(&remote_list_cmd(abs)).await?;
        if !out.status.success() {
            return Err(remote_failure("ls", abs, &out));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut entries: Vec<(String, bool)> = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| match l.strip_suffix('/') {
                Some(name) => (name.to_owned(), true),
                None => (l.to_owned(), false),
            })
            .collect();
        entries.sort();
        Ok(entries)
    }

    async fn exec(
        &self,
        script: &str,
        on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ExecOutput> {
        let root = posix(&self.root)?;
        let mut command = self.command(&remote_exec_cmd(root, script));
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut tree = ProcessTree::spawn(command).map_err(|source| {
            IoError::new("spawning", format!("ssh exec on {}", self.host), source)
        })?;
        let (stdout, stderr, code) = collect_stream(&mut tree, on_chunk).await?;
        // stderr of the local ssh process itself (not the remote command)
        // rides along after the combined stream.
        let mut combined = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.trim().is_empty() {
            combined.push('\n');
            combined.push_str(&stderr);
        }
        Ok(ExecOutput { combined, code })
    }
}

/// Probe the remote OS/arch of an ssh target (`uname -sm`), cached per target
/// for the process lifetime (success and failure alike). Serves the
/// workspace listing's ssh line. `None` = not probed (no ctl dir / probe
/// failed).
pub(crate) async fn probe_uname(
    host: &str,
    ctl_dir: Option<&Path>,
    cache: &std::sync::Mutex<HashMap<String, Option<String>>>,
) -> Option<String> {
    {
        let cached = cache.lock().unwrap();
        if let Some(hit) = cached.get(host) {
            return hit.clone();
        }
    }
    let ctl = ctl_dir.map(|dir| ctl_path(dir, host));
    let probed = tokio::time::timeout(Duration::from_secs(5), async {
        let mut cmd = Command::new("ssh");
        cmd.args(ssh_args(host, ctl.as_deref()))
            .arg("uname -sm")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_owned())
            })
    })
    .await
    .unwrap_or_default()
    .filter(|s| !s.is_empty());
    cache
        .lock()
        .unwrap()
        .insert(host.to_owned(), probed.clone());
    probed
}

/// Is this backend error a missing file or directory?
pub(crate) fn is_not_found(e: &BackendError) -> bool {
    match e {
        BackendError::NotFound { .. } => true,
        BackendError::Io(io) => io.is_not_found(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_chunks_hold_split_characters() {
        let bytes = "a中😀b".as_bytes();
        // Every split point, including inside both multi-byte characters.
        for at in 0..=bytes.len() {
            let mut decoder = Utf8Chunks::default();
            let mut text = decoder.push(&bytes[..at]);
            text.push_str(&decoder.push(&bytes[at..]));
            text.push_str(&decoder.finish());
            assert_eq!(text, "a中😀b", "split at {at}");
        }
        // Invalid bytes still decode lossily, and a truncated character at
        // EOF comes out on `finish`.
        let mut decoder = Utf8Chunks::default();
        assert_eq!(decoder.push(b"x\xffy\xe4\xb8"), "x\u{fffd}y");
        assert_eq!(decoder.finish(), "\u{fffd}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_returns_while_a_started_process_holds_the_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend {
            root: dir.path().to_owned(),
        };
        let started = std::time::Instant::now();
        let out = backend
            .exec("sleep 5 & echo started", &mut |_| {})
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(out.code, 0);
        assert!(out.combined.starts_with("started\n"), "{}", out.combined);
        assert!(out.combined.contains("still running"), "{}", out.combined);
        // A command that leaves nothing behind gets no notice.
        let out = backend.exec("echo done", &mut |_| {}).await.unwrap();
        assert_eq!(out.combined, "done\n");
    }

    #[test]
    fn quoting_and_remote_commands() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("with space"), "'with space'");
        assert_eq!(sh_quote("$HOME `x` \"y\""), "'$HOME `x` \"y\"'");
        // Embedded single quotes close, escape and reopen the string.
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote("a'b'c"), "'a'\\''b'\\''c'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("line1\nline2"), "'line1\nline2'");

        // Paths are machine-absolute: no root joining happens here.
        assert_eq!(
            remote_read_cmd(Utf8UnixPath::new("/srv/app/src/main.rs")),
            "test -e '/srv/app/src/main.rs' || exit 44; cat '/srv/app/src/main.rs'"
        );
        assert_eq!(
            remote_read_cmd(Utf8UnixPath::new("/srv/app/a'b.txt")),
            "test -e '/srv/app/a'\\''b.txt' || exit 44; cat '/srv/app/a'\\''b.txt'"
        );
        // A nested write checks the parent exists, never creates it.
        assert_eq!(
            remote_write_cmd(Utf8UnixPath::new("/srv/app/a/b.txt")),
            "test -d '/srv/app/a' || exit 44; cat > '/srv/app/a/b.txt'"
        );
        assert_eq!(
            remote_write_cmd(Utf8UnixPath::new("/b.txt")),
            "cat > '/b.txt'"
        );
        assert_eq!(
            remote_list_cmd(Utf8UnixPath::new("/srv/app")),
            "test -e '/srv/app' || exit 44; LC_ALL=C ls -1pA '/srv/app'"
        );
        // The exec script travels as one single-quoted argument and starts in
        // the workspace root.
        assert_eq!(
            remote_exec_cmd(Utf8UnixPath::new("/srv/app"), "echo 'hi' >&2"),
            "bash -c 'exec 2>&1\ncd '\\''/srv/app'\\'' || exit 1\necho '\\''hi'\\'' >&2'"
        );
    }

    #[test]
    fn ssh_args_and_ctl_path() {
        let args = ssh_args("deploy@prod", Some(Path::new("/tmp/rumu-ssh/x.sock")));
        // The program name is added by the caller: the arguments start with
        // an option, and the destination is the last one (never first, or
        // ssh would take "ssh" as the host).
        assert_eq!(args[0], "-o");
        assert!(!args.iter().any(|a| a == "ssh"), "{args:?}");
        for needle in [
            "BatchMode=yes",
            "ControlMaster=auto",
            "ControlPath=/tmp/rumu-ssh/x.sock",
            "ControlPersist=10m",
        ] {
            assert!(args.iter().any(|a| a == needle), "{args:?} lacks {needle}");
        }
        assert_eq!(args.last().unwrap(), "deploy@prod");

        // Without a ctl dir multiplexing is skipped, everything else stays.
        let plain = ssh_args("deploy@prod", None);
        assert_eq!(plain[0], "-o");
        assert_eq!(plain.last().unwrap(), "deploy@prod");
        assert!(!plain.iter().any(|a| a.starts_with("Control")), "{plain:?}");

        let dir = Path::new("/tmp/rumu-ssh");
        let a = ctl_path(dir, "deploy@prod.example.com");
        let b = ctl_path(dir, "deploy@prod.example.com");
        let c = ctl_path(dir, "deploy@prod.example.org");
        assert_eq!(a, b, "stable per host");
        assert_ne!(a, c, "different hosts differ");
        assert!(
            a.to_string_lossy().len() < 100,
            "under the socket path limit"
        );
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')),
            "filesystem-safe: {}",
            a.display()
        );
    }
}
