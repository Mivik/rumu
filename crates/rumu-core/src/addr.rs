//! Addressing: workspace names and the `<workspace>:<path>` qualification
//! shared by the tool implementations and hook field extraction. One address
//! space: every workspace preset, the session's `@scratch` directory and the
//! `@rumu` virtual workspace holding rumu's own data.

use std::borrow::Cow;

use crate::{Error, PathState, Result, Workspace};
use typed_path::{Utf8TypedPath, Utf8TypedPathBuf, Utf8UnixPath, Utf8UnixPathBuf};

/// The internal workspace holding rumu's own data (`/cards`, `/sessions`).
/// No bash; reads and card writes only. The leading `@` is not valid in a
/// preset name, so no user workspace can ever collide with it.
pub const RUMU_WORKSPACE: &str = "@rumu";

/// How many leading characters of a branch id its address shows; an address
/// names a branch by any prefix unique in its session.
pub const BRANCH_ABBR: usize = 8;

/// The `@rumu` address of `branch` in `session`: read it for the branch's own
/// transcript, `branch.send` on it to message the agent there.
pub fn branch_address(session: &crate::SessionId, branch: &crate::BranchId) -> String {
    let short: String = branch.0.chars().take(BRANCH_ABBR).collect();
    format!("{RUMU_WORKSPACE}:/sessions/{session}/branches/{short}")
}

/// The session's scratch workspace: a real local directory of its own
/// (`RUMU_HOME/scratch/<session id>`), created on first use and removed with
/// the session. The default when no other workspace is. Like `@rumu`, its
/// `@` keeps it apart from preset names.
pub const SCRATCH_WORKSPACE: &str = "@scratch";

/// The directory inside `@scratch` that rumu itself writes (prompt
/// attachments, incoming events): hidden from `ls` and globs, so the
/// model's own files and clean-ups stay clear of it.
pub const SCRATCH_INBOX: &str = ".inbox";

/// Names that can never be workspaces: the common URI schemes are reserved
/// so `http://…`-shaped text is never mistaken for a qualified path
/// (CommonMark renders `proj:x` as a link, so the syntax must stay
/// scheme-compatible).
const RESERVED_NAMES: &[&str] = &[
    "http",
    "https",
    "file",
    "mailto",
    "data",
    "javascript",
    "ssh",
    "git",
    "ftp",
    "tel",
];

/// Workspace names: `[a-z][a-z0-9.-]*`, at least 2 chars (one-letter names
/// would read as drive letters), never a reserved name. All-lowercase so
/// addressing is unambiguous.
pub fn valid_workspace_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.chars().count() >= 2
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
        && !RESERVED_NAMES.contains(&name)
}

/// A resolved tool-path address: the workspace it lives in and the normalized
/// absolute path inside it (on the workspace's machine in its path style;
/// `/cards/…` or `/sessions/…` for `@rumu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    /// A preset name, `@scratch`, or `@rumu`.
    pub ws: String,
    pub abs: Utf8TypedPathBuf,
}

/// Split `raw` into `(<name>, <rest>)` when it starts like a qualified
/// address whose name could be a workspace (`@scratch` and `@rumu` included).
/// Reserved scheme names (`http`, …) never qualify, so `https://…`-shaped
/// text stays a plain path.
fn split_qualified(raw: &str) -> Option<(&str, &str)> {
    let (name, rest) = raw.split_once(':')?;
    (name == RUMU_WORKSPACE || name == SCRATCH_WORKSPACE || valid_workspace_name(name))
        .then_some((name, rest))
}

