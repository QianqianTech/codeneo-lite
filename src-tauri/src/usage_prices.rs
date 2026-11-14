use crate::usage::Tokens;
use serde::{Deserialize, Serialize};

pub const VERIFIED: &str = "2026-11-13";
pub const EXCHANGE_RATE_DATE: &str = "2026-11-12";
pub const USD_TO_CNY: f64 = 6.7174;
pub const EXCHANGE_RATE_SOURCE: &str =
    "https://xinhuanet.com.cn/20261112/97116e6012634afcaefaa35c8e5f9a92/c.html";

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Anthropic,
    Openai,
    Deepseek,
    QwenMax,
    QwenFlash,
    Kimi,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Reference {
    Rate(Source),
    Exchange(Exchange),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exchange {
    Exchange,
}

impl Reference {
    pub fn url(self) -> &'static str {
        match self {
            Self::Rate(Source::Anthropic) => {
                "https://platform.claude.com/docs/en/about-claude/pricing"
            }
            Self::Rate(Source::Openai) => "https://developers.openai.com/api/docs/pricing",
            Self::Rate(Source::Deepseek) => "https://api-docs.deepseek.com/quick_start/pricing/",
            Self::Rate(Source::QwenMax) => "https://help.aliyun.com/zh/model-studio/qwen3-8-max",
            Self::Rate(Source::QwenFlash) => {
                "https://help.aliyun.com/zh/model-studio/qwen3-8-flash"
            }
            Self::Rate(Source::Kimi) => "https://www.kimi.com/resources/kimi-k3-1-pro-pricing",
            Self::Exchange(_) => EXCHANGE_RATE_SOURCE,
        }
    }
}

#[derive(Clone, Copy)]
struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    cache_write_long: f64,
}

impl Rates {
    fn new(
        input: f64,
        output: f64,
        cache_read: f64,
        cache_write: f64,
        cache_write_long: f64,
    ) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write,
            cache_write_long,
        }
    }

    fn claude(input: f64, output: f64, cache_read: f64) -> Self {
        Self::new(input, output, cache_read, input * 1.25, input * 2.0)
    }

    fn cost(self, tokens: Tokens) -> f64 {
        (tokens.input as f64 * self.input
            + tokens.output as f64 * self.output
            + tokens.cache_read as f64 * self.cache_read
            + tokens.cache_write as f64 * self.cache_write
            + tokens.cache_write_long as f64 * self.cache_write_long)
            / 1_000_000.0
    }
}

fn dated_alias(model: &str) -> &str {
    // API snapshots use either YYYY-MM-DD or YYYYMMDD after the model ID.
    for (width, date_has_dashes) in [(10, true), (8, false)] {
        let Some(split) = model.len().checked_sub(width + 1) else {
            continue;
        };
        let Some((base, suffix)) = model.split_at_checked(split) else {
            continue;
        };
        if let Some(date) = suffix.strip_prefix('-') {
            if date.starts_with("20")
                && date.bytes().enumerate().all(|(i, ch)| {
                    if date_has_dashes && matches!(i, 4 | 7) {
                        ch == b'-'
                    } else {
                        ch.is_ascii_digit()
                    }
                })
            {
                return base;
            }
        }
    }
    model
}

#[derive(Clone, Copy, Serialize)]
pub struct Estimate {
    pub low_cny: f64,
    pub high_cny: f64,
    pub source: Source,
}

/// Match a rate only when the usage source identifies the official provider.
pub fn for_provider(
    provider: &str,
    model: &str,
    tokens: Tokens,
    per_request: bool,
) -> Option<Estimate> {
    use Source::*;
    let model = model
        .strip_prefix("anthropic/")
        .or_else(|| model.strip_prefix("openai/"))
        .unwrap_or(model);
    let model = model.strip_suffix("[1m]").unwrap_or(model);
    let source = match provider {
        "openai" | "api.openai.com" => Openai,
        "anthropic" | "api.anthropic.com" => Anthropic,
        "deepseek" | "api.deepseek.com" => Deepseek,
        "dashscope.aliyuncs.com" => {
            if dated_alias(model) == "qwen3.8-max" {
                QwenMax
            } else {
                QwenFlash
            }
        }
        "moonshot" | "api.moonshot.cn" | "api.moonshot.ai" => Kimi,
        _ => return None,
    };
    // Session aggregates cannot establish the input size of each request.
    let tiered = source == Openai
        && matches!(
            dated_alias(model),
            "gpt-6-astra"
                | "gpt-6.1-sol"
                | "gpt-6.1-terra"
                | "gpt-6.1-luna"
                | "gpt-6-sol"
                | "gpt-5.6-sol"
                | "gpt-5.6-terra"
                | "gpt-5.6-luna"
                | "gpt-5.5"
                | "gpt-5.4"
        )
        || source == Anthropic && dated_alias(model) == "claude-haiku-5-5";
    if tiered && !per_request {
        return None;
    }
    estimate(model, tokens).filter(|price| price.source == source)
}

