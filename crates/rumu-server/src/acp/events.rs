//! The first half of the ACP translation: rumu's pushes (the same JSON the
//! web gets) become protocol-neutral [`Event`]s of one session's main line.
//! Each protocol version only renders these (see `v1.rs`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rumu_cards::diff::Hunk;
use rumu_core::{Branch, BranchKind, FILE_PATCH_TAG, Node, NodeBody, PathState, ToolCall};
use serde::Deserialize;
use serde_json::Value;

/// Something that happened on a session's main line.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Event {
    /// A piece of the reply.
    Text(String),
    /// A piece of the reasoning.
    Thought(String),
    /// The model called a tool.
    ToolStart {
        id: String,
        name: String,
        title: String,
        kind: Kind,
        input: Value,
        /// Local files the call is about.
        locations: Vec<PathBuf>,
    },
    /// A tool printed more: `text` is the tail of all it printed so far.
    ToolOutput { id: String, text: String },
    /// A tool returned.
    ToolEnd {
        id: String,
        failed: bool,
        text: String,
        diffs: Vec<FileDiff>,
    },
    /// An `agent` call runs on the sub-agent branch `branch`.
    ToolChild { id: String, branch: String },
}

/// What a tool does, as clients group them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Read,
    Edit,
    Delete,
    Execute,
    Search,
    Other,
}

/// One change to a local file: the whole file or a hunk of it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FileDiff {
    pub path: PathBuf,
    /// `None` for a file that did not exist.
    pub old: Option<String>,
    pub new: String,
}

/// The most of a running tool's output one [`Event::ToolOutput`] carries.
const OUTPUT_TAIL: usize = 8 * 1024;
/// The least time between two [`Event::ToolOutput`]s of one call: each
/// carries the whole tail, and its result carries everything anyway.
const OUTPUT_EVERY: Duration = Duration::from_secs(1);

/// A call the model made, kept until its result lands.
struct Open {
    call: ToolCall,
    /// Its `path` argument, resolved to a local file.
    path: Option<PathBuf>,
    /// What it printed so far (the last [`OUTPUT_TAIL`] bytes).
    output: String,
    /// When its output was last reported.
    reported: Option<Instant>,
}

/// Turns the pushes of one main branch into [`Event`]s.
pub(super) struct Translator {
    main: String,
    /// Whether the reply / reasoning in flight has streamed any deltas: a
    /// reply that did not is sent whole when its node lands.
    streamed_text: bool,
    streamed_thought: bool,
    open: HashMap<String, Open>,
    children: HashSet<String>,
}

impl Translator {
    pub fn new(main: String) -> Self {
        Self {
            main,
            streamed_text: false,
            streamed_thought: false,
            open: HashMap::new(),
            children: HashSet::new(),
        }
    }

    /// Whether a push names the main line as its `branch`.
    pub fn on_main(&self, params: &Value) -> bool {
        params.get("branch").and_then(Value::as_str) == Some(self.main.as_str())
    }

    /// The events of one push. `state` is the main line's path state, for
    /// resolving the paths of the calls an assistant node carries.
    pub fn feed(&mut self, method: &str, params: &Value, state: Option<&PathState>) -> Vec<Event> {
        match method {
            "llm.delta" if self.on_main(params) => {
                if let Some(text) = params.get("text").and_then(Value::as_str) {
                    self.streamed_text = true;
                    return vec![Event::Text(text.to_owned())];
                }
                if let Some(text) = params.get("reasoning").and_then(Value::as_str) {
                    self.streamed_thought = true;
                    return vec![Event::Thought(text.to_owned())];
                }
                vec![]
            }
            "node.appended" if self.on_main(params) => {
                match params
                    .get("node")
                    .cloned()
                    .map(serde_json::from_value::<Node>)
                {
                    Some(Ok(node)) => self.node(node, state),
                    _ => vec![],
                }
            }
            "tool.progress" if self.on_main(params) => {
                let id = params.get("call_id").and_then(Value::as_str);
                let text = params.get("append").and_then(Value::as_str);
                let (Some(id), Some(text)) = (id, text) else {
                    return vec![];
                };
                let Some(open) = self.open.get_mut(id) else {
                    return vec![];
                };
                open.output.push_str(text);
                if open.output.len() > OUTPUT_TAIL {
                    let mut cut = open.output.len() - OUTPUT_TAIL;
                    while !open.output.is_char_boundary(cut) {
                        cut += 1;
                    }
                    open.output.drain(..cut);
                }
                if open.reported.is_some_and(|t| t.elapsed() < OUTPUT_EVERY) {
                    return vec![];
                }
                open.reported = Some(Instant::now());
                vec![Event::ToolOutput {
                    id: id.to_owned(),
                    text: open.output.clone(),
                }]
            }
            "branch.updated" => {
                let Some(Ok(branch)) = params
                    .get("branch")
                    .cloned()
                    .map(serde_json::from_value::<Branch>)
                else {
                    return vec![];
                };
                match branch.kind {
                    BranchKind::Agent {
                        parent_branch,
                        call_id,
                        via_plugin: false,
                        ..
                    } if parent_branch.to_string() == self.main
                        && self.children.insert(branch.id.to_string()) =>
                    {
                        vec![Event::ToolChild {
                            id: call_id,
                            branch: branch.id.to_string(),
                        }]
                    }
                    _ => vec![],
                }
            }
            _ => vec![],
        }
    }