/// Resolve a tool `path` argument: `<name>:<rest>` selects that
/// workspace when it is known (a preset, `@scratch`, or `@rumu`); anything
/// else is a path in the branch's default workspace. Errors name the
/// available workspaces.
pub fn resolve(raw: &str, workspaces: &[Workspace], default: Option<&str>) -> Result<Addr> {
    if let Some((name, rest)) = split_qualified(raw) {
        if let Some(ws) = workspaces.iter().find(|w| w.name == name) {
            return Ok(Addr {
                ws: ws.name.clone(),
                abs: normalize_abs(ws, rest)?,
            });
        }
        if name == RUMU_WORKSPACE {
            return Ok(Addr {
                ws: RUMU_WORKSPACE.to_owned(),
                abs: Utf8TypedPathBuf::Unix(normalize_rumu(rest)?),
            });
        }
        return Err(crate::error::unknown_workspace_error(name, raw, workspaces));
    }
    let default = default.ok_or(Error::NoDefaultWorkspace)?;
    if let Some(ws) = workspaces.iter().find(|w| w.name == default) {
        return Ok(Addr {
            ws: ws.name.clone(),
            abs: normalize_abs(ws, raw)?,
        });
    }
    if default == RUMU_WORKSPACE {
        return Ok(Addr {
            ws: RUMU_WORKSPACE.to_owned(),
            abs: Utf8TypedPathBuf::Unix(normalize_rumu(raw)?),
        });
    }
    Err(crate::error::unknown_workspace_error(
        default, raw, workspaces,
    ))
}

/// [`resolve`] over a derived path state (its workspaces are the merged
/// preset catalog).
pub fn resolve_state(state: &PathState, raw: &str) -> Result<Addr> {
    resolve(raw, &state.workspaces, state.default_workspace.as_deref())
}

/// Lexically normalize a path: skip empty and `.` segments, let `..` pop one
/// (stopping at `/`), always return an absolute path with no trailing `/`
/// (the root itself is `/`). Absolute paths and `..` are allowed — the
/// workspace root is an addressing convention, not a security boundary.
pub fn normalize_lexical(path: &Utf8UnixPath) -> Utf8UnixPathBuf {
    Utf8UnixPath::new("/").join(path).normalize()
}

/// Normalize `rel` to an absolute path on the workspace's machine, parsed in
/// the workspace's path style (Windows accepts both `\` and `/`): a leading
/// `/` (or drive) means absolute on that machine, anything else is relative
/// to its root. `~` / `~/…` expands to `$HOME` on local workspaces and is an
/// error on ssh ones. NUL is rejected.
pub fn normalize_abs(ws: &Workspace, rel: &str) -> Result<Utf8TypedPathBuf> {
    if rel.contains('\0') {
        return Err(Error::InvalidPath);
    }
    let rel = expand_tilde(ws, rel)?;
    Ok(ws.root.to_path().join(&*rel).normalize())
}

/// `~` / `~/…` → `$HOME` on local workspaces (an error on ssh ones); any
/// other `~`-prefixed name stays literal.
fn expand_tilde<'a>(ws: &Workspace, rel: &'a str) -> Result<Cow<'a, str>> {
    let Some(tail) = rel.strip_prefix('~') else {
        return Ok(Cow::Borrowed(rel));
    };
    let windows = matches!(ws.path_type(), typed_path::PathType::Windows);
    if !(tail.is_empty() || tail.starts_with('/') || windows && tail.starts_with('\\')) {
        return Ok(Cow::Borrowed(rel));
    }
    if !ws.is_local() {
        return Err(Error::TildeOnSsh);
    }
    let home = std::env::home_dir().unwrap_or_default();
    Ok(Cow::Owned(format!("{}{tail}", home.display())))
}

/// Normalize a path in the `@rumu` workspace (its root is `/`, so
/// `@rumu:cards/x` ≡ `@rumu:/cards/x`).
pub fn normalize_rumu(rel: &str) -> Result<Utf8UnixPathBuf> {
    if rel.contains('\0') {
        return Err(Error::InvalidPath);
    }
    Ok(normalize_lexical(Utf8UnixPath::new(rel)))
}

