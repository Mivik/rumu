//! The built-in providers: their endpoints, and the catalog provider their
//! model metadata is read from.

use rumu_core::Protocol::{self, *};

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// The models.dev provider id.
    pub catalog: &'static str,
    /// Serves other vendors' models (`vendor/model` ids).
    pub router: bool,
    pub endpoints: &'static [(Protocol, &'static str)],
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "anthropic",
        name: "Anthropic",
        catalog: "anthropic",
        router: false,
        endpoints: &[(Anthropic, "https://api.anthropic.com")],
    },
    Preset {
        id: "openai",
        name: "OpenAI",
        catalog: "openai",
        router: false,
        endpoints: &[
            (OpenaiResponses, "https://api.openai.com/v1"),
            (Openai, "https://api.openai.com/v1"),
        ],
    },
    Preset {
        id: "google",
        name: "Google",
        catalog: "google",
        router: false,
        endpoints: &[
            (Gemini, "https://generativelanguage.googleapis.com/v1beta"),
            (
                Openai,
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
            (Openai, "https://openrouter.ai/api/v1"),
            (Anthropic, "https://openrouter.ai/api"),
        ],
    },
    Preset {
        id: "zenmux",
        name: "ZenMux",
        catalog: "zenmux",
        router: true,
        endpoints: &[
            (Openai, "https://zenmux.ai/api/v1"),
            (OpenaiResponses, "https://zenmux.ai/api/v1"),
            (Anthropic, "https://zenmux.ai/api/anthropic"),
        ],
    },
    Preset {
        id: "requesty",
        name: "Requesty",
        catalog: "requesty",
        router: true,
        endpoints: &[
            (Openai, "https://router.requesty.ai/v1"),
            (Anthropic, "https://router.requesty.ai"),
        ],
    },
    Preset {
        id: "deepseek",
        name: "DeepSeek",
        catalog: "deepseek",
        router: false,
        endpoints: &[
            (Openai, "https://api.deepseek.com"),
            (Anthropic, "https://api.deepseek.com/anthropic"),
        ],
    },
    Preset {
        id: "moonshot-cn",
        name: "Moonshot AI (China)",
        catalog: "moonshotai-cn",
        router: false,
        endpoints: &[
            (Openai, "https://api.moonshot.cn/v1"),
            (Anthropic, "https://api.moonshot.cn/anthropic"),
        ],
    },
    Preset {
        id: "moonshot",
        name: "Moonshot AI",
        catalog: "moonshotai",
        router: false,
        endpoints: &[
            (Openai, "https://api.moonshot.ai/v1"),
            (Anthropic, "https://api.moonshot.ai/anthropic"),
        ],
    },
    Preset {
        id: "zhipu",
        name: "Zhipu AI",
        catalog: "zhipuai",
        router: false,
        endpoints: &[
            (Openai, "https://open.bigmodel.cn/api/paas/v4"),
            (Anthropic, "https://open.bigmodel.cn/api/anthropic"),
        ],
    },
    Preset {
        id: "zai",
        name: "Z.AI",
        catalog: "zai",
        router: false,
        endpoints: &[
            (Openai, "https://api.z.ai/api/paas/v4"),
            (Anthropic, "https://api.z.ai/api/anthropic"),
        ],
    },
    Preset {
        id: "alibaba-cn",
        name: "Alibaba (China)",
        catalog: "alibaba-cn",
        router: false,
        endpoints: &[
            (Openai, "https://dashscope.aliyuncs.com/compatible-mode/v1"),
            (Anthropic, "https://dashscope.aliyuncs.com/apps/anthropic"),
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
                "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            ),
            (
                Anthropic,
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
            (Anthropic, "https://api.minimaxi.com/anthropic"),
            (Openai, "https://api.minimaxi.com/v1"),
        ],
    },
    Preset {
        id: "minimax",
        name: "MiniMax",
        catalog: "minimax",
        router: false,
        endpoints: &[
            (Anthropic, "https://api.minimax.io/anthropic"),
            (Openai, "https://api.minimax.io/v1"),
        ],
    },
    Preset {
        id: "xai",
        name: "xAI",
        catalog: "xai",
        router: false,
        endpoints: &[(Openai, "https://api.x.ai/v1")],
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The catalog providers whose models are their own: where a model's native
/// protocol is read from (a router's is not its models').
pub fn first_party<'a>() -> impl Iterator<Item = &'a str> {
    PRESETS.iter().filter(|p| !p.router).map(|p| p.catalog)
}