    fn node(&mut self, node: Node, state: Option<&PathState>) -> Vec<Event> {
        match node.body {
            NodeBody::Assistant {
                content,
                reasoning,
                tool_calls,
                ..
            } => {
                let mut events = vec![];
                if !self.streamed_thought {
                    let thought: String = reasoning
                        .iter()
                        .filter_map(|b| match b {
                            rumu_core::ReasoningBlock::Thinking { text, .. } => Some(text.as_str()),
                            rumu_core::ReasoningBlock::Redacted { .. } => None,
                        })
                        .collect();
                    if !thought.is_empty() {
                        events.push(Event::Thought(thought));
                    }
                }
                if !self.streamed_text && !content.text.is_empty() {
                    events.push(Event::Text(content.text));
                }
                self.streamed_text = false;
                self.streamed_thought = false;
                for call in tool_calls {
                    let path = call
                        .args
                        .get("path")
                        .and_then(Value::as_str)
                        .zip(state)
                        .and_then(|(raw, state)| local_path(state, raw));
                    events.push(Event::ToolStart {
                        id: call.call_id.clone(),
                        name: call.name.clone(),
                        title: title(&call),
                        kind: kind(&call.name),
                        input: call.args.clone(),
                        locations: path.iter().cloned().collect(),
                    });
                    self.open.insert(
                        call.call_id.clone(),
                        Open {
                            call,
                            path,
                            output: String::new(),
                            reported: None,
                        },
                    );
                }
                events
            }
            NodeBody::ToolResult {
                call_id,
                content,
                is_error,
                invoke: None,
            } => {
                let Some(open) = self.open.remove(&call_id) else {
                    return vec![];
                };
                let diffs = if is_error {
                    vec![]
                } else {
                    open.path
                        .map(|path| diffs(&open.call, path, content.data.as_ref()))
                        .unwrap_or_default()
                };
                vec![Event::ToolEnd {
                    id: call_id,
                    failed: is_error,
                    text: content.text,
                    diffs,
                }]
            }
            _ => vec![],
        }
    }
}

/// The local file a tool path names, when it is one an editor can open.
fn local_path(state: &PathState, raw: &str) -> Option<PathBuf> {
    let addr = rumu_core::resolve_state(state, raw).ok()?;
    let ws = state.workspaces.iter().find(|w| w.name == addr.ws)?;
    ws.is_local().then(|| PathBuf::from(addr.abs.as_str()))
}

fn kind(tool: &str) -> Kind {
    match tool {
        "read" => Kind::Read,
        "write" | "edit" => Kind::Edit,
        "delete" => Kind::Delete,
        "bash" => Kind::Execute,
        "search" | "list" => Kind::Search,
        _ => Kind::Other,
    }
}

/// A one-line title for a call.
fn title(call: &ToolCall) -> String {
    let arg = |key: &str| {
        call.args
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let line = |s: &str| {
        let first = s.lines().next().unwrap_or_default();
        let mut out: String = first.chars().take(80).collect();
        if out.len() < s.len() {
            out.push('…');
        }
        out
    };
    match call.name.as_str() {
        "read" => format!("Read {}", arg("path")),
        "write" => format!("Write {}", arg("path")),
        "edit" => format!("Edit {}", arg("path")),
        "delete" => format!("Delete {}", arg("path")),
        "list" => format!("List {}", arg("path")),
        "show" => format!("Show {}", arg("path")),
        "search" => format!("Search {}", line(arg("query"))),
        "bash" => line(arg("command")),
        "agent" => format!("Agent: {}", line(arg("task"))),
        name => name.to_owned(),
    }
}

/// The `workspace.file_patch` payload of a `write`/`edit` result.
#[derive(Deserialize)]
struct FilePatch {
    kind: String,
    #[serde(default)]
    hunks: Option<Vec<Hunk>>,
}

/// What a `write`/`edit`/`delete` did to `path`: its hunks (a delete's as
/// emptying the file), else what the call itself says (a created file's
/// content, an edit's strings).
fn diffs(call: &ToolCall, path: PathBuf, data: Option<&rumu_core::Tagged>) -> Vec<FileDiff> {
    if !matches!(call.name.as_str(), "write" | "edit" | "delete") {
        return vec![];
    }
    let Some(patch) = data
        .filter(|d| d.tag == FILE_PATCH_TAG)
        .and_then(|d| serde_json::from_value::<FilePatch>(d.data.clone()).ok())
    else {
        return vec![];
    };
    let arg = |key: &str| call.args.get(key).and_then(Value::as_str);
    if let Some(hunks) = patch.hunks {
        return hunks
            .iter()
            .map(|h| {
                let (old, new) = sides(h);
                FileDiff {
                    path: path.clone(),
                    old: Some(old),
                    new,
                }
            })
            .collect();
    }
    match (patch.kind.as_str(), call.name.as_str()) {
        ("create", _) => arg("content")
            .map(|new| FileDiff {
                path,
                old: None,
                new: new.to_owned(),
            })
            .into_iter()
            .collect(),
        ("update", "edit") => match (arg("old_string"), arg("new_string")) {
            (Some(old), Some(new)) => vec![FileDiff {
                path,
                old: Some(old.to_owned()),
                new: new.to_owned(),
            }],
            _ => vec![],
        },
        _ => vec![],
    }
}

/// The old and new text of a hunk.
fn sides(hunk: &Hunk) -> (String, String) {
    let (mut old, mut new) = (vec![], vec![]);
    for line in &hunk.lines {
        match line.split_at_checked(1) {
            Some((" ", text)) => {
                old.push(text);
                new.push(text);
            }
            Some(("-", text)) => old.push(text),
            Some(("+", text)) => new.push(text),
            _ => {}
        }
    }
    (old.join("\n"), new.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks_split_into_their_sides() {
        let hunk = Hunk {
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            lines: vec![" keep".into(), "-old".into(), "+new".into()],
        };
        assert_eq!(sides(&hunk), ("keep\nold".into(), "keep\nnew".into()));
    }
}
