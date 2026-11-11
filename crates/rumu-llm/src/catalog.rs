//! The model catalog, parsed from <https://models.dev/api.json>: per model
//! the efforts, context, modalities and price, and the protocol its vendor
//! serves it on. Only what rumu uses is kept; models that cannot call tools
//! are dropped.

use std::collections::{BTreeMap, HashMap};

use rumu_core::{Effort, ModelCost, Protocol};
use serde::Deserialize;

use crate::presets::first_party;

/// Efforts assumed for a model the catalog does not list.
pub fn default_efforts(protocol: Protocol) -> Vec<Effort> {
    use Effort::*;
    match protocol {
        Protocol::Anthropic => vec![Low, Medium, High, XHigh, Max],
        Protocol::Openai | Protocol::OpenaiResponses => vec![None, Low, Medium, High],
        Protocol::Gemini => vec![Low, Medium, High],
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    providers: BTreeMap<String, CatalogProvider>,
    /// The providers listing each model id, in id order: an id nothing
    /// lists is answered without a scan.
    index: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct CatalogProvider {
    pub name: String,
    /// Where it serves a protocol rumu speaks, when at a plain address (not
    /// a template, nor behind an SDK of its own).
    pub endpoint: Option<(Protocol, String)>,
    /// The protocol its own SDK speaks; `None` for an auth scheme of its own
    /// (Vertex, Bedrock, Azure).
    pub protocol: Option<Protocol>,
    pub models: BTreeMap<String, CatalogModel>,
}

#[derive(Debug, Clone)]
pub struct CatalogModel {
    pub name: String,
    pub efforts: Vec<Effort>,
    pub context: Option<u64>,
    pub output: Option<u64>,
    pub image_input: bool,
    pub cost: Option<ModelCost>,
}

// -- models.dev wire shapes (only the fields read).

#[derive(Deserialize)]
struct RawProvider {
    #[serde(default)]
    name: String,
    #[serde(default)]
    npm: String,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    models: BTreeMap<String, RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    #[serde(default)]
    name: String,
    #[serde(default)]
    tool_call: bool,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    reasoning_options: Option<Vec<RawReasoning>>,
    #[serde(default)]
    modalities: Option<RawModalities>,
    #[serde(default)]
    limit: Option<RawLimit>,
    #[serde(default)]
    cost: Option<RawCost>,
}

#[derive(Deserialize)]
struct RawReasoning {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    values: Vec<String>,
}

#[derive(Deserialize)]
struct RawModalities {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Deserialize)]
struct RawLimit {
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    output: Option<u64>,
}

#[derive(Deserialize)]
struct RawCost {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
}

fn protocol_of(npm: &str) -> Option<Protocol> {
    match npm {
        "@ai-sdk/anthropic" => Some(Protocol::Anthropic),
        "@ai-sdk/google" => Some(Protocol::Gemini),
        "@ai-sdk/openai" => Some(Protocol::OpenaiResponses),
        n if n.starts_with("@ai-sdk/google-vertex")
            || n == "@ai-sdk/amazon-bedrock"
            || n == "@ai-sdk/azure" =>
        {
            None
        }
        _ => Some(Protocol::Openai),
    }
}

/// The endpoint of a provider served at `api` by SDK `npm`; Anthropic roots
/// are kept without `/v1`.
fn endpoint_of(npm: &str, api: Option<&str>) -> Option<(Protocol, String)> {
    let api = api?.trim().trim_end_matches('/');
    if api.is_empty() || api.contains("${") {
        return None;
    }
    Some(match npm {
        "@ai-sdk/openai-compatible" => (Protocol::Openai, api.to_owned()),
        "@ai-sdk/openai" => (Protocol::OpenaiResponses, api.to_owned()),
        "@ai-sdk/anthropic" => (
            Protocol::Anthropic,
            api.strip_suffix("/v1").unwrap_or(api).to_owned(),
        ),
        _ => return None,
    })
}

/// The native protocol of a model the catalog does not place, by family.
fn native_by_name(id: &str) -> Option<Protocol> {
    let id = id.to_ascii_lowercase();
    let o_series = id.len() > 1 && id.starts_with('o') && id.as_bytes()[1].is_ascii_digit();
    if id.starts_with("claude") {
        Some(Protocol::Anthropic)
    } else if id.starts_with("gemini") {
        Some(Protocol::Gemini)
    } else if id.starts_with("gpt-") || id.starts_with("codex") || o_series {
        Some(Protocol::OpenaiResponses)
    } else {
        None
    }
}

/// The id without a vendor prefix (`anthropic/claude-x` is `claude-x`).
fn bare(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

/// `aion-3.0-mini` as "Aion 3.0 Mini", `gpt-x` as "GPT X".
fn words(s: &str) -> String {
    s.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            if ["gpt", "glm"].contains(&w) {
                return w.to_ascii_uppercase();
            }
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn efforts_of(m: &RawModel) -> Vec<Effort> {
    if !m.reasoning {
        return Vec::new();
    }
    let opts = m.reasoning_options.as_deref().unwrap_or_default();
    if let Some(e) = opts.iter().find(|o| o.kind == "effort") {
        let mut v: Vec<Effort> = e.values.iter().filter_map(|s| Effort::parse(s)).collect();
        v.sort();
        v.dedup();
        return v;
    }
    if opts.iter().any(|o| o.kind == "toggle") {
        return vec![Effort::None, Effort::High];
    }
    // Reasons, but takes no effort (only a token budget, or always on).
    Vec::new()
}

impl Catalog {
    /// Parse `api.json`.
    pub fn parse(json: &[u8]) -> Result<Self, serde_json::Error> {
        let raw: BTreeMap<String, RawProvider> = serde_json::from_slice(json)?;
        let providers = raw
            .into_iter()
            .map(|(id, p)| {
                let models = p
                    .models
                    .into_iter()
                    .filter(|(_, m)| m.tool_call)
                    .map(|(mid, m)| {
                        let model = CatalogModel {
                            name: if m.name.is_empty() {
                                mid.clone()
                            } else {
                                m.name.clone()
                            },
                            efforts: efforts_of(&m),
                            context: m.limit.as_ref().and_then(|l| l.context),
                            output: m.limit.as_ref().and_then(|l| l.output),
                            image_input: m
                                .modalities
                                .as_ref()
                                .is_some_and(|x| x.input.iter().any(|i| i == "image")),
                            cost: m.cost.as_ref().map(|c| ModelCost {
                                input: c.input,
                                output: c.output,
                            }),
                        };
                        (mid, model)
                    })
                    .collect();
                let provider = CatalogProvider {
                    name: if p.name.is_empty() {
                        id.clone()
                    } else {
                        p.name
                    },
                    endpoint: endpoint_of(&p.npm, p.api.as_deref()),
                    protocol: protocol_of(&p.npm),
                    models,
                };
                (id, provider)
            })
            .collect::<BTreeMap<String, CatalogProvider>>();
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        for (id, p) in &providers {
            for m in p.models.keys() {
                index.entry(m.clone()).or_default().push(id.clone());
            }
        }
        Ok(Self { providers, index })
    }

    /// Provider `id`.
    pub fn provider(&self, id: &str) -> Option<&CatalogProvider> {
        self.providers.get(id)
    }

    /// The providers that can be added as they are: at an endpoint, with a
    /// model that can call tools.
    pub fn offered(&self) -> impl Iterator<Item = (&str, &CatalogProvider)> {
        self.providers
            .iter()
            .filter(|(_, p)| p.endpoint.is_some() && !p.models.is_empty())
            .map(|(id, p)| (id.as_str(), p))
    }

    /// What the catalog knows about model `id`, from the first of these
    /// that lists it (as is, or without a vendor prefix): `home`, the
    /// first-party providers, any provider.
    pub fn lookup(&self, home: Option<&str>, id: &str) -> Option<&CatalogModel> {
        let listing = |key: &str| self.index.get(key).map(Vec::as_slice).unwrap_or_default();
        // Of any provider, the first (in id order) that lists it.
        let any = listing(id)
            .iter()
            .chain(listing(bare(id)))
            .map(String::as_str)
            .min()?;
        let order = home.into_iter().chain(first_party()).chain([any]);
        self.find(order, id).map(|(_, m)| m)
    }

    /// A name for model `id`: the catalog's; else that of a model it extends,
    /// the rest in words (`gpt-x-fast` is "GPT X Fast"); else its id in
    /// words.
    pub fn name(&self, home: Option<&str>, id: &str) -> String {
        // A catalog name that is only the id is none.
        let named = |id: &str| {
            self.lookup(home, id)
                .map(|m| m.name.as_str())
                .filter(|n| *n != id && *n != bare(id))
        };
        if let Some(n) = named(id) {
            return n.to_owned();
        }
        let start = id.len() - bare(id).len();
        let mut cut = id.len();
        while let Some(i) = id[start..cut].rfind('-') {
            cut = start + i;
            if let Some(n) = named(&id[..cut]) {
                return format!("{n} {}", words(&id[cut + 1..]));
            }
        }
        words(bare(id))
    }

    /// The protocol model `id`'s vendor serves it on: from the first-party
    /// provider that lists it, else by its family.
    pub fn native(&self, id: &str) -> Option<Protocol> {
        self.find(first_party(), id)
            .and_then(|(p, _)| p.protocol)
            .or_else(|| native_by_name(bare(id)))
    }

    fn find<'s, 'a>(
        &'s self,
        order: impl Iterator<Item = &'a str>,
        id: &str,
    ) -> Option<(&'s CatalogProvider, &'s CatalogModel)> {
        for p in order.filter_map(|p| self.providers.get(p)) {
            if let Some(m) = p.models.get(id).or_else(|| p.models.get(bare(id))) {
                return Some((p, m));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "anthropic": {"npm":"@ai-sdk/anthropic","models":{
        "claude-x": {"name":"Claude X","tool_call":true,"reasoning":true,
          "reasoning_options":[{"type":"effort","values":["high","low","medium"]}],
          "modalities":{"input":["text","image"]},"limit":{"context":200000},
          "cost":{"input":5,"output":25}},
        "claude-embed": {"name":"E","tool_call":false}
      }},
      "openai": {"npm":"@ai-sdk/openai","models":{"gpt-x": {"tool_call":true}}},
      "minimax": {"npm":"@ai-sdk/anthropic","models":{"MiniMax-M9": {"tool_call":true}}},
      "deepseek": {"npm":"@ai-sdk/openai-compatible","models":{
        "r": {"name":"R","tool_call":true,"reasoning":true,"reasoning_options":[{"type":"toggle"}]}
      }},
      "openrouter": {"npm":"@openrouter/ai-sdk-provider","models":{
        "anthropic/claude-x": {"name":"Claude X via OR","tool_call":true}
      }},
      "aggregator": {"npm":"@ai-sdk/openai-compatible","models":{"glm-z": {"tool_call":true}}}
    }"#;

    #[test]
    fn looks_models_up_and_places_their_protocol() {
        let c = Catalog::parse(SAMPLE.as_bytes()).unwrap();
        let m = c.lookup(None, "claude-x").unwrap();
        assert_eq!(m.efforts, [Effort::Low, Effort::Medium, Effort::High]);
        assert_eq!((m.context, m.image_input), (Some(200_000), true));
        assert!(c.lookup(None, "claude-embed").is_none());
        assert_eq!(
            c.lookup(None, "r").unwrap().efforts,
            [Effort::None, Effort::High]
        );

        // The home provider first; a vendor prefix is looked through.
        assert_eq!(
            c.lookup(Some("openrouter"), "anthropic/claude-x")
                .unwrap()
                .name,
            "Claude X via OR"
        );
        assert_eq!(c.lookup(None, "vendor/claude-x").unwrap().name, "Claude X");
        // Any provider, as a last resort.
        assert!(c.lookup(None, "glm-z").is_some());

        assert_eq!(c.native("anthropic/claude-x"), Some(Protocol::Anthropic));
        assert_eq!(c.native("gpt-x"), Some(Protocol::OpenaiResponses));
        assert_eq!(c.native("MiniMax-M9"), Some(Protocol::Anthropic));
        assert_eq!(c.native("r"), Some(Protocol::Openai));
        // Not first-party: by family, else none.
        assert_eq!(c.native("glm-z"), None);
        assert_eq!(c.native("claude-new"), Some(Protocol::Anthropic));
        assert_eq!(c.native("o5-mini"), Some(Protocol::OpenaiResponses));
        assert_eq!(c.native("gemini-9"), Some(Protocol::Gemini));

        // Names: the catalog's, extended, or the id's words.
        assert_eq!(c.name(None, "g/claude-x"), "Claude X");
        assert_eq!(c.name(None, "g/claude-x-fast-1"), "Claude X Fast 1");
        assert_eq!(c.name(None, "or/aion-labs/aion-3.0-mini"), "Aion 3.0 Mini");
        assert_eq!(c.name(None, "gpt-x"), "GPT X");
    }
}
