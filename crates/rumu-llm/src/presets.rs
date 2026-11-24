//! The built-in providers: their endpoints, and the catalog provider their
//! model metadata is read from. They are offered first; the catalog's other
//! providers are offered after them, a preset id then naming one of those.

use genai::adapter::AdapterKind;
use rumu_core::Protocol::{self, *};

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// The models.dev provider id.
    pub catalog: &'static str,
    /// Serves other vendors' models (`vendor/model` ids).
    pub router: bool,
    /// The adapter each protocol's requests are shaped by, and its address.
    /// The adapter must speak the protocol: an OpenAI-family one posts
    /// `chat/completions`, an Anthropic one `messages`.
    pub endpoints: &'static [(Protocol, AdapterKind, &'static str)],
}

/// A vendor's own adapter is a refinement of the protocol, not a replacement
/// for it: in genai they are almost all pass-throughs to the OpenAI one
/// (which is why an OpenAI row may name Mistral, Aliyun, …), while
/// `MiniMax` speaks Anthropic, so `Anthropic` rows name
/// `AdapterKind::Anthropic`. `Zai` is never named: it replaces the endpoint
/// with api.z.ai, so an address of the provider's own (the coding plan's, say)
/// would be ignored. The test below checks both.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "anthropic",
        name: "Anthropic",
        catalog: "anthropic",
        router: false,
        endpoints: &[(
            Anthropic,
            AdapterKind::Anthropic,
            "https://api.anthropic.com",
        )],
    },
    Preset {
        id: "openai",
        name: "OpenAI",
        catalog: "openai",
        router: false,
        endpoints: &[
            (
                OpenaiResponses,
                AdapterKind::OpenAIResp,
                "https://api.openai.com/v1",
            ),
            (Openai, AdapterKind::OpenAI, "https://api.openai.com/v1"),
        ],
    },
    Preset {
        id: "google",
        name: "Google",
        catalog: "google",
        router: false,
        endpoints: &[
            (
                Gemini,
                AdapterKind::Gemini,
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            (
                Openai,
                AdapterKind::OpenAI,
                "https://generativelanguage.googleapis.com/v1beta/openai",
            ),
        ],
    },
    Preset {
        id: "openrouter",
        name: "OpenRouter",
        catalog: "openrouter",
        router: true,
        endpoints: &[
            (
                Openai,
                AdapterKind::OpenRouter,
                "https://openrouter.ai/api/v1",
            ),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://openrouter.ai/api",
            ),
        ],
    },
    Preset {
        id: "zenmux",
        name: "ZenMux",
        catalog: "zenmux",
        router: true,
        endpoints: &[
            (Openai, AdapterKind::OpenAI, "https://zenmux.ai/api/v1"),
            (
                OpenaiResponses,
                AdapterKind::OpenAIResp,
                "https://zenmux.ai/api/v1",
            ),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://zenmux.ai/api/anthropic",
            ),
        ],
    },
    Preset {
        id: "requesty",
        name: "Requesty",
        catalog: "requesty",
        router: true,
        endpoints: &[
            (
                Openai,
                AdapterKind::Requesty,
                "https://router.requesty.ai/v1",
            ),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://router.requesty.ai",
            ),
        ],
    },
    Preset {
        id: "deepseek",
        name: "DeepSeek",
        catalog: "deepseek",
        router: false,
        endpoints: &[
            (Openai, AdapterKind::DeepSeek, "https://api.deepseek.com"),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://api.deepseek.com/anthropic",
            ),
        ],
    },
    Preset {
        id: "moonshot-cn",
        name: "Moonshot AI (China)",
        catalog: "moonshotai-cn",
        router: false,
        endpoints: &[
            (Openai, AdapterKind::Moonshot, "https://api.moonshot.cn/v1"),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://api.moonshot.cn/anthropic",
            ),
        ],
    },
    Preset {
        id: "moonshot",
        name: "Moonshot AI",
        catalog: "moonshotai",
        router: false,
        endpoints: &[
            (Openai, AdapterKind::Moonshot, "https://api.moonshot.ai/v1"),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://api.moonshot.ai/anthropic",
            ),
        ],
    },
    Preset {
        id: "zhipu",
        name: "Zhipu AI",
        catalog: "zhipuai",
        router: false,
        endpoints: &[
            (
                Openai,
                AdapterKind::OpenAI,
                "https://open.bigmodel.cn/api/paas/v4",
            ),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://open.bigmodel.cn/api/anthropic",
            ),
        ],
    },
    Preset {
        id: "zai",
        name: "Z.AI",
        catalog: "zai",
        router: false,
        endpoints: &[
            (Openai, AdapterKind::OpenAI, "https://api.z.ai/api/paas/v4"),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://api.z.ai/api/anthropic",
            ),
        ],
    },
    Preset {
        id: "alibaba-cn",
        name: "Alibaba (China)",
        catalog: "alibaba-cn",
        router: false,
        endpoints: &[
            (
                Openai,
                AdapterKind::Aliyun,
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
            ),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://dashscope.aliyuncs.com/apps/anthropic",
            ),
        ],
    },
    Preset {
        id: "alibaba",
        name: "Alibaba",
        catalog: "alibaba",
        router: false,
        endpoints: &[
            (
                Openai,
                AdapterKind::Aliyun,
                "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            ),
            (
                Anthropic,
                AdapterKind::Anthropic,
                "https://dashscope-intl.aliyuncs.com/apps/anthropic",
            ),
        ],
    },
    Preset {
        id: "minimax-cn",
        name: "MiniMax (China)",
        catalog: "minimax-cn",
        router: false,
        endpoints: &[
            (
                Anthropic,
                AdapterKind::MiniMax,
                "https://api.minimaxi.com/anthropic",
            ),
            (Openai, AdapterKind::OpenAI, "https://api.minimaxi.com/v1"),
        ],
    },
    Preset {
        id: "minimax",
        name: "MiniMax",
        catalog: "minimax",
        router: false,
        endpoints: &[
            (
                Anthropic,
                AdapterKind::MiniMax,
                "https://api.minimax.io/anthropic",
            ),
            (Openai, AdapterKind::OpenAI, "https://api.minimax.io/v1"),
        ],
    },
    Preset {
        id: "xai",
        name: "xAI",
        catalog: "xai",
        router: false,
        endpoints: &[(Openai, AdapterKind::Xai, "https://api.x.ai/v1")],
    },
    Preset {
        id: "groq",
        name: "Groq",
        catalog: "groq",
        router: true,
        endpoints: &[(Openai, AdapterKind::Groq, "https://api.groq.com/openai/v1")],
    },
    Preset {
        id: "mistral",
        name: "Mistral",
        catalog: "mistral",
        router: false,
        endpoints: &[(Openai, AdapterKind::Mistral, "https://api.mistral.ai/v1")],
    },
    Preset {
        id: "together",
        name: "Together AI",
        catalog: "togetherai",
        router: true,
        endpoints: &[(Openai, AdapterKind::Together, "https://api.together.xyz/v1")],
    },
    Preset {
        id: "deepinfra",
        name: "Deep Infra",
        catalog: "deepinfra",
        router: true,
        endpoints: &[(
            Openai,
            AdapterKind::OpenAI,
            "https://api.deepinfra.com/v1/openai",
        )],
    },
    Preset {
        id: "cerebras",
        name: "Cerebras",
        catalog: "cerebras",
        router: true,
        endpoints: &[(Openai, AdapterKind::OpenAI, "https://api.cerebras.ai/v1")],
    },
    Preset {
        id: "vercel",
        name: "Vercel AI Gateway",
        catalog: "vercel",
        router: true,
        endpoints: &[(
            Openai,
            AdapterKind::OpenAI,
            "https://ai-gateway.vercel.sh/v1",
        )],
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The catalog provider preset `id`'s model metadata is read from: a
/// built-in's, else the catalog's provider of that id.
pub fn home(id: &str) -> &str {
    preset(id).map_or(id, |p| p.catalog)
}

/// The catalog providers whose models are their own: where a model's native
/// protocol is read from (a router's is not its models').
pub fn first_party<'a>() -> impl Iterator<Item = &'a str> {
    PRESETS.iter().filter(|p| !p.router).map(|p| p.catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format `kind` produces. Every OpenAI-compatible adapter
    /// delegates to the OpenAI one; the rest are named because they do not.
    fn dialect(kind: AdapterKind) -> Protocol {
        match kind {
            AdapterKind::Anthropic | AdapterKind::MiniMax => Protocol::Anthropic,
            AdapterKind::OpenAIResp => Protocol::OpenaiResponses,
            AdapterKind::Gemini | AdapterKind::GeminiIx => Protocol::Gemini,
            AdapterKind::OpenAI
            | AdapterKind::OpenRouter
            | AdapterKind::Requesty
            | AdapterKind::DeepSeek
            | AdapterKind::Moonshot
            | AdapterKind::Aliyun
            | AdapterKind::Xai
            | AdapterKind::Groq
            | AdapterKind::Mistral
            | AdapterKind::Together => Protocol::Openai,
            AdapterKind::Zai => panic!("the Zai adapter ignores the endpoint: name OpenAI"),
            other => panic!("classify AdapterKind::{other} here before using it"),
        }
    }

    /// A mismatched pair builds the wrong path and body: `/v1/chat/completions`
    /// against an Anthropic route, say, which only a 404 reports.
    #[test]
    fn every_endpoint_speaks_its_protocol() {
        for p in PRESETS {
            for &(protocol, kind, url) in p.endpoints {
                assert_eq!(
                    dialect(kind),
                    protocol,
                    "{}: {kind} cannot speak {protocol:?} at {url}",
                    p.id
                );
            }
        }
    }
}
