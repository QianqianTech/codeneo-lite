use crate::{models::QWEN4_MAX, usage::Tokens};
use serde::{Deserialize, Serialize};

pub const VERIFIED: &str = "2026-11-17";
pub const EXCHANGE_RATE_DATE: &str = "2026-11-12";
pub const USD_TO_CNY: f64 = 6.7174;
pub const EXCHANGE_RATE_SOURCE: &str =
    "https://xinhuanet.com.cn/20261112/97116e6012634afcaefaa35c8e5f9a92/c.html";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Anthropic,
    Openai,
    Deepseek,
    Qwen4Max,
    QwenMax,
    QwenFlash,
    Kimi,
    Glm,
    QwenPreview,
    QwenTokenPlan,
    Qwen37Max,
    Qwen37Plus,
    Qwen36Plus,
    Qwen36Flash,
    BailianDeepseek41,
    BailianDeepseekPro,
    BailianDeepseekFlash,
    BailianKimi27,
    BailianKimi26,
    BailianKimi25,
    BailianMinimax,
    BailianGlm,
    BailianModels,
    KimiCode,
}

#[derive(Serialize)]
pub struct SourceInfo {
    pub id: Source,
    pub label: &'static str,
    pub url: &'static str,
    pub verified: &'static str,
}

impl Source {
    pub fn info(self) -> SourceInfo {
        use Source::*;
        let (label, url) = match self {
            Anthropic => (
                "Anthropic 官方价",
                "https://platform.claude.com/docs/en/about-claude/pricing",
            ),
            Openai => (
                "OpenAI 官方价",
                "https://developers.openai.com/api/docs/pricing",
            ),
            Deepseek => (
                "DeepSeek 峰谷价",
                "https://api-docs.deepseek.com/quick_start/pricing/",
            ),
            Qwen4Max => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/model-pricing",
            ),
            QwenMax => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/qwen3-8-max",
            ),
            QwenFlash => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/qwen3-8-flash",
            ),
            Kimi => (
                "Kimi 官方价",
                "https://www.kimi.com/resources/kimi-k3-1-pro-pricing",
            ),
            Glm => (
                "智谱官方价",
                "https://docs.bigmodel.cn/cn/guide/start/pricing",
            ),
            QwenPreview => (
                "Token Plan 官方说明",
                "https://help.aliyun.com/zh/model-studio/qwen4-max-preview",
            ),
            QwenTokenPlan => (
                "Token Plan 官方说明",
                "https://help.aliyun.com/zh/model-studio/token-plan-usage-quota",
            ),
            Qwen37Max => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/qwen3-7-max",
            ),
            Qwen37Plus => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/qwen3-7-plus",
            ),
            Qwen36Plus => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/qwen3-6-plus",
            ),
            Qwen36Flash => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/qwen3-6-flash",
            ),
            BailianDeepseek41 => (
                "百炼峰谷价 · 北京",
                "https://help.aliyun.com/zh/model-studio/deepseek-v4-1-flash",
            ),
            BailianDeepseekPro => (
                "百炼峰谷价 · 北京",
                "https://help.aliyun.com/zh/model-studio/deepseek-v4-pro",
            ),
            BailianDeepseekFlash => (
                "百炼峰谷价 · 北京",
                "https://help.aliyun.com/zh/model-studio/model-routing",
            ),
            BailianKimi27 => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/kimi-k2-7-code",
            ),
            BailianKimi26 => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/kimi-k2-6",
            ),
            BailianKimi25 => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/kimi-k2-5",
            ),
            BailianMinimax => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/minimax-m2-5",
            ),
            BailianGlm => (
                "百炼官方价 · 北京",
                "https://help.aliyun.com/zh/model-studio/glm-5-3",
            ),
            BailianModels => (
                "百炼模型说明",
                "https://help.aliyun.com/zh/model-studio/token-plan-team-overview",
            ),
            KimiCode => (
                "Kimi Code 模型说明",
                "https://www.kimi.com/code/docs/third-party-tools/claude-code.html",
            ),
        };
        SourceInfo {
            id: self,
            label,
            url,
            verified: match self {
                Anthropic | Openai | Deepseek | QwenMax | QwenFlash | Kimi => "2026-11-13",
                Qwen4Max | QwenTokenPlan => VERIFIED,
                _ => "2026-11-16",
            },
        }
    }
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
            Self::Rate(source) => source.info().url,
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

