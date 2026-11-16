//! A session's config options: the assistant (until the first prompt), the
//! model and the reasoning effort. The model and effort ride on every prompt
//! (rumu records a change where it happens).

use agent_client_protocol::v1::schema::{
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
};
use rumu_core::Effort;
use serde_json::{Value, json};

use super::Rumu;

/// The option value standing for "the assistant's own".
const DEFAULT: &str = "default";

/// What the client picked.
#[derive(Debug, Clone, Default)]
pub(super) struct Config {
    /// The assistant deck id; the default assistant when `None`.
    pub assistant: Option<String>,
    /// A model ref (`provider/model`); the assistant's model when `None`.
    pub model: Option<String>,
    pub effort: Option<Effort>,
    /// The assistant's own model ref, which an effort alone applies to.
    pub base: Option<String>,
}

impl Config {
    /// The `model` param of a `prompt`.
    pub fn choice(&self) -> Value {
        match (&self.model, self.effort) {
            (Some(model), effort) => json!({ "model": model, "effort": effort }),
            (None, Some(effort)) => match &self.base {
                Some(model) => json!({ "model": model, "effort": effort }),
                None => Value::Null,
            },
            (None, None) => Value::Null,
        }
    }
}

/// The assistants (id, name, model ref, default) and the models (ref, name)
/// to choose from.
pub(super) struct Choices {
    assistants: Vec<(String, String, Option<String>, bool)>,
    models: Vec<(String, String)>,
}

impl Choices {
    pub async fn load(rumu: &Rumu) -> Result<Self, String> {
        let list = rumu.call("assistant.list", json!({})).await?;
        let assistants = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some((
                    a.get("id")?.as_str()?.to_owned(),
                    a.get("name")?.as_str()?.to_owned(),
                    a.pointer("/model/model")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    a.get("default").and_then(Value::as_bool) == Some(true),
                ))
            })
            .collect();
        let settings = rumu.call("settings.get", json!({})).await?;
        let model = |m: &Value| {
            Some((
                m.get("ref")?.as_str()?.to_owned(),
                m.get("name")?.as_str()?.to_owned(),
            ))
        };
        let mut models: Vec<(String, String)> = settings
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(model)
            .collect();
        for p in settings
            .get("providers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            models.extend(
                p.get("models")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(model),
            );
        }
        Ok(Self { assistants, models })
    }

    /// The deck id `config` runs as.
    fn assistant<'a>(&'a self, config: &'a Config) -> Option<&'a str> {
        config
            .assistant
            .as_deref()
            .or_else(|| self.assistants.iter().find(|a| a.3).map(|a| a.0.as_str()))
    }

    /// Fill in the assistant's own model.
    pub fn resolve(&self, config: &mut Config) {
        let id = self.assistant(config).map(str::to_owned);
        config.base = self
            .assistants
            .iter()
            .find(|a| Some(&a.0) == id.as_ref())
            .and_then(|a| a.2.clone());
    }

    /// The options as `config` stands.
    pub fn options(&self, config: &Config) -> Vec<SessionConfigOption> {
        let assistants: Vec<_> = self
            .assistants
            .iter()
            .map(|a| SessionConfigSelectOption::new(a.0.clone(), a.1.clone()))
            .collect();
        let own = match &config.base {
            Some(base) => format!("Assistant's model ({base})"),
            None => "Assistant's model".to_owned(),
        };
        let mut models = vec![SessionConfigSelectOption::new(DEFAULT, own)];
        models.extend(self.models.iter().map(|(r, name)| {
            SessionConfigSelectOption::new(r.clone(), name.clone()).description(r.clone())
        }));
        let mut efforts = vec![SessionConfigSelectOption::new(DEFAULT, "Model's default")];
        efforts.extend(Effort::ALL.iter().map(|e| {
            let id = effort_id(*e);
            SessionConfigSelectOption::new(id.clone(), id)
        }));
        vec![
            SessionConfigOption::select(
                "assistant",
                "Assistant",
                self.assistant(config).unwrap_or_default().to_owned(),
                assistants,
            )
            .description("Who answers; fixed once the session has a prompt"),
            SessionConfigOption::select(
                "model",
                "Model",
                config.model.clone().unwrap_or_else(|| DEFAULT.into()),
                models,
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "effort",
                "Reasoning effort",
                config.effort.map_or_else(|| DEFAULT.into(), effort_id),
                efforts,
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ]
    }

    /// Check a model or effort pick and apply it to `config`. The assistant
    /// is the caller's: changing it replaces the session.
    pub fn apply(&self, config: &mut Config, id: &str, value: &str) -> Result<(), String> {
        match id {
            "model" if value == DEFAULT => config.model = None,
            "model" if self.models.iter().any(|(r, _)| r == value) => {
                config.model = Some(value.to_owned());
            }
            "model" => return Err(format!("unknown model `{value}`")),
            "effort" if value == DEFAULT => config.effort = None,
            "effort" => {
                config.effort =
                    Some(Effort::parse(value).ok_or_else(|| format!("unknown effort `{value}`"))?);
            }
            "assistant" if self.assistants.iter().any(|a| a.0 == value) => {
                config.assistant = Some(value.to_owned());
                self.resolve(config);
            }
            "assistant" => return Err(format!("unknown assistant `{value}`")),
            _ => return Err(format!("unknown config option `{id}`")),
        }
        Ok(())
    }
}

fn effort_id(effort: Effort) -> String {
    serde_json::to_value(effort)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
