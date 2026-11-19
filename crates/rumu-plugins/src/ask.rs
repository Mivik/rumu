//! `ask`: questions to the user.
//!
//! The user gets up to four questions at once, each with optional options
//! to choose from, through a decision (`kind: "questions"`).
//!
//! Questions are withdrawn after a while (`timeout`) so that a user away from
//! the screen does not stall the turn, unless the user has started on them.

use async_trait::async_trait;
use rumu_cards::deck::DeckMount;
use rumu_cards::{Deck, Decks};
use rumu_core::{
    Cx, Manifest, ToolError, ToolOutput, ToolResult, ToolResultExt, ToolSpec, tool_schema,
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
    /// The model its sessions run on, until one picks another.
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
    /// seconds to wait before going on without an answer, default 600; 0 waits until they answer. Once the user starts answering, it waits for them
    #[serde(default)]
    timeout: Option<u64>,
}

/// How long `ask` waits for the user by default, and an approval at all
/// (seconds).
pub(crate) const ASK_TIMEOUT: u64 = 600;

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
            } else if !q.multiple && !parts.is_empty() {
                format!("(their note) {custom}")
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
        let timeout = match args.timeout.unwrap_or(ASK_TIMEOUT) {
            0 => None,
            secs => Some(std::time::Duration::from_secs(secs)),
        };
        self.ask_user(from, args.questions, timeout, cx).await
    }

    async fn ask_user(
        &self,
        from: Deck,
        questions: Vec<Question>,
        timeout: Option<std::time::Duration>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let questions = user_questions(questions)?;
        let request = json!({
            "kind": "questions",
            "from": from.meta.name,
            "from_id": from.id(),
            "questions": questions,
        });
        let Some(reply) = cx
            .decide(request, timeout)
            .await
            .map_err(|e| ToolError::model(format!("ask: {e}")))?
        else {
            let mins = timeout.map_or(0, |t| t.as_secs().div_ceil(60));
            return Ok(ToolOutput::ok(format!(
                "The user did not answer within {mins} min, so the questions were withdrawn. \
                 Where a wrong guess is cheap to undo, go on with your best judgment and say what \
                 you assumed; where it is not, stop and leave the question for when they are back."
            )));
        };
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
own words instead. If nobody answers within `timeout`, you get told and go on without them. To have another assistant do something, `invoke` `session.create` on `@rumu:/sessions`.\n\n\
Answers are not saved: if one will matter later, write it into a card yourself."
                        .into(),
                    schema: tool_schema::<AskArgs>(),
                    background: false,
                    output_schema: None,
                    parallel: false,
                },
            ],
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
            { "selected": ["Postgres"], "custom": "with pgvector" },
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
             \n\nQ: Which database?\nA: Postgres; (their note) with pgvector\
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
    async fn the_model_hears_when_nobody_answered() {
        let home = tempfile::tempdir().unwrap();
        let plugin = plugin(home.path());
        let host = FakeRuntime::arc();
        host.answers
            .lock()
            .unwrap()
            .push_back(serde_json::Value::Null);
        let args = json!({ "questions": [{ "question": "Ship it?" }] });
        let out = plugin.call_tool("ask", args, &host.cx()).await.unwrap();
        assert!(
            !out.is_error && out.content.text.contains("did not answer within 10 min"),
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