// Normalize only known vendor namespaces and documented aliases. In particular,
// never strip arbitrary router names or map a new/preview model to an older price.
fn normalized_model(raw: &str) -> String {
    let raw = raw.trim().to_ascii_lowercase();
    let raw = raw.strip_suffix("[1m]").unwrap_or(&raw);
    let raw = match raw.split_once('/') {
        Some(("anthropic", model)) if model.starts_with("claude-") => model,
        Some(("openai", model)) if model.starts_with("gpt-") => model,
        Some(("deepseek", model)) if model.starts_with("deepseek-") => model,
        Some(("qwen", model)) if model.starts_with("qwen") => model,
        Some(("z-ai" | "zhipu", model)) if model.starts_with("glm-") => model,
        Some(("moonshotai" | "kimi", model)) if model.starts_with("kimi-") => model,
        Some(("minimax", model)) if model.starts_with("minimax-") => model,
        _ => raw,
    };
    let raw = dated_alias(raw);
    match raw {
        "qwen4.0-max-preview" => "qwen4-max-preview".into(),
        // OpenRouter uses dotted Claude versions, while Anthropic uses dashes.
        model if model.starts_with("claude-") => model.replace('.', "-"),
        _ => raw.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    MissingPrice,
    PlanOnly,
    Subscription,
    DynamicModel,
    UnknownProvider,
    ProviderMismatch,
    RequestDetailRequired,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Unpriced {
    pub reason: Reason,
    pub source: Option<Source>,
}

impl Unpriced {
    pub fn provider_unknown() -> Self {
        Self {
            reason: Reason::UnknownProvider,
            source: None,
        }
    }
}

fn unavailable(model: &str) -> Unpriced {
    use Reason::*;
    let (reason, source) = match model {
        "qwen4-max-preview" => (PlanOnly, Some(Source::QwenPreview)),
        "auto" => (DynamicModel, Some(Source::BailianModels)),
        "kimi-for-coding" | "kimi-for-coding-highspeed" => (DynamicModel, Some(Source::KimiCode)),
        "codex-auto-review"
        | "~anthropic/claude-sonnet-latest"
        | "~anthropic/claude-opus-latest"
        | "~anthropic/claude-fable-latest"
        | "~anthropic/claude-haiku-latest" => (DynamicModel, None),
        _ => (MissingPrice, None),
    };
    Unpriced { reason, source }
}

#[derive(Clone, Copy, Debug, Serialize)]
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
) -> Result<Estimate, Unpriced> {
    use Source::*;
    let model = normalized_model(model);
    let model = model.as_str();
    let mut price = estimate(model, tokens)?;
    let source = match provider {
        "openai" | "api.openai.com" => Openai,
        "anthropic" | "api.anthropic.com" => Anthropic,
        "deepseek" | "api.deepseek.com" => Deepseek,
        "glm" | "zhipu" | "bigmodel" | "open.bigmodel.cn" => Glm,
        "dashscope.aliyuncs.com" => match price.source {
            Qwen4Max | QwenMax | QwenFlash | Qwen37Max | Qwen37Plus | Qwen36Plus | Qwen36Flash
            | BailianDeepseek41 | BailianDeepseekPro | BailianDeepseekFlash | BailianKimi27
            | BailianKimi26 | BailianKimi25 | BailianMinimax => price.source,
            Glm if model == "glm-5.3" => {
                price.source = BailianGlm;
                BailianGlm
            }
            _ => {
                return Err(Unpriced {
                    reason: Reason::ProviderMismatch,
                    source: Some(price.source),
                })
            }
        },
        "token-plan.cn-beijing.maas.aliyuncs.com" if model == QWEN4_MAX => {
            return Err(Unpriced {
                reason: Reason::Subscription,
                source: Some(QwenTokenPlan),
            });
        }
        host if model == QWEN4_MAX && host.ends_with(".cn-beijing.maas.aliyuncs.com") => Qwen4Max,
        "moonshot" | "api.moonshot.cn" | "api.moonshot.ai" => Kimi,
        _ => return Err(Unpriced::provider_unknown()),
    };
    // Session aggregates cannot establish the input size of each request.
    let tiered = source == Openai
        && matches!(
            dated_alias(model),
            "gpt-6-astra"
                | "gpt-6.1-sol"
                | "gpt-6.1-terra"
                | "gpt-6-luna"
                | "gpt-6-sol"
                | "gpt-5.6-sol"
                | "gpt-5.6-terra"
                | "gpt-5.6-luna"
                | "gpt-5.5"
                | "gpt-5.4"
        )
        || source == Anthropic && model == "claude-haiku-5-5"
        || source == Glm && matches!(model, "glm-5.1" | "glm-5" | "glm-5-turbo")
        || matches!(source, Qwen37Plus | Qwen36Plus | Qwen36Flash);
    if tiered && !per_request {
        return Err(Unpriced {
            reason: Reason::RequestDetailRequired,
            source: Some(source),
        });
    }
    if price.source != source {
        return Err(Unpriced {
            reason: Reason::ProviderMismatch,
            source: Some(price.source),
        });
    }
    Ok(price)
}

pub fn estimate(model: &str, tokens: Tokens) -> Result<Estimate, Unpriced> {
    use Source::*;
    let model = normalized_model(model);
    let model = model.as_str();
    let long = tokens.input_total() > 272_000;
    let qwen_long = tokens.input_total() > 256_000;
    let glm_long = tokens.input_total() >= 32_000;
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
        QWEN4_MAX => (Qwen4Max, Rates::new(10.0, 30.0, 0.5, 12.5, 20.0)),
        "qwen3.8-max" => (QwenMax, Rates::new(12.0, 36.0, 1.5, 15.0, 15.0)),
        "qwen3.8-flash" => (QwenFlash, Rates::new(0.80, 2.70, 0.10, 1.25, 1.25)),
        "qwen3.7-max" => (Qwen37Max, Rates::new(12.0, 36.0, 2.4, 15.0, 15.0)),
        "qwen3.7-plus" if qwen_long => (Qwen37Plus, Rates::new(6.0, 24.0, 1.2, 7.5, 7.5)),
        "qwen3.7-plus" => (Qwen37Plus, Rates::new(2.0, 8.0, 0.4, 2.5, 2.5)),
        "qwen3.6-plus" if qwen_long => (Qwen36Plus, Rates::new(8.0, 48.0, 0.8, 10.0, 10.0)),
        "qwen3.6-plus" => (Qwen36Plus, Rates::new(2.0, 12.0, 0.2, 2.5, 2.5)),
        "qwen3.6-flash" if qwen_long => (Qwen36Flash, Rates::new(4.8, 28.8, 0.48, 6.0, 6.0)),
        "qwen3.6-flash" => (Qwen36Flash, Rates::new(1.2, 7.2, 0.12, 1.5, 1.5)),
        // GLM uses CNY, with implicit cache creation charged as ordinary input.
        // Cache storage is currently free; do not apply Anthropic's write multipliers.
        "glm-5.3" | "glm-5.2" => (Glm, Rates::new(8.0, 28.0, 2.0, 8.0, 8.0)),
        "glm-5.3-flash" => (Glm, Rates::new(0.8, 2.8, 0.23, 0.8, 0.8)),
        "glm-5.3-flashx" | "glm-5.3-flashx-legal" => (Glm, Rates::new(2.0, 7.0, 0.57, 2.0, 2.0)),
        "glm-5.1" if glm_long => (Glm, Rates::new(8.0, 28.0, 2.0, 8.0, 8.0)),
        "glm-5.1" => (Glm, Rates::new(6.0, 24.0, 1.3, 6.0, 6.0)),
        "glm-5" if glm_long => (Glm, Rates::new(6.0, 22.0, 1.5, 6.0, 6.0)),
        "glm-5" => (Glm, Rates::new(4.0, 18.0, 1.0, 4.0, 4.0)),
        "glm-5-turbo" if glm_long => (Glm, Rates::new(7.0, 26.0, 1.8, 7.0, 7.0)),
        "glm-5-turbo" => (Glm, Rates::new(5.0, 22.0, 1.2, 5.0, 5.0)),
        "deepseek-v4.1-flash" | "deepseek-v4.1-flash-1017" => {
            (BailianDeepseek41, Rates::new(2.0, 8.0, 0.04, 2.0, 2.0))
        }
        "deepseek-v4-pro-0813" => (BailianDeepseekPro, Rates::new(9.0, 27.0, 0.9, 9.0, 9.0)),
        "deepseek-v4-flash-0731" => (BailianDeepseekFlash, Rates::new(3.0, 9.0, 0.3, 3.0, 3.0)),
        "kimi-k2.7-code" => (BailianKimi27, Rates::new(6.5, 27.0, 1.3, 8.125, 8.125)),
        "kimi-k2.6" => (BailianKimi26, Rates::new(6.5, 27.0, 1.3, 8.125, 8.125)),
        "kimi-k2.5" => (BailianKimi25, Rates::new(4.0, 21.0, 0.8, 5.0, 5.0)),
        "minimax-m2.5" => (BailianMinimax, Rates::new(2.1, 8.4, 0.42, 2.1, 2.1)),
        "k3" | "kimi-k3" | "k3-256k" => (Kimi, Rates::new(20.0, 100.0, 2.0, 20.0, 40.0)),
        "k3.1-pro" | "kimi-k3.1-pro" => (Kimi, Rates::new(13.0, 68.0, 1.30, 13.0, 26.0)),
        "k3.1-pro-highspeed" | "kimi-k3.1-pro-highspeed" => {
            (Kimi, Rates::new(26.0, 136.0, 1.30, 26.0, 52.0))
        }
        _ => return Err(unavailable(model)),
    };
    let high = rates.cost(tokens);
    let (low_cny, high_cny) = match source {
        Deepseek => (high * 0.5 * USD_TO_CNY, high * USD_TO_CNY),
        BailianDeepseek41 | BailianDeepseekPro | BailianDeepseekFlash => (high * 0.5, high),
        QwenMax => (
            Rates {
                cache_read: 1.0,
                ..rates
            }
            .cost(tokens),
            high,
        ),
        Qwen37Max | Qwen37Plus | BailianKimi27 | BailianKimi26 | BailianKimi25 => (
            Rates {
                cache_read: rates.cache_read * 0.5,
                ..rates
            }
            .cost(tokens),
            high,
        ),
        Anthropic | Openai => (high * USD_TO_CNY, high * USD_TO_CNY),
        _ => (high, high),
    };
    Ok(Estimate {
        low_cny,
        high_cny,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
    }

    #[test]
    fn glm_prices_include_input_output_and_cache_without_usd_conversion() {
        let tokens = Tokens {
            input: 18_540_000,
            cache_read: 40_510_000,
            output: 197_000,
            ..Tokens::default()
        };
        let price = estimate("glm-5.3", tokens).unwrap();
        close(price.low_cny, 234.856);
        close(price.high_cny, 234.856);
        assert_eq!(price.source, Source::Glm);

        let write = Tokens {
            cache_write: 1_000_000,
            cache_write_long: 1_000_000,
            ..Tokens::default()
        };
        close(estimate("glm-5.3", write).unwrap().high_cny, 16.0);
        let cached = Tokens {
            cache_read: 1_000_000,
            ..Tokens::default()
        };
        close(estimate("glm-5.3-flash", cached).unwrap().high_cny, 0.23);
        close(estimate("glm-5.3-flashx", cached).unwrap().high_cny, 0.57);
    }

    #[test]
    fn qwen4_uses_the_beijing_rate_for_each_token_category() {
        let n = 1_000_000;
        for (tokens, rate) in [
            (Tokens { input: n, ..Tokens::default() }, 10.0),
            (Tokens { output: n, ..Tokens::default() }, 30.0),
            (Tokens { cache_read: n, ..Tokens::default() }, 0.5),
            (Tokens { cache_write: n, ..Tokens::default() }, 12.5),
            (Tokens { cache_write_long: n, ..Tokens::default() }, 20.0),
        ] {
            let price = estimate("QWEN/QWEN4-MAX[1m]", tokens).unwrap();
            close(price.low_cny, rate);
            close(price.high_cny, rate);
            assert_eq!(price.source, Source::Qwen4Max);
        }
        let tokens = Tokens { input: 1000, ..Tokens::default() };
        for host in ["dashscope.aliyuncs.com", "workspace.cn-beijing.maas.aliyuncs.com"] {
            close(for_provider(host, QWEN4_MAX, tokens, false).unwrap().high_cny, 0.01);
        }
        let plan = for_provider("token-plan.cn-beijing.maas.aliyuncs.com", QWEN4_MAX, tokens, true)
            .unwrap_err();
        assert_eq!(plan.reason, Reason::Subscription);
        assert_eq!(plan.source, Some(Source::QwenTokenPlan));
    }

    #[test]
    fn aliases_share_prices_but_unknown_versions_never_borrow_them() {
        let tokens = Tokens {
            input: 1000,
            ..Tokens::default()
        };
        for alias in [
            "glm-5.3[1m]",
            "ZHIPU/GLM-5.3",
            "z-ai/glm-5.3",
            "GLM-5.3-2026-08-14",
        ] {
            close(estimate(alias, tokens).unwrap().high_cny, 0.008);
        }
        close(
            estimate("anthropic/claude-sonnet-5.5", tokens)
                .unwrap()
                .high_cny,
            estimate("claude-sonnet-5-5", tokens).unwrap().high_cny,
        );
        for alias in [
            "qwen4-max-preview",
            "qwen4.0-max-preview",
            "QWEN/QWEN4.0-MAX-PREVIEW[1m]",
        ] {
            let issue = estimate(alias, tokens).unwrap_err();
            assert_eq!(issue.reason, Reason::PlanOnly);
            assert_eq!(issue.source, Some(Source::QwenPreview));
        }
        for unknown in [
            "glm-5.4",
            "qwen5-max",
            "qwen4-max-experimental",
            "proxy/glm-5.3",
            "openai/glm-5.3",
            "glm-5.3-special",
        ] {
            assert_eq!(
                estimate(unknown, tokens).unwrap_err().reason,
                Reason::MissingPrice
            );
        }
    }

    #[test]
    fn official_provider_prices_are_not_applied_to_other_providers() {
        let tokens = Tokens {
            input: 1000,
            ..Tokens::default()
        };
        for provider in ["glm", "zhipu", "open.bigmodel.cn"] {
            close(
                for_provider(provider, "glm-5.3", tokens, true)
                    .unwrap()
                    .high_cny,
                0.008,
            );
        }
        assert_eq!(
            for_provider("dashscope.aliyuncs.com", "glm-5.3", tokens, true)
                .unwrap()
                .source,
            Source::BailianGlm
        );
        assert_eq!(
            for_provider("api.z.ai", "glm-5.3", tokens, true)
                .unwrap_err()
                .reason,
            Reason::UnknownProvider
        );
        assert_eq!(
            for_provider("openai", "glm-5.3", tokens, true)
                .unwrap_err()
                .reason,
            Reason::ProviderMismatch
        );
        for model in ["gpt-6-luna", "gpt-6.1-sol"] {
            assert_eq!(
                for_provider("openai", model, tokens, false)
                    .unwrap_err()
                    .reason,
                Reason::RequestDetailRequired
            );
        }
    }

    #[test]
    fn tiered_models_use_each_request_input_including_cached_tokens() {
        let small = Tokens {
            input: 1,
            cache_read: 31_998,
            ..Tokens::default()
        };
        let large = Tokens {
            cache_read: 31_999,
            ..small
        };
        close(
            estimate("glm-5.1", small).unwrap().high_cny,
            (6.0 + 31_998.0 * 1.3) / 1e6,
        );
        close(
            estimate("glm-5.1", large).unwrap().high_cny,
            (8.0 + 31_999.0 * 2.0) / 1e6,
        );
        assert_eq!(
            for_provider("glm", "glm-5.1", large, false)
                .unwrap_err()
                .reason,
            Reason::RequestDetailRequired
        );
        let small = Tokens {
            cache_read: 256_000,
            ..Tokens::default()
        };
        let large = Tokens { input: 1, ..small };
        close(estimate("qwen3.7-plus", small).unwrap().high_cny, 0.1024);
        close(estimate("qwen3.7-plus", large).unwrap().high_cny, 0.307206);
    }

    #[test]
    fn every_preset_model_has_a_price_or_an_explicit_reviewed_exception() {
        let presets = serde_json::to_value(crate::models::presets()).unwrap();
        let mut models = std::collections::BTreeSet::new();
        for preset in presets.as_array().unwrap() {
            for model in preset["models"].as_array().unwrap() {
                models.insert(model.as_str().unwrap().to_owned());
            }
        }
        for preset in claudekeeper_claude::provider_presets() {
            for model in [
                preset.model,
                preset.sonnet_model,
                preset.opus_model,
                preset.haiku_model,
                preset.small_fast_model,
                preset.subagent_model,
                preset.reasoning_model,
                preset.fable_model,
            ] {
                if !model.is_empty() {
                    models.insert(model.to_owned());
                }
            }
        }
        assert!(models.len() >= 35);
        for model in models {
            match estimate(&model, Tokens { input: 1000, ..Tokens::default() }) {
                Ok(price) => {
                    assert!(price.low_cny.is_finite() && price.low_cny >= 0.0);
                    assert!(price.high_cny.is_finite() && price.high_cny >= price.low_cny);
                    assert!(price.source.info().url.starts_with("https://"));
                }
                Err(issue) => assert!(matches!(issue.reason,
                    Reason::PlanOnly | Reason::DynamicModel),
                    "预置模型 {model} 缺少计价规则；新增模型时必须核对单价或记录明确的例外：{issue:?}"),
            }
        }
    }
}
