//! `ask`: questions to the user; and `delegate`, an `agent` call handed to
//! another assistant deck.
//!
//! The user gets up to four questions at once, each with optional options
//! to choose from, through a decision (`kind: "questions"`). Another deck gets
//! a task, with the user's approval (see [`crate::approve`]), done in a session
//! of its own; `allow_ask` in its `deck.yaml` grants it up front.

use async_trait::async_trait;
use rumu_cards::deck::{Action, DeckMount};
use rumu_cards::{Deck, Decks};
use rumu_core::{
    CardMount, Content, Cx, DELEGATE_TOOL, Delegation, Effort, Manifest, ToolError, ToolOutput,
    ToolResult, ToolResultExt, ToolSpec, tool_schema,
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
    /// The body of the pinned `instructions.md`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instructions: Option<String>,
    /// Deck ids that may ask it without a prompt.
    pub allow_ask: Vec<String>,
    /// Its `deck.yaml` default mounts.
    pub mounts: Vec<DeckMount>,
    /// The model its sessions start on.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<rumu_core::ModelUse>,
    /// The MCP servers its sessions use.
    pub mcp: rumu_core::McpUse,
    /// The rules enabling the skills its sessions see.
    pub skills: Vec<String>,
    /// The one sessions run as when none is named.
    pub default: bool,
    /// Its mascot; left out, the web draws one from the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cat: Option<rumu_cards::DeckCat>,
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
        instructions,
        allow_ask: deck.meta.allow_ask.clone(),
        mounts: deck.meta.mounts.clone(),
        mcp: deck.meta.mcp.clone(),
        skills: deck.meta.skills.clone(),
        model: deck.meta.model.clone(),
        default: decks.default_assistant() == deck.meta.id,
        cat: deck.meta.cat.clone(),
    }
}

/// `ask` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    /// up to four questions
    questions: Vec<Question>,
}

/// `delegate` arguments: an `agent` call naming another assistant.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    /// what it should do, self-contained: it sees nothing of this conversation
    task: String,
    /// the id of the assistant deck
    assistant: String,
    /// a model alias (`rumu/<name>`) to run on instead of its own model
    #[serde(default)]
    model: Option<String>,
    /// the reasoning effort to run at
    #[serde(default)]
    effort: Option<Effort>,
    /// refused but for `context: "none"`
    #[serde(default)]
    inherit: Option<Value>,
}

/// One question of an `ask`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Question {
    /// the question, self-contained
    question: String,
    /// a word or two naming the question
    #[serde(default, skip_serializing_if = "Option::is_none")]
    header: Option<String>,
    /// likely answers to choose from; they can always answer in their own words instead, so leave out an "other" option
    #[serde(default)]
    options: Vec<Choice>,
    /// whether several options may be chosen
    #[serde(default)]
    multiple: bool,
}

/// One option of a question to the user.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Choice {
    /// the option, in a few words
    label: String,
    /// what choosing it means, when the label alone does not say
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// The user's answers (`decision.answer`); no `answers` when they dismissed
/// the questions.
#[derive(Debug, Default, Deserialize)]
struct Reply {
    #[serde(default)]
    answers: Option<Vec<Answer>>,
}

/// The answer to one question: the labels chosen and/or their own words.
#[derive(Debug, Default, Deserialize)]
struct Answer {
    #[serde(default)]
    selected: Vec<String>,
    #[serde(default)]
    custom: Option<String>,
}

/// The most questions one `ask` may put to the user.
const MAX_QUESTIONS: usize = 4;
/// The most options one question may offer.
const MAX_OPTIONS: usize = 6;

/// Trim and check the questions for the user.
fn user_questions(questions: Vec<Question>) -> ToolResult<Vec<Question>> {
    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return Err(ToolError::model(format!(
            "ask: give the user 1 to {MAX_QUESTIONS} questions"
        )));
    }
    let trim = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    questions
        .into_iter()
        .map(|q| {
            let question = q.question.trim().to_owned();
            if question.is_empty() {
                return Err("ask: `question` must not be empty".into());
            }
            if q.options.len() > MAX_OPTIONS {
                return Err(ToolError::model(format!(
                    "ask: offer at most {MAX_OPTIONS} options per question"
                )));
            }
            let mut options: Vec<Choice> = Vec::with_capacity(q.options.len());
            for o in q.options {
                let label = o.label.trim().to_owned();
                if label.is_empty() {
                    return Err("ask: an option's `label` must not be empty".into());
                }
                if options.iter().any(|p| p.label == label) {
                    return Err(ToolError::model(format!(
                        "ask: the option `{label}` appears twice in one question"
                    )));
                }
                options.push(Choice {
                    label,
                    description: trim(o.description),
                });
            }
            Ok(Question {
                question,
                header: trim(q.header),
                multiple: q.multiple && !options.is_empty(),
                options,
            })
        })
        .collect()
}

