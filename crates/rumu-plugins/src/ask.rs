//! `ask`: one assistant deck putting a question to another, with the user's
//! approval (see [`crate::approve`]), answered in a session of the asked
//! deck. `allow_ask` in the asked deck's `deck.yaml` grants it up front.

use async_trait::async_trait;
use rumu_cards::deck::{Action, DeckMount};
use rumu_cards::{Deck, Decks};
use rumu_core::{
    CardMount, Consult, Content, Cx, Manifest, ToolError, ToolOutput, ToolResult, ToolResultExt,
    ToolSpec, tool_schema,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One assistant deck as the web roster sees it.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Assistant {
    pub id: String,
    pub name: String,
    /// Its `deck.yaml` workspaces; the first is the default.
    pub workspaces: Vec<String>,
    /// The body of the pinned `instructions.md`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instructions: Option<String>,
    /// Deck ids that may ask it without a prompt.
    pub allow_ask: Vec<String>,
    /// Its `deck.yaml` default mounts.
    pub mounts: Vec<DeckMount>,
    /// The one sessions run as when none is named.
    pub default: bool,
}

/// Build the wire form of one assistant deck.
pub fn assistant_of(decks: &Decks, deck: &Deck) -> Assistant {
    let instructions = deck
        .read("instructions")
        .ok()
        .flatten()
        .map(|text| rumu_cards::card::parse_card("instructions", &text).body)
        .filter(|b| !b.trim().is_empty());
    Assistant {
        id: deck.meta.id.clone(),
        name: deck.meta.name.clone(),
        workspaces: deck.meta.workspaces.clone(),
        instructions,
        allow_ask: deck.meta.allow_ask.clone(),
        mounts: deck.meta.mounts.clone(),
        default: decks.default_assistant() == deck.meta.id,
    }
}

/// `ask` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    /// the id of the assistant deck to ask
    to: String,
    /// the question, self-contained: the other deck sees nothing of this conversation
    question: String,
}

/// The plugin behind `ask`.
pub struct AskPlugin {
    decks: Decks,
}

impl AskPlugin {
    pub fn new(decks: Decks) -> Self {
        Self { decks }
    }

    async fn ask(&self, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: AskArgs = serde_json::from_value(args.clone())
            .map_err(|e| ToolError::model(format!("ask: invalid arguments: {e}")))?;
        let question = args.question.trim();
        if question.is_empty() {
            return Err("ask: `question` must not be empty".into());
        }
        let state = cx.state()?;
        let from = state.assistant.clone();
        if state.asked_by.is_some() {
            return Err("ask: you are answering a question yourself; answer it directly".into());
        }
        let Some(from_deck) = self.decks.get(&from) else {
            return Err(ToolError::model(format!(
                "ask: this session's assistant deck `{from}` is gone"
            )));
        };
        let Some(to) = self.decks.get(&args.to).filter(Deck::assistant) else {
            let names: Vec<String> = self
                .decks
                .assistants()
                .into_iter()
                .filter(|d| d.id() != from)
                .map(|d| format!("`{}` ({})", d.id(), d.meta.name))
                .collect();
            return Err(ToolError::model(format!(
                "ask: no assistant with id `{}` (assistants you can ask: {})",
                args.to,
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            )));
        };
        if to.id() == from {
            return Err("ask: you cannot ask yourself".into());
        }
        let request = json!({
            "kind": "ask",
            "from": from_deck.meta.name,
            "from_id": from,
            "to": to.meta.name,
            "to_id": to.id(),
            "question": question,
        });
        let allowed = crate::approve::approve(&self.decks, cx, Action::Ask, to.id(), request)
            .await
            .map_err(|e| ToolError::model(format!("ask: {e}")))?;
        if !allowed {
            return Err(ToolError::model(format!(
                "ask: the user declined to let you ask `{}`",
                to.meta.name
            )));
        }
        let prompt = format!(
            "The assistant `{}` asks you, with the user's permission:\n\n{question}\n\n\
             Answer from what you know and your cards. Your reply goes back to `{}` as \
             your answer; you cannot change your cards here.",
            from_deck.meta.name, from_deck.meta.name
        );
        // The asked deck's default mounts; nothing is written in the session.
        let mut mounts: Vec<CardMount> = self.decks.default_mounts(to.id());
        mounts.sort_by(|a, b| a.at.cmp(&b.at));
        let reply = cx
            .consult(Consult {
                assistant: to.id().to_owned(),
                workspaces: to.meta.workspaces.clone(),
                asked_by: from.clone(),
                mounts,
                prompt: Content::text(prompt),
            })
            .await
            .map_err(|e| ToolError::model(format!("ask: {e}")))?;
        let Some(text) = reply.text.filter(|t| !t.trim().is_empty()) else {
            return Err(ToolError::model(format!(
                "ask: `{}` gave no answer",
                to.meta.name
            )));
        };
        Ok(ToolOutput::ok(format!(
            "`{}` answers:\n\n{text}",
            to.meta.name
        )))
    }
}

#[async_trait]
impl rumu_core::Plugin for AskPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "ask".into(),
            instructions: None,
            tools: vec![ToolSpec {
                name: "ask".into(),
                description: "Ask another assistant a question; it answers from its own memory, \
which you cannot read. The user approves each ask unless they chose to always allow it. The \
answer is not saved: if it will matter later, write it into a card yourself."
                    .into(),
                schema: tool_schema::<AskArgs>(),
                background: false,
                output_schema: None,
                parallel: false,
            }],
            hooks: vec![],
        }
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        match name {
            "ask" => self.ask(&args, cx).await.finish(),
            other => Ok(ToolOutput::err(format!(
                "plugin `ask` has no tool `{other}`"
            ))),
        }
    }
}
