//! Model settings: providers, the custom models over them, the roles bound
//! to models, and the `model.use` marker a path picks its model with.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Structured;

/// A reasoning effort. Each model accepts a subset (see the adapter's
/// catalog); an effort it does not take is moved to the nearest one it does.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// The effort in `allowed` nearest to this one (the higher on a tie);
    /// `None` when `allowed` is empty.
    pub fn nearest(self, allowed: &[Effort]) -> Option<Effort> {
        allowed
            .iter()
            .copied()
            .min_by_key(|e| ((*e as i32 - self as i32).abs(), std::cmp::Reverse(*e)))
    }
}

/// A wire format an endpoint speaks.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Protocol {
    /// Anthropic messages. The endpoint is the API root without `/v1`, as
    /// `ANTHROPIC_BASE_URL` takes it.
    Anthropic,
    /// OpenAI chat completions, and every compatible endpoint.
    Openai,
    /// The OpenAI Responses API.
    OpenaiResponses,
    Gemini,
}

impl Protocol {
    pub const ALL: [Protocol; 4] = [
        Self::Anthropic,
        Self::Openai,
        Self::OpenaiResponses,
        Self::Gemini,
    ];
    /// What a model runs on after its native protocol, among those its
    /// provider has an endpoint for.
    pub const FALLBACK: [Protocol; 4] = [
        Self::Openai,
        Self::Anthropic,
        Self::OpenaiResponses,
        Self::Gemini,
    ];
}

/// One provider in `settings.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderSettings {
    /// The name the web shows, when not the preset's (or the id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A built-in provider, or a catalog one: its endpoints, and where its
    /// models are found in the catalog. A catalog one's endpoints are stored
    /// here whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Endpoints by protocol, over the preset's; an empty one turns the
    /// preset's off.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub endpoints: BTreeMap<Protocol, String>,
    /// Never leaves the server ([`ProviderInfo`] has only whether it is set).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// Which models are enabled: patterns (`claude-*`) and ids, listed or
    /// not, each excluding after a `!` (`!*-preview`, `!claude-old`); a rule
    /// naming a model wins over a pattern ([`rules_enable`]).
    pub models: Vec<String>,
    /// Model ids shown first in the picker.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<String>,
    /// Models held to one protocol instead of their native one.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub protocols: BTreeMap<String, Protocol>,
}