/// The tool result for the user's answers: each question with its answer.
fn render_answers(questions: &[Question], answers: &[Answer]) -> String {
    let mut out = String::from("The user answers:");
    for (q, a) in questions.iter().zip(answers) {
        // Only offered labels count, in the order they were offered.
        let mut parts: Vec<String> = q
            .options
            .iter()
            .filter(|o| a.selected.contains(&o.label))
            .map(|o| o.label.clone())
            .collect();
        if !q.multiple {
            parts.truncate(1);
        }
        if let Some(custom) = a.custom.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            parts.push(if q.options.is_empty() {
                custom.to_owned()
            } else {
                format!("(in their own words) {custom}")
            });
        }
        let answer = if parts.is_empty() {
            "(no answer)".to_owned()
        } else {
            parts.join("; ")
        };
        out.push_str(&format!("\n\nQ: {}\nA: {answer}", q.question));
    }
    out
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
        let state = cx.state()?;
        let Some(from) = self.decks.get(&state.assistant) else {
            return Err(ToolError::model(format!(
                "ask: this session's assistant deck `{}` is gone",
                state.assistant
            )));
        };
        self.ask_user(from, args.questions, cx).await
    }

    /// `agent` naming another assistant: with the user's approval, it does
    /// the task in a session of its own, with its own mounts (and the
    /// project), cards it may write and its own model.
    async fn delegate(&self, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: DelegateArgs = serde_json::from_value(args.clone())
            .map_err(|e| ToolError::model(format!("agent: invalid arguments: {e}")))?;
        let state = cx.state()?;
        let from = state.assistant.clone();
        if state.delegated_by.is_some() {
            return Err("agent: you are doing another assistant's task; do it yourself".into());
        }
        if let Some(inherit) = &args.inherit
            && (inherit.get("context").is_some_and(|c| c != "none")
                || inherit.get("workspaces").is_some())
        {
            return Err("agent: another assistant sees only the task; leave `inherit` out".into());
        }
        let task = args.task.trim();
        if task.is_empty() {
            return Err("agent: `task` must not be empty".into());
        }
        let Some(from_deck) = self.decks.get(&from) else {
            return Err(ToolError::model(format!(
                "agent: this session's assistant deck `{from}` is gone"
            )));
        };
        if args.assistant == from {
            return Err("agent: that is you; leave `assistant` out".into());
        }
        let Some(to) = self
            .decks
            .get(&args.assistant)
            .filter(|d| d.assistant() && self.decks.visible(d.id(), &from, &state.mounts))
        else {
            let names: Vec<String> = self
                .decks
                .assistants()
                .into_iter()
                .filter(|d| d.id() != from && self.decks.visible(d.id(), &from, &state.mounts))
                .map(|d| format!("`{}` ({})", d.id(), d.meta.name))
                .collect();
            return Err(ToolError::model(format!(
                "agent: no assistant with id `{}` (assistants: {})",
                args.assistant,
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            )));
        };
        let request = json!({
            "kind": "ask",
            "from": from_deck.meta.name,
            "from_id": from,
            "to": to.meta.name,
            "to_id": to.id(),
            "task": task,
        });
        let allowed = crate::approve::approve(&self.decks, cx, Action::Ask, to.id(), request)
            .await
            .map_err(|e| ToolError::model(format!("agent: {e}")))?;
        if !allowed {
            return Err(ToolError::model(format!(
                "agent: the user declined to hand the task to `{}`",
                to.meta.name
            )));
        }
        let prompt = format!(
            "The assistant `{}` hands you this task, with the user's permission:\n\n{task}\n\n\
             Do the work, then reply with a concise summary: your final reply goes back to `{}` \
             as the result.",
            from_deck.meta.name, from_deck.meta.name
        );
        // Its own default mounts, and the project the task belongs to.
        let mut mounts: Vec<CardMount> = self.decks.default_mounts(to.id());
        if let Some(project) = &state.project {
            mounts.push(CardMount {
                deck: project.clone(),
                at: rumu_core::PROJECT_MOUNT.into(),
            });
        }
        mounts.sort_by(|a, b| a.at.cmp(&b.at));
        let reply = cx
            .delegate(Delegation {
                assistant: to.id().to_owned(),
                delegated_by: from.clone(),
                mounts,
                model: args.model,
                effort: args.effort,
                base: to.meta.model.clone(),
                prompt: Content::text(prompt),
            })
            .await
            .map_err(|e| ToolError::model(format!("agent: {e}")))?;
        let text = reply
            .text
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "(it stopped without a final reply)".into());
        Ok(ToolOutput::ok(format!(
            "`{}` reports:\n\n{text}\n\n[transcript: {}:/sessions/{}]",
            to.meta.name,
            rumu_core::RUMU_WORKSPACE,
            reply.session
        )))
    }

    async fn ask_user(
        &self,
        from: Deck,
        questions: Vec<Question>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let questions = user_questions(questions)?;
        let request = json!({
            "kind": "questions",
            "from": from.meta.name,
            "from_id": from.id(),
            "questions": questions,
        });
        let reply = cx
            .decide(request)
            .await
            .map_err(|e| ToolError::model(format!("ask: {e}")))?;
        let reply: Reply = serde_json::from_value(reply).unwrap_or_default();
        match reply.answers {
            Some(answers) if answers.len() == questions.len() => {
                Ok(ToolOutput::ok(render_answers(&questions, &answers)))
            }
            _ => Err("ask: the user dismissed the questions without answering".into()),
        }
    }
}