pub fn estimate(model: &str, tokens: Tokens) -> Option<Estimate> {
    use Source::{Anthropic, Deepseek, Kimi, Openai, QwenFlash, QwenMax};
    let model = dated_alias(model.strip_suffix("[1m]").unwrap_or(model));
    let long = tokens.input_total() > 272_000;
    let (source, rates) = match model {
        "claude-fable-5-2" | "claude-mythos-5-2" => (Anthropic, Rates::claude(8.0, 40.0, 0.20)),
        "claude-fable-5-1" | "claude-mythos-5-1" => (Anthropic, Rates::claude(10.0, 50.0, 0.25)),
        "claude-fable-5" | "claude-mythos-5" => (Anthropic, Rates::claude(10.0, 50.0, 1.0)),
        "claude-opus-5-5" => (Anthropic, Rates::claude(4.0, 20.0, 0.20)),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
        | "claude-opus-4-5" => (Anthropic, Rates::claude(5.0, 25.0, 0.50)),
        "claude-sonnet-5-5" => (Anthropic, Rates::claude(2.0, 10.0, 0.10)),
        "claude-sonnet-5" => (Anthropic, Rates::claude(2.0, 10.0, 0.20)),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4" => {
            (Anthropic, Rates::claude(3.0, 15.0, 0.30))
        }
        "claude-haiku-5-5" if tokens.input_total() > 100_000 => {
            (Anthropic, Rates::claude(0.50, 2.50, 0.05))
        }
        "claude-haiku-5-5" => (Anthropic, Rates::claude(0.10, 0.50, 0.01)),
        "claude-haiku-4-5" => (Anthropic, Rates::claude(1.0, 5.0, 0.10)),
        "gpt-6-astra" if long => (Openai, Rates::new(20.0, 75.0, 2.0, 25.0, 50.0)),
        "gpt-6-astra" => (Openai, Rates::new(10.0, 50.0, 1.0, 12.50, 25.0)),
        "gpt-6.1-sol" if long => (Openai, Rates::new(4.0, 15.0, 0.20, 5.0, 10.0)),
        "gpt-6.1-sol" => (Openai, Rates::new(2.0, 10.0, 0.10, 2.50, 5.0)),
        "gpt-6.1-terra" if long => (Openai, Rates::new(2.0, 7.50, 0.10, 2.50, 5.0)),
        "gpt-6.1-terra" => (Openai, Rates::new(1.0, 5.0, 0.05, 1.25, 2.50)),
        "gpt-6-luna" if long => (Openai, Rates::new(0.20, 0.75, 0.02, 0.25, 0.50)),
        "gpt-6-luna" => (Openai, Rates::new(0.10, 0.50, 0.01, 0.125, 0.25)),
        "gpt-6-sol" if long => (Openai, Rates::new(4.0, 15.0, 0.40, 5.0, 10.0)),
        "gpt-6-sol" => (Openai, Rates::new(2.0, 10.0, 0.20, 2.50, 5.0)),
        "gpt-5.6-sol" if long => (Openai, Rates::new(8.0, 30.0, 0.80, 10.0, 20.0)),
        "gpt-5.6-sol" => (Openai, Rates::new(4.0, 20.0, 0.40, 5.0, 10.0)),
        "gpt-5.6-terra" if long => (Openai, Rates::new(4.0, 18.0, 0.40, 5.0, 10.0)),
        "gpt-5.6-terra" => (Openai, Rates::new(2.0, 12.0, 0.20, 2.50, 5.0)),
        "gpt-5.6-luna" if long => (Openai, Rates::new(0.40, 1.80, 0.04, 0.50, 1.0)),
        "gpt-5.6-luna" => (Openai, Rates::new(0.20, 1.20, 0.02, 0.25, 0.50)),
        "gpt-5.5" if long => (Openai, Rates::new(10.0, 45.0, 1.0, 10.0, 10.0)),
        "gpt-5.5" => (Openai, Rates::new(5.0, 30.0, 0.50, 5.0, 5.0)),
        "gpt-5.4" if long => (Openai, Rates::new(5.0, 22.50, 0.50, 5.0, 5.0)),
        "gpt-5.4" => (Openai, Rates::new(2.50, 15.0, 0.25, 2.50, 2.50)),
        "gpt-5.4-mini" => (Openai, Rates::new(0.75, 4.50, 0.075, 0.75, 0.75)),
        "gpt-5.2" => (Openai, Rates::new(1.75, 14.0, 0.175, 1.75, 1.75)),
        "gpt-5.1" | "gpt-5" => (Openai, Rates::new(1.25, 10.0, 0.125, 1.25, 1.25)),
        "gpt-5-mini" => (Openai, Rates::new(0.25, 2.0, 0.025, 0.25, 0.25)),
        "deepseek-flash" | "deepseek-v4-flash" | "deepseek-v4-flash-vision-exp" => {
            (Deepseek, Rates::new(0.30, 1.20, 0.006, 0.30, 0.30))
        }
        "deepseek-v4-pro" => (Deepseek, Rates::new(1.32, 3.96, 0.044, 1.32, 1.32)),
        "qwen3.8-max" => (QwenMax, Rates::new(12.0, 36.0, 1.5, 15.0, 15.0)),
        "qwen3.8-flash" => (QwenFlash, Rates::new(0.80, 2.70, 0.10, 1.25, 1.25)),
        "k3" | "kimi-k3" => (Kimi, Rates::new(20.0, 100.0, 2.0, 20.0, 40.0)),
        "k3.1-pro" | "kimi-k3.1-pro" => (Kimi, Rates::new(13.0, 68.0, 1.30, 13.0, 26.0)),
        "k3.1-pro-highspeed" | "kimi-k3.1-pro-highspeed" => {
            (Kimi, Rates::new(26.0, 136.0, 1.30, 26.0, 52.0))
        }
        _ => return None,
    };
    let high = rates.cost(tokens);
    let (low_cny, high_cny) = match source {
        Deepseek => (high * 0.5 * USD_TO_CNY, high * USD_TO_CNY),
        QwenMax => (
            Rates {
                cache_read: 1.0,
                ..rates
            }
            .cost(tokens),
            high,
        ),
        QwenFlash | Kimi => (high, high),
        _ => (high * USD_TO_CNY, high * USD_TO_CNY),
    };
    Some(Estimate {
        low_cny,
        high_cny,
        source,
    })
}
