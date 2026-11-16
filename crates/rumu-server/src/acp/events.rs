//! The first half of the ACP translation: rumu's pushes (the same JSON the
//! web gets) become protocol-neutral [`Event`]s of one session's main line.
//! Each protocol version only renders these (see `v1.rs`).

use rumu_core::{Node, NodeBody};
use serde_json::Value;

/// Something that happened on a session's main line.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Event {
    /// A piece of the reply.
    Text(String),
    /// A piece of the reasoning.
    Thought(String),
}

/// Turns the pushes of one main branch into [`Event`]s.
pub(super) struct Translator {
    main: String,
    /// Whether the reply / reasoning in flight has streamed any deltas: a
    /// reply that did not is sent whole when its node lands.
    streamed_text: bool,
    streamed_thought: bool,
}

impl Translator {
    pub fn new(main: String) -> Self {
        Self {
            main,
            streamed_text: false,
            streamed_thought: false,
        }
    }

    /// Whether a push names the main line as its `branch`.
    pub fn on_main(&self, params: &Value) -> bool {
        params.get("branch").and_then(Value::as_str) == Some(self.main.as_str())
    }

    /// The events of one push.
    pub fn feed(&mut self, method: &str, params: &Value) -> Vec<Event> {
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
                    Some(Ok(node)) => self.node(node),
                    _ => vec![],
                }
            }
            _ => vec![],
        }
    }

    fn node(&mut self, node: Node) -> Vec<Event> {
        let NodeBody::Assistant {
            content, reasoning, ..
        } = node.body
        else {
            return vec![];
        };
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
        events
    }
}