#[async_trait]
impl rumu_core::Plugin for AskPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "ask".into(),
            instructions: None,
            tools: vec![
                ToolSpec {
                    name: "ask".into(),
                    description: "Ask the user up to four questions at once and wait for the \
answers. Use it when you need a decision or a fact only the user has, not to ask leave for an \
action. Offer `options` when the likely answers are known (a short label, with a description \
when the label needs one; put a recommended option first); the user can always answer in their \
own words instead. To have another assistant do something, use `agent` with `assistant`.\n\n\
Answers are not saved: if one will matter later, write it into a card yourself."
                        .into(),
                    schema: tool_schema::<AskArgs>(),
                    background: false,
                    output_schema: None,
                    parallel: false,
                },
                // Routed from `agent`, never listed to the model.
                ToolSpec {
                    name: DELEGATE_TOOL.into(),
                    description: "`agent` for another assistant.".into(),
                    schema: tool_schema::<DelegateArgs>(),
                    background: true,
                    output_schema: None,
                    parallel: true,
                },
            ],
            hooks: vec![],
        }
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        match name {
            "ask" => self.ask(&args, cx).await.finish(),
            DELEGATE_TOOL => self.delegate(&args, cx).await.finish(),
            other => Ok(ToolOutput::err(format!(
                "plugin `ask` has no tool `{other}`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use rumu_core::Plugin;
    use serde_json::json;

    use super::AskPlugin;
    use crate::testutil::FakeRuntime;

    fn plugin(home: &std::path::Path) -> AskPlugin {
        crate::testutil::main_deck(home);
        AskPlugin::new(rumu_cards::Decks::load(home).unwrap())
    }

    #[tokio::test]
    async fn asks_the_user_and_reports_the_answers() {
        let home = tempfile::tempdir().unwrap();
        let plugin = plugin(home.path());
        let host = FakeRuntime::arc();
        host.answers.lock().unwrap().push_back(json!({ "answers": [
            { "selected": ["Postgres"] },
            { "selected": ["Search", "Auth", "Nope"], "custom": "realtime too" },
            { "custom": "next Friday" },
        ] }));
        let args = json!({ "questions": [
            { "question": "Which database?", "header": " DB ", "options": [
                { "label": "Postgres", "description": "the default" },
                { "label": "SQLite" },
            ] },
            { "question": "Which features?", "multiple": true, "options": [
                { "label": "Auth" }, { "label": "Search" },
            ] },
            { "question": "When is the deadline?", "multiple": true },
        ] });
        let out = plugin.call_tool("ask", args, &host.cx()).await.unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        assert_eq!(
            out.content.text,
            "The user answers:\
             \n\nQ: Which database?\nA: Postgres\
             \n\nQ: Which features?\nA: Auth; Search; (in their own words) realtime too\
             \n\nQ: When is the deadline?\nA: next Friday"
        );
        let request = host.decisions.lock().unwrap()[0].clone();
        assert_eq!(request["kind"], "questions");
        assert_eq!(request["questions"][0]["header"], "DB");
        assert_eq!(
            request["questions"][0]["options"][1],
            json!({ "label": "SQLite" })
        );
        // `multiple` without options means nothing.
        assert_eq!(request["questions"][2]["multiple"], false);
    }

    #[tokio::test]
    async fn a_dismissal_is_an_error_for_the_model() {
        let home = tempfile::tempdir().unwrap();
        let plugin = plugin(home.path());
        let host = FakeRuntime::arc();
        host.answers.lock().unwrap().push_back(json!({}));
        let args = json!({ "questions": [{ "question": "Ship it?" }] });
        let out = plugin.call_tool("ask", args, &host.cx()).await.unwrap();
        assert!(
            out.is_error && out.content.text.contains("dismissed"),
            "{}",
            out.content.text
        );
    }

    #[tokio::test]
    async fn malformed_questions_never_reach_the_user() {
        let home = tempfile::tempdir().unwrap();
        let plugin = plugin(home.path());
        let host = FakeRuntime::arc();
        let q = |n: usize| json!({ "question": format!("q{n}") });
        let cases = [
            json!({ "questions": [] }),
            json!({ "questions": (0..5).map(q).collect::<Vec<_>>() }),
            json!({ "questions": [{ "question": "  " }] }),
            json!({ "questions": [{ "question": "x", "options": [
                { "label": "a" }, { "label": " a" },
            ] }] }),
            // An assistant takes one plain question.
        ];
        for args in cases {
            let out = plugin
                .call_tool("ask", args.clone(), &host.cx())
                .await
                .unwrap();
            assert!(out.is_error, "accepted {args}");
        }
        assert!(host.decisions.lock().unwrap().is_empty());
    }
}