/// The canonical display form used in tool output, sub-agent footnotes
/// and search results: `@rumu:/cards/<mount>/<path>` for `@rumu`; real workspaces show
/// `proj:src/a.rs` under their root (with `/` separators, whatever the
/// machine's style) and `proj:<absolute path>` outside it.
pub fn display(ws: &str, root: Utf8TypedPath, abs: Utf8TypedPath) -> String {
    if ws == RUMU_WORKSPACE {
        return format!("{ws}:{abs}");
    }
    let root = root.normalize();
    match abs.strip_prefix(&root) {
        Ok(rel) if !rel.as_str().is_empty() => format!("{ws}:{}", rel.with_unix_encoding()),
        _ => format!("{ws}:{abs}"),
    }
}

/// Split a trailing `#L<n>` / `#L<a>-L<b>` anchor off a `read` path. Only
/// `read` interprets it; other tools treat `#` as an ordinary character.
/// Returns the path and `(start line, optional end line)`.
pub fn split_line_anchor(raw: &str) -> (&str, Option<(u64, Option<u64>)>) {
    let Some(hash) = raw.find('#') else {
        return (raw, None);
    };
    let anchor = &raw[hash + 1..];
    let Some(body) = anchor.strip_prefix('L') else {
        return (raw, None);
    };
    let (a, b) = match body.split_once("-L") {
        Some((a, b)) => (a, Some(b)),
        None => (body, None),
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !digits(a) || !b.is_none_or(digits) {
        return (raw, None);
    }
    (
        &raw[..hash],
        Some((
            a.parse().unwrap_or(u64::MAX),
            b.map(|b| b.parse().unwrap_or(u64::MAX)),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(name: &str, root: &str) -> Workspace {
        Workspace {
            name: name.into(),
            ssh: None,
            root: Utf8TypedPathBuf::from_unix(root),
            read_only: false,
        }
    }

    fn ssh(name: &str, root: &str) -> Workspace {
        Workspace {
            ssh: Some("deploy@prod".into()),
            ..ws(name, root)
        }
    }

    fn unix(s: &str) -> Utf8TypedPath<'_> {
        Utf8TypedPath::unix(s)
    }

    #[test]
    fn name_rules() {
        for ok in ["ab", "proj", "my-app", "a.b", "x9", "a-b.c2", "rumu"] {
            assert!(valid_workspace_name(ok), "should accept `{ok}`");
        }
        for bad in [
            "", "a", "A", "Ab", "proj1 ", "proj/x", "proj:x", " proj", "日本", "@rumu",
        ] {
            assert!(!valid_workspace_name(bad), "should reject `{bad}`");
        }
        // Reserved: the common URI schemes.
        for bad in RESERVED_NAMES {
            assert!(!valid_workspace_name(bad), "`{bad}` is reserved");
        }
    }

    #[test]
    fn scratch_qualifies_only_when_the_state_has_it() {
        let mut st = PathState {
            workspaces: vec![ws(SCRATCH_WORKSPACE, "/scratch/s1")],
            default_workspace: Some(SCRATCH_WORKSPACE.into()),
            ..Default::default()
        };
        // `@scratch` is a reserved qualifier: when the state carries it, it
        // resolves to that workspace's root.
        let a = resolve_state(&st, "@scratch:foo/bar.txt").unwrap();
        assert_eq!(a.ws, SCRATCH_WORKSPACE);
        assert_eq!(a.abs.as_str(), "/scratch/s1/foo/bar.txt");
        // Unqualified paths fall to it as the default.
        assert_eq!(
            resolve_state(&st, "foo.txt").unwrap().abs.as_str(),
            "/scratch/s1/foo.txt"
        );
        // Without it in the state, the qualifier is still parsed and then
        // reported as unknown.
        st.workspaces.clear();
        let err = resolve_state(&st, "@scratch:foo").unwrap_err().to_string();
        assert!(err.contains("unknown workspace `@scratch`"), "{err}");
    }

    #[test]
    fn resolves_qualified_and_default_paths() {
        let cat = [ws("proj", "/home/u/code/proj"), ssh("app", "/srv/app")];

        // A known qualifier selects the workspace.
        let a = resolve("proj:src/main.rs", &cat, Some("proj")).unwrap();
        assert_eq!(
            a,
            Addr {
                ws: "proj".into(),
                abs: Utf8TypedPathBuf::from_unix("/home/u/code/proj/src/main.rs")
            }
        );
        // `@rumu` is always known, with or without the leading slash.
        assert_eq!(
            resolve("@rumu:cards/x", &cat, None).unwrap().abs.as_str(),
            "/cards/x"
        );
        assert_eq!(
            resolve("@rumu:/cards/x", &cat, None).unwrap().abs.as_str(),
            "/cards/x"
        );
        // The internal workspace never collides: `rumu` is an ordinary preset.
        assert_eq!(
            resolve("rumu:src/a.rs", &[ws("rumu", "/w/rumu")], None)
                .unwrap()
                .abs
                .as_str(),
            "/w/rumu/src/a.rs"
        );

        // An unknown qualifier errors and teaches the `./` escape.
        let err = resolve("ghost:x.txt", &cat, Some("proj")).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("unknown workspace `ghost`"), "{err}");
        assert!(err.contains("available: proj, app, @rumu"), "{err}");
        assert!(err.contains("`./ghost:x.txt`"), "{err}");

        // Unqualified paths fall to the branch default; `./a:b.txt` defeats
        // the qualifier syntax (a leading `.` is not a name).
        assert_eq!(
            resolve("./a:b.txt", &cat, Some("proj"))
                .unwrap()
                .abs
                .as_str(),
            "/home/u/code/proj/a:b.txt"
        );
        // A `:` in a later segment is not a qualifier either.
        assert_eq!(
            resolve("src/foo:bar.rs", &cat, Some("proj"))
                .unwrap()
                .abs
                .as_str(),
            "/home/u/code/proj/src/foo:bar.rs"
        );
        // Reserved scheme-looking names stay plain paths (their `//`
        // collapses like any empty segment).
        assert_eq!(
            resolve("https://example.com/x", &cat, Some("proj"))
                .unwrap()
                .abs
                .as_str(),
            "/home/u/code/proj/https:/example.com/x"
        );

        // No default anywhere → the no-default error.
        let err = resolve("a.txt", &cat, None).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("no default workspace"), "{err}");
        assert!(err.contains("`workspace.use`"), "{err}");
    }

    #[test]
    fn normalizes_tilde_dotdot_and_absolutes() {
        let local = ws("proj", "/home/u/code/proj");
        // Relative to the root, `.`/empty skipped, `..` pops.
        assert_eq!(
            normalize_abs(&local, "a/./b/../c").unwrap().as_str(),
            "/home/u/code/proj/a/c"
        );
        assert_eq!(
            normalize_abs(&local, "../escape").unwrap().as_str(),
            "/home/u/code/escape"
        );
        // Absolute paths are machine-absolute, not root-relative.
        assert_eq!(
            normalize_abs(&local, "/etc/passwd").unwrap().as_str(),
            "/etc/passwd"
        );
        // `..` from the root pops one level per segment.
        assert_eq!(
            normalize_abs(&local, "../../x").unwrap().as_str(),
            "/home/u/x"
        );
        // Root itself, with or without trailing slash.
        assert_eq!(
            normalize_abs(&local, "").unwrap().as_str(),
            "/home/u/code/proj"
        );
        assert_eq!(
            normalize_lexical(Utf8UnixPath::new("/home/u//")).as_str(),
            "/home/u"
        );
        assert_eq!(normalize_lexical(Utf8UnixPath::new("/")).as_str(), "/");
        // NUL is rejected.
        assert!(normalize_abs(&local, "a\0b").is_err());

        // `~` expands on local workspaces.
        let home = std::env::home_dir()
            .unwrap_or_default()
            .display()
            .to_string();
        assert_eq!(
            normalize_abs(&local, "~/notes").unwrap().as_str(),
            format!("{home}/notes")
        );
        assert_eq!(normalize_abs(&local, "~").unwrap().as_str(), home);
        // …and is an error on ssh ones.
        let err = normalize_abs(&ssh("app", "/srv/app"), "~/notes").unwrap_err();
        assert_eq!(
            err.to_string(),
            "`~` is not expanded on ssh workspaces; use an absolute path"
        );

        // The @rumu workspace's root is `/`.
        assert_eq!(normalize_rumu("cards/x").unwrap().as_str(), "/cards/x");
        assert_eq!(
            normalize_rumu("/sessions/a/../b").unwrap().as_str(),
            "/sessions/b"
        );
    }

    #[test]
    fn line_anchors() {
        let (p, a) = split_line_anchor("proj:src/a.rs#L10");
        assert_eq!(p, "proj:src/a.rs");
        assert_eq!(a, Some((10, None)));
        let (p, a) = split_line_anchor("src/a.rs#L10-L20");
        assert_eq!(p, "src/a.rs");
        assert_eq!(a, Some((10, Some(20))));
        let (p, a) = split_line_anchor("@rumu:/cards/x.md#L3");
        assert_eq!(p, "@rumu:/cards/x.md");
        assert_eq!(a, Some((3, None)));

        // Not anchors: no `L`, non-digits, or an earlier `#`.
        for raw in [
            "a.rs",
            "a.rs#L",
            "a.rs#Lx",
            "a.rs#10",
            "a.rs#L1-L",
            "a#b#L1",
        ] {
            let (p, a) = split_line_anchor(raw);
            assert_eq!((p, a), (raw, None), "{raw}");
        }
    }

    #[test]
    fn display_forms() {
        // Under the root: root-relative. Outside: absolute.
        assert_eq!(
            display(
                "proj",
                unix("/home/u/code/proj"),
                unix("/home/u/code/proj/src/a.rs")
            ),
            "proj:src/a.rs"
        );
        assert_eq!(
            display("proj", unix("/p/r"), unix("/etc/passwd")),
            "proj:/etc/passwd"
        );
        assert_eq!(display("proj", unix("/p/r/"), unix("/p/r/x")), "proj:x");
        // The root itself shows absolute.
        assert_eq!(display("proj", unix("/p/r"), unix("/p/r")), "proj:/p/r");
        // @rumu always keeps the slash: `@rumu:/cards/x`.
        assert_eq!(
            display(RUMU_WORKSPACE, unix("/"), unix("/cards/x")),
            "@rumu:/cards/x"
        );
    }

    #[test]
    fn windows_workspace_paths() {
        let win = Workspace {
            name: "proj".into(),
            ssh: None,
            root: Utf8TypedPathBuf::from_windows(r"C:\Users\me\proj"),
            read_only: false,
        };
        // Both separators parse in the workspace's style.
        assert_eq!(
            normalize_abs(&win, r"src\a.rs").unwrap().as_str(),
            r"C:\Users\me\proj\src\a.rs"
        );
        assert_eq!(
            normalize_abs(&win, "src/../b.rs").unwrap().as_str(),
            r"C:\Users\me\proj\b.rs"
        );
        // Display keeps `/` under the root, whatever the machine's style.
        let abs = normalize_abs(&win, r"src\a.rs").unwrap();
        assert_eq!(
            display("proj", win.root.to_path(), abs.to_path()),
            "proj:src/a.rs"
        );
        // Outside the root, the native path is shown as-is.
        let outside = normalize_abs(&win, r"D:\Windows\x").unwrap();
        assert_eq!(
            display("proj", win.root.to_path(), outside.to_path()),
            r"proj:D:\Windows\x"
        );
        // `..` pops through the root but stays on its drive.
        assert_eq!(
            normalize_abs(&win, r"..\other").unwrap().as_str(),
            r"C:\Users\me\other"
        );
    }

    #[test]
    fn ssh_workspaces_stay_unix() {
        let app = ssh("app", "/srv/app");
        assert!(matches!(app.path_type(), typed_path::PathType::Unix));
        assert_eq!(
            normalize_abs(&app, "src/a.rs").unwrap().as_str(),
            "/srv/app/src/a.rs"
        );
        assert_eq!(
            display("app", app.root.to_path(), unix("/srv/app/src/a.rs")),
            "app:src/a.rs"
        );
    }
}