/// Whether `id` matches `rule`, where `*` matches any run of characters.
pub fn model_matches(rule: &str, id: &str) -> bool {
    let mut parts = rule.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = id.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for p in middle {
        match rest.find(p) {
            Some(i) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Whether `rules` enable `id`. A rule naming it decides (`!id` over
/// `id`); else a matching `!` pattern excludes it, and a matching pattern
/// enables it.
pub fn rules_enable(rules: &[String], id: &str) -> bool {
    if rules.iter().any(|r| r.strip_prefix('!') == Some(id)) {
        return false;
    }
    if rules.iter().any(|r| r == id) {
        return true;
    }
    let mut enabled = false;
    for r in rules.iter().filter(|r| r.contains('*')) {
        match r.strip_prefix('!') {
            Some(p) if model_matches(p, id) => return false,
            Some(_) => {}
            None => enabled |= model_matches(r, id),
        }
    }
    enabled
}

/// The models `rules` enable ([`rules_enable`]): of `listed`, then the
/// exact ids not listed, in rule order.
pub fn enabled_models(rules: &[String], listed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = listed
        .iter()
        .filter(|id| rules_enable(rules, id))
        .cloned()
        .collect();
    for r in rules {
        if !r.starts_with('!') && !r.contains('*') && !out.contains(r) && rules_enable(rules, r) {
            out.push(r.clone());
        }
    }
    out
}

/// The built-in provider whose models are the [`CustomModel`]s:
/// `rumu/<name>`. No configured provider may take its id.
pub const CUSTOM_PROVIDER: &str = "rumu";

/// A model of the built-in provider: another provider's model at an effort.
/// Sessions and roles name it, so pointing it at a newer model moves all of
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CustomModel {
    /// The model it routes to: `provider/model-id`, never a custom one.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<Effort>,
}

/// What a background job or a fresh path runs on, by model ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Role {
    /// Every path without a `model.use` marker.
    Default,
    /// Session titles. Unset keeps the truncated first prompt.
    Title,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct Roles {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
}

impl Roles {
    pub fn get(&self, role: Role) -> Option<&str> {
        match role {
            Role::Default => self.default.as_deref(),
            Role::Title => self.title.as_deref(),
        }
    }
}

/// Where session titles come from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Titles {
    /// The first prompt, truncated.
    #[default]
    First,
    /// The latest prompt, truncated.
    Last,
    /// Generated from the first prompt by the `title` role's model (the
    /// first prompt until it is done, or with no model bound).
    Model,
}

/// The model part of `RUMU_HOME/settings.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub providers: BTreeMap<String, ProviderSettings>,
    /// The built-in provider's models, by name.
    pub custom: BTreeMap<String, CustomModel>,
    pub roles: Roles,
    #[serde(skip_serializing_if = "is_default")]
    pub titles: Titles,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Split a model ref at its first `/`: `("openrouter", "anthropic/claude")`.
pub fn split_model_ref(r: &str) -> Option<(&str, &str)> {
    r.split_once('/')
        .filter(|(p, m)| !p.is_empty() && !m.is_empty())
}

/// Payload of the `model.use` marker (v2): the model this path runs on from
/// here on. Written only when a prompt is sent after the pick changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ModelUse {
    /// A model ref (`provider/model-id`); a custom one follows its edits.
    pub model: String,
    /// Overrides the custom model's effort (or sets the model's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<Effort>,
}

impl Structured for ModelUse {
    const TAG: &'static str = "model.use";
    const V: u32 = 2;
}

/// The model a call asks for.
#[derive(Debug, Clone, Copy)]
pub enum ModelPick<'a> {
    Role(Role),
    Use(&'a ModelUse),
}

/// A provider as the web sees it.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    /// The name set on it; absent while it shows the preset's (or the id).
    #[ts(optional)]
    pub custom_name: Option<String>,
    #[ts(optional)]
    pub preset: Option<String>,
    /// The endpoints in use, the preset's included.
    pub endpoints: BTreeMap<Protocol, String>,
    pub has_api_key: bool,
    /// The enabling rules ([`ProviderSettings::models`]).
    pub rules: Vec<String>,
    pub protocols: BTreeMap<String, Protocol>,
    /// The enabled models, pinned first.
    pub models: Vec<ModelInfo>,
    /// Every id the endpoints list, as last fetched.
    pub listed: Vec<String>,
    /// The listed models' names, by id.
    pub names: BTreeMap<String, String>,
    /// Why the last fetch failed.
    #[ts(optional)]
    pub list_error: Option<String>,
}

/// A model as the web sees it, with what the catalog knows about it.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ModelInfo {
    /// The model ref: `provider/model-id`.
    #[serde(rename = "ref")]
    #[ts(rename = "ref")]
    pub model_ref: String,
    pub id: String,
    pub name: String,
    /// The protocol it runs on; `None` when the provider has no endpoint it
    /// can use.
    pub protocol: Option<Protocol>,
    /// Its own protocol, which it runs on when not held to another and the
    /// provider has an endpoint for it.
    pub native: Option<Protocol>,
    pub pinned: bool,
    /// The efforts it takes; empty when it does not reason.
    pub efforts: Vec<Effort>,
    #[ts(type = "number | null")]
    pub context: Option<u64>,
    /// The most tokens it may write in one reply.
    #[ts(type = "number | null")]
    pub output: Option<u64>,
    pub image_input: bool,
    /// USD per million input / output tokens.
    pub cost: Option<ModelCost>,
    /// Whether the catalog lists it (otherwise the protocol's defaults).
    pub known: bool,
    /// A custom model's route; what is known is of the model it routes to.
    #[ts(optional)]
    pub route: Option<CustomModel>,
}

#[derive(Debug, Clone, Copy, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
}

/// A provider offered when adding one: the built-in ones first, then the
/// catalog's.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PresetInfo {
    pub id: String,
    pub name: String,
    pub endpoints: BTreeMap<Protocol, String>,
}

/// `settings.get`.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SettingsInfo {
    pub providers: Vec<ProviderInfo>,
    /// The built-in provider's models.
    pub custom: Vec<ModelInfo>,
    pub roles: Roles,
    pub titles: Titles,
    pub presets: Vec<PresetInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_effort() {
        use Effort::*;
        assert_eq!(XHigh.nearest(&[Low, Medium, High]), Some(High));
        assert_eq!(Minimal.nearest(&[None, Low]), Some(Low));
        assert_eq!(Max.nearest(&[]), Option::None);
        assert_eq!(Effort::parse("xhigh"), Some(XHigh));
        assert_eq!(serde_json::to_string(&XHigh).unwrap(), "\"xhigh\"");
    }

    #[test]
    fn model_refs_split_at_the_first_slash() {
        assert_eq!(
            split_model_ref("openrouter/anthropic/claude"),
            Some(("openrouter", "anthropic/claude"))
        );
        assert_eq!(split_model_ref("bare"), None);
        assert_eq!(split_model_ref("/x"), None);
    }

    #[test]
    fn rules_enable_matching_and_exact_ids() {
        assert!(model_matches("claude-*", "claude-opus-5"));
        assert!(model_matches("*opus*", "claude-opus-5"));
        assert!(model_matches("a*b*c", "a-x-b-y-c"));
        assert!(!model_matches("a*b*c", "a-c-b"));
        assert!(!model_matches("gpt-5", "gpt-5-mini"));
        assert!(model_matches("*", "anything"));
        let rules: Vec<String> = ["gpt-*", "custom", "gpt-5"].map(Into::into).into();
        let listed: Vec<String> = ["claude-x", "gpt-5", "gpt-5-mini"].map(Into::into).into();
        assert_eq!(
            enabled_models(&rules, &listed),
            ["gpt-5", "gpt-5-mini", "custom"]
        );
    }
}
