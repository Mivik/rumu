//! Field extraction (`Fields`) from event payloads and path state.

use rumu_core::{Addr, Fields, PathState, RUMU_WORKSPACE, Utf8TypedPathBuf, Utf8UnixPath};
use serde_json::Value;

/// Extract normalized fields for an event from its payload and the current path state.
pub fn extract_fields(kind: &str, payload: &Value, state: &PathState) -> Fields {
    let mut f = Fields {
        event: kind.to_owned(),
        ..Default::default()
    };
    match kind {
        "prompt" | "llm.done" => {
            f.text = payload
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        "tool.pre" | "tool.post" => {
            f.tool = payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned);
            // Internal `code` calls carry `caller: "code"`; direct model calls
            // omit it (and read as "model"). Kernel-owned: rewrites cannot
            // change it (see `dispatch`).
            f.caller = Some(
                payload
                    .get("caller")
                    .and_then(Value::as_str)
                    .unwrap_or("model")
                    .to_owned(),
            );
            let args = payload.get("args");
            // `cmd` is the command or script the tool is about to run:
            // `bash` takes `command`, `code` takes `code`.
            let cmd_key = if f.tool.as_deref() == Some("code") {
                "code"
            } else {
                "command"
            };
            f.cmd = args
                .and_then(|a| a.get(cmd_key))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(args) = args {
                match f.tool.as_deref() {
                    Some("read") | Some("write") | Some("edit") | Some("list") => {
                        let (ws, paths) = file_target(args, state);
                        f.workspace = ws;
                        f.paths = paths;
                    }
                    Some("bash") => {
                        f.workspace = bash_target(args, state);
                    }
                    // search / workspace_use / agent / code carry no
                    // workspace or paths.
                    _ => {}
                }
            }
            if kind == "tool.post" {
                f.output = payload
                    .get("output")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
        }
        "post" => {
            f.source = payload
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        _ => {}
    }
    f
}

/// The `workspace` of a `bash` call: `args.workspace` (a bare name) or the
/// branch default, when it names a preset or `@rumu`.
fn bash_target(args: &Value, state: &PathState) -> Option<String> {
    let raw = args
        .get("workspace")
        .and_then(Value::as_str)
        .or(state.default_workspace.as_deref())?;
    known(raw, state).then(|| raw.to_owned())
}

/// The `workspace` / `paths` of a file tool call: every `path`(+`paths`) arg
/// is resolved through the shared address parser; unresolvable entries are
/// skipped because the tool itself reports them. `workspace` comes from the
/// first resolvable entry (or the valid default when there is no path at
/// all, e.g. a bare `list`).
fn file_target(args: &Value, state: &PathState) -> (Option<String>, Vec<String>) {
    let mut workspace = None;
    let mut paths = Vec::new();
    let mut add = |raw: &str| {
        // Unknown workspace / no default: the tool errors; the fields just
        // stay absent.
        if let Ok(Addr { ws, abs }) = rumu_core::resolve_state(state, raw) {
            if workspace.is_none() {
                workspace = Some(ws.clone());
            }
            paths.push(if ws == RUMU_WORKSPACE {
                rumu_field_path(&abs)
            } else {
                abs.to_string()
            });
        }
    };
    if let Some(p) = args.get("path").and_then(Value::as_str) {
        add(p);
    }
    if let Some(list) = args.get("paths").and_then(Value::as_array) {
        for p in list.iter().filter_map(Value::as_str) {
            add(p);
        }
    }
    if workspace.is_none() && args.get("path").is_none() && args.get("paths").is_none() {
        // A path-less call still names its target: the valid branch default.
        if let Some(default) = state.default_workspace.as_deref()
            && known(default, state)
        {
            workspace = Some(default.to_owned());
        }
    }
    (workspace, paths)
}

/// Does `name` address a workspace here (a preset or `@rumu`)?
fn known(name: &str, state: &PathState) -> bool {
    name == RUMU_WORKSPACE || state.workspaces.iter().any(|w| w.name == name)
}

/// The field form of a `@rumu` workspace path: card ids drop their optional
/// `.md` (`/cards/<id>`); everything else (including `/sessions/…` and bare
/// prefixes) keeps the normalized path.
fn rumu_field_path(abs: &Utf8TypedPathBuf) -> String {
    let Utf8TypedPathBuf::Unix(abs) = abs else {
        return abs.to_string();
    };
    match abs.strip_prefix("/cards").ok().map(Utf8UnixPath::as_str) {
        Some(id) if !id.is_empty() => match id.strip_suffix(".md") {
            Some(stripped) if !stripped.is_empty() => format!("/cards/{stripped}"),
            _ => abs.to_string(),
        },
        _ => abs.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::Workspace;
    use serde_json::json;

    fn state() -> PathState {
        PathState {
            workspaces: vec![Workspace {
                name: "proj".into(),
                ssh: None,
                root: "/home/u/code/proj".into(),
                read_only: false,
            }],
            default_workspace: Some("proj".into()),
            ..Default::default()
        }
    }

    #[test]
    fn code_cmd_and_caller() {
        // `cmd` is the script for `code` (not `args.command`), and `caller`
        // distinguishes direct calls from script-internal ones.
        let st = state();
        let payload = json!({
            "name": "code",
            "args": { "code": "await rumu.bash({ command: \"rm -rf /\" });" },
            "caller": "code",
            "parent_call": "c1",
        });
        let f = extract_fields("tool.pre", &payload, &st);
        assert_eq!(
            f.cmd.as_deref(),
            Some("await rumu.bash({ command: \"rm -rf /\" });")
        );
        assert_eq!(f.caller.as_deref(), Some("code"));
        // `code` extracts no workspace/paths.
        assert_eq!(f.workspace, None);
        assert!(f.paths.is_empty());

        // Direct payloads omit `caller`; it reads as "model".
        let payload = json!({ "name": "code", "args": { "code": "return 1;" } });
        let f = extract_fields("tool.pre", &payload, &st);
        assert_eq!(f.caller.as_deref(), Some("model"));
        assert_eq!(f.cmd.as_deref(), Some("return 1;"));

        // Non-tool events have no caller.
        let f = extract_fields("prompt", &json!({ "text": "hi" }), &st);
        assert_eq!(f.caller, None);
    }

    #[test]
    fn file_paths_resolve_to_absolute_machine_paths() {
        let st = state();
        let payload = json!({
            "name": "write",
            "args": {"path": "src/main.rs", "paths": ["app:~/x", "a.rs", "ghost:b.rs", "../escape", "/etc/hosts"]}
        });
        let f = extract_fields("tool.pre", &payload, &st);
        assert_eq!(
            f.paths,
            vec![
                "/home/u/code/proj/src/main.rs".to_owned(),
                "/home/u/code/proj/a.rs".to_owned(),
                "/home/u/code/escape".to_owned(),
                "/etc/hosts".to_owned(),
            ],
            "unknown qualifiers are skipped; `..` and absolutes are allowed"
        );
        assert_eq!(f.workspace.as_deref(), Some("proj"));
        assert_eq!(f.tool.as_deref(), Some("write"));
        // Non-tool events carry no workspace (and no tool fields at all).
        let f = extract_fields("prompt", &json!({ "text": "hi" }), &st);
        assert_eq!(f.workspace, None);

        // No default workspace → no workspace, no paths.
        let f = extract_fields(
            "tool.pre",
            &json!({ "name": "read", "args": {"path": "a.rs"} }),
            &PathState::default(),
        );
        assert!(f.paths.is_empty());
        assert_eq!(f.workspace, None);
    }

    #[test]
    fn qualified_paths_and_the_rumu_workspace() {
        let st = state();
        // `app` is not a preset here: the entry is skipped, `proj` (the
        // resolvable one) names the workspace.
        let payload = json!({
            "name": "read",
            "args": {"path": "app:/var/log/app.log", "paths": ["proj:src/a.rs"]}
        });
        let f = extract_fields("tool.pre", &payload, &st);
        assert_eq!(f.paths, ["/home/u/code/proj/src/a.rs".to_owned()]);
        assert_eq!(f.workspace.as_deref(), Some("proj"));

        // @rumu paths: card ids lose the optional `.md`; sessions keep theirs.
        let payload = json!({
            "name": "read",
            "args": {"path": "@rumu:/cards/dev/x.md", "paths": ["@rumu:cards/y", "@rumu:/sessions/s1/n1"]}
        });
        let f = extract_fields("tool.pre", &payload, &st);
        assert_eq!(
            f.paths,
            [
                "/cards/dev/x".to_owned(),
                "/cards/y".to_owned(),
                "/sessions/s1/n1".to_owned()
            ]
        );
        assert_eq!(f.workspace.as_deref(), Some("@rumu"));

        // A default of `@rumu` applies to unqualified paths.
        let st = PathState {
            default_workspace: Some("@rumu".into()),
            ..st
        };
        let payload = json!({ "name": "list", "args": {"path": "cards/dev"} });
        let f = extract_fields("tool.pre", &payload, &st);
        assert_eq!(f.workspace.as_deref(), Some("@rumu"));
        assert_eq!(f.paths, ["/cards/dev".to_owned()]);
    }

    #[test]
    fn bash_targets_and_bare_lists() {
        let st = state();
        // `bash` carries no path; the resolved target (`proj`, the branch
        // default) is still matchable — and so is an explicit name.
        for ws in [None, Some("proj")] {
            let mut args = json!({ "command": "ls" });
            if let Some(w) = ws {
                args["workspace"] = json!(w);
            }
            let f = extract_fields("tool.pre", &json!({ "name": "bash", "args": args }), &st);
            assert!(f.paths.is_empty());
            assert_eq!(f.workspace.as_deref(), Some("proj"));
        }
        // An unknown explicit workspace resolves nothing.
        let f = extract_fields(
            "tool.pre",
            &json!({ "name": "bash", "args": { "workspace": "ghost", "command": "ls" } }),
            &st,
        );
        assert_eq!(f.workspace, None);
        // `workspace_use` / `search` / `agent` extract nothing.
        for tool in ["workspace_use", "search", "agent"] {
            let f = extract_fields(
                "tool.pre",
                &json!({ "name": tool, "args": { "query": "x", "goal": "g" } }),
                &st,
            );
            assert_eq!(f.workspace, None, "{tool}");
            assert!(f.paths.is_empty(), "{tool}");
        }
        // A path-less `list` still names the default workspace.
        let f = extract_fields("tool.pre", &json!({ "name": "list", "args": {} }), &st);
        assert_eq!(f.workspace.as_deref(), Some("proj"));
        assert!(f.paths.is_empty());
    }
}
