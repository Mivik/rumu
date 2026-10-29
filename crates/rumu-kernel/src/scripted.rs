//! A scripted [`Llm`]: replays a fixed list of replies in order. Tests use it
//! to drive the kernel deterministically; the server uses it (in looping mode)
//! for `RUMU_LLM=script:<path>` so the real UI can run without a model.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use rumu_core::{Node, ReasoningBlock, ToolCall, ToolSpec, Usage};
use serde::Deserialize;

use crate::{Delta, Llm, LlmError, LlmResponse, OnDelta};

/// Characters per streamed delta.
const CHUNK_CHARS: usize = 16;
/// Reply used by [`ScriptedLlm::looping`] when its script is empty.
const EMPTY_REPLY: &str = "(the scripted LLM has no turns)";

/// One reply in a [`Script`] file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptTurn {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

impl From<ScriptTurn> for LlmResponse {
    fn from(turn: ScriptTurn) -> Self {
        Self {
            text: turn.text,
            reasoning: turn
                .reasoning
                .into_iter()
                .map(|text| ReasoningBlock::Thinking {
                    text,
                    signature: None,
                })
                .collect(),
            tool_calls: turn.tool_calls,
            model: "script".into(),
            usage: Usage::default(),
            ..Self::default()
        }
    }
}

/// A `RUMU_LLM=script:<path>` file: replies in prompt order. The server
/// cycles through them, so after the last reply the first is used again.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub turns: Vec<ScriptTurn>,
}

#[derive(Clone, Copy)]
enum Mode {
    /// Panic on exhaustion: a test whose script is too short must fail.
    Strict,
    /// Start over after the last reply (the server's mode).
    Loop,
}

pub struct ScriptedLlm {
    script: Mutex<VecDeque<LlmResponse>>,
    mode: Mode,
    seen: Mutex<Vec<Vec<Node>>>,
    chunk_delay: Duration,
}

impl ScriptedLlm {
    /// Replay `script` exactly once; the next call panics.
    pub fn strict(script: impl IntoIterator<Item = LlmResponse>) -> Self {
        Self::new(script, Mode::Strict)
    }

    /// Replay `script`, starting over after the last reply. An empty script
    /// yields a short fixed text reply so a server turn always finishes.
    pub fn looping(script: impl IntoIterator<Item = LlmResponse>) -> Self {
        Self::new(script, Mode::Loop)
    }

    fn new(script: impl IntoIterator<Item = LlmResponse>, mode: Mode) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            mode,
            seen: Mutex::new(vec![]),
            chunk_delay: Duration::ZERO,
        }
    }

    /// Pause between streamed chunks (zero by default, for tests).
    pub fn with_chunk_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = delay;
        self
    }

    /// The projections each completion was called with, in call order.
    pub fn projections(&self) -> Vec<Vec<Node>> {
        self.seen.lock().unwrap().clone()
    }

    fn next_response(&self) -> LlmResponse {
        let mut script = self.script.lock().unwrap();
        match self.mode {
            Mode::Strict => script
                .pop_front()
                .expect("scripted LLM exhausted (strict mode)"),
            Mode::Loop => match script.pop_front() {
                Some(response) => {
                    script.push_back(response.clone());
                    response
                }
                None => LlmResponse::text(EMPTY_REPLY),
            },
        }
    }
}

#[async_trait]
impl Llm for ScriptedLlm {
    async fn complete(
        &self,
        _system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError> {
        self.seen.lock().unwrap().push(nodes.to_vec());
        let response = self.next_response();
        if let Some(cb) = on_delta {
            let thought: String = response
                .reasoning
                .iter()
                .filter_map(|block| match block {
                    ReasoningBlock::Thinking { text, .. } => Some(text.as_str()),
                    ReasoningBlock::Redacted { .. } => None,
                })
                .collect();
            if !thought.is_empty() {
                stream(&cb, Delta::Reasoning, &thought, self.chunk_delay).await;
            }
            stream(&cb, Delta::Text, &response.text, self.chunk_delay).await;
        }
        Ok(response)
    }
}

/// Emit `text` in small chunks, optionally pausing between them.
async fn stream(cb: &OnDelta, delta: impl Fn(String) -> Delta, text: &str, delay: Duration) {
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(CHUNK_CHARS) {
        cb(delta(chunk.iter().collect()));
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}
