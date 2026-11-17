use crate::{
    desktop_effort,
    model_transport::{self as http, EffortLevel},
    models::{self, Account, Model, Protocol, Store},
    transaction::Client,
};
use claudekeeper_claude::AuthMode;
use futures_util::{stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, time::Duration};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Options {
    pub levels: Vec<EffortLevel>,
    pub default: Option<EffortLevel>,
}

pub(crate) type Catalog = BTreeMap<String, Options>;
pub(crate) type Accounts = BTreeMap<String, Catalog>;

impl Options {
    fn known(levels: &[EffortLevel], default: Option<EffortLevel>) -> Self {
        Self {
            levels: levels.to_vec(),
            default,
        }
    }

    pub(crate) fn within(&self, supported: &[EffortLevel]) -> Option<Self> {
        let levels: Vec<_> = self
            .levels
            .iter()
            .copied()
            .filter(|level| supported.contains(level))
            .collect();
        (levels
            .iter()
            .filter(|&&level| level != EffortLevel::None)
            .count()
            >= 2)
            .then(|| Self {
                default: self.default.filter(|level| levels.contains(level)),
                levels,
            })
    }

    pub(crate) fn fallback(&self) -> Option<EffortLevel> {
        self.default
            .or_else(|| {
                [EffortLevel::Medium, EffortLevel::High]
                    .into_iter()
                    .find(|level| self.levels.contains(level))
            })
            .or_else(|| {
                self.levels
                    .iter()
                    .copied()
                    .find(|&level| level != EffortLevel::None)
            })
    }

    pub(crate) fn nearest(&self, requested: EffortLevel) -> Option<EffortLevel> {
        if self.levels.contains(&requested) {
            return Some(requested);
        }
        self.levels
            .iter()
            .copied()
            .filter(|&level| level != EffortLevel::None)
            .min_by_key(|level| (*level as usize).abs_diff(requested as usize))
    }
}

pub(crate) fn endpoint(account: &Account, protocol: Protocol) -> String {
    if protocol == Protocol::Anthropic {
        account.base_for(protocol).to_owned()
    } else {
        models::openai_endpoint(account, protocol, "")
    }
}

pub(crate) fn is_openrouter(account: &Account, protocol: Protocol) -> bool {
    http::provider_base(&endpoint(account, protocol)).is_ok_and(|base| {
        base.host_str() == Some("openrouter.ai") && base.path().trim_end_matches('/') == "/api"
    })
}

pub(crate) fn is_qwen4_responses(endpoint: &Url, model: &str) -> bool {
    model == models::QWEN4_MAX
        && matches!(
            endpoint.path().trim_end_matches('/'),
            "/compatible-mode/v1" | "/compatible-mode/v1/responses"
        )
        && http::is_model_studio(endpoint)
}

pub(crate) fn adaptive_anthropic(account: &Account, model: &str) -> bool {
    let Ok(base) = http::provider_base(account.base_for(Protocol::Anthropic)) else {
        return false;
    };
    if is_openrouter(account, Protocol::Anthropic) {
        return model.starts_with("anthropic/")
            && !model.contains("claude-opus-4.5")
            && !model.contains("claude-opus-4-5")
            && desktop_effort::known_levels(&base, model).is_some();
    }
    matches!(
        base.host_str(),
        Some("api.anthropic.com" | "api.deepseek.com" | "api.kimi.com" | "api.kimi.ai")
    ) && desktop_effort::known_levels(&base, model).is_some()
        && !model.starts_with("claude-opus-4-5")
}

pub(crate) fn claude_code_capabilities(
    account: &Account,
    model: &str,
    catalogs: &Accounts,
) -> Option<String> {
    use EffortLevel::{High, Low, Max, Medium, XHigh};
    let model = http::provider_api_model(model).ok()?;
    let options = options(account, Protocol::Anthropic, &model, catalogs);
    // Claude Code always offers low/medium/high as a group. Other subsets
    // are available through the per-session launcher in Lite.
    if ![Low, Medium, High]
        .iter()
        .all(|level| options.levels.contains(level))
    {
        return None;
    }
    let mut capabilities = vec!["effort"];
    if options.levels.contains(&XHigh) {
        capabilities.push("xhigh_effort");
    }
    if options.levels.contains(&Max) {
        capabilities.push("max_effort");
    }
    if adaptive_anthropic(account, &model) {
        capabilities.extend(["thinking", "adaptive_thinking"]);
    }
    Some(capabilities.join(","))
}

pub(crate) fn options(
    account: &Account,
    protocol: Protocol,
    model: &str,
    catalogs: &Accounts,
) -> Options {
    declared(account, protocol, model, catalogs).unwrap_or_default()
}

pub(crate) fn declared(
    account: &Account,
    protocol: Protocol,
    model: &str,
    catalogs: &Accounts,
) -> Option<Options> {
    if let Some(declared) = catalogs
        .get(&account.id)
        .and_then(|catalog| catalog.get(model))
    {
        return Some(declared.clone());
    }
    http::provider_base(&endpoint(account, protocol))
        .ok()
        .and_then(|base| known(&base, protocol, model))
}

fn dated_alias(model: &str) -> &str {
    if model.len() >= 11 {
        let date = &model.as_bytes()[model.len() - 11..];
        if date.starts_with(b"-20")
            && date[5] == b'-'
            && date[8] == b'-'
            && date
                .iter()
                .enumerate()
                .all(|(i, value)| [0, 5, 8].contains(&i) || value.is_ascii_digit())
        {
            return &model[..model.len() - 11];
        }
    }
    model
}

fn known(base: &Url, protocol: Protocol, raw_model: &str) -> Option<Options> {
    use EffortLevel::{High, Low, Max, Medium, Minimal, None as Off, XHigh};
    let model = raw_model.to_ascii_lowercase();
    let model = model.as_str();
    let host = base.host_str()?;
    let path = base.path().trim_end_matches('/');
    if matches!(
        (host, path, model),
        (
            "api.kimi.com" | "api.kimi.ai",
            "/coding",
            "kimi-for-coding-highspeed"
        )
    ) || (models::is_aliyun_token_plan(base) && model == "auto")
    {
        return Some(Options::default());
    }
    if protocol == Protocol::Anthropic {
        let levels = desktop_effort::known_levels(base, model)?;
        let default = match (host, model) {
            ("api.deepseek.com", _) => Some(High),
            ("api.kimi.com" | "api.kimi.ai", "kimi-for-coding") => Some(Max),
            ("api.kimi.com" | "api.kimi.ai", _) => Some(High),
            ("open.bigmodel.cn" | "api.z.ai", _) => Some(Max),
            (_, models::QWEN4_MAX) if http::is_model_studio(base) => Some(High),
            (_, _) if http::is_model_studio(base) => levels.last().copied(),
            _ => None,
        };
        return Some(Options::known(levels, default));
    }
    let option = |levels: &[EffortLevel], default| Some(Options::known(levels, Some(default)));
    match (host, path, dated_alias(model)) {
        (
            "api.deepseek.com",
            "",
            "deepseek-flash"
            | "deepseek-v4-pro"
            | "deepseek-v4-flash"
            | "deepseek-v4-flash-vision-exp",
        ) => option(&[Off, Low, High, Max], High),
        (
            "api.kimi.com" | "api.kimi.ai",
            "/coding",
            "k3.1-pro" | "k3.1-pro-highspeed" | "k3" | "k3-256k",
        ) => option(&[Low, High, Max], High),
        ("api.kimi.com" | "api.kimi.ai", "/coding", "kimi-for-coding") => {
            option(&[Low, High, Max], Max)
        }
        (
            "open.bigmodel.cn" | "api.z.ai",
            "/api" | "/api/paas/v4" | "/api/coding/paas/v4",
            "glm-5.3" | "glm-5.3-flash" | "glm-5.3-flashx" | "glm-5.3-flashx-legal",
        ) => option(&[Low, High, Max], Max),
        ("api.openai.com", "", "gpt-6-astra" | "gpt-6.1-sol") => {
            option(&EffortLevel::STANDARD, Medium)
        }
        ("api.openai.com", "", "gpt-6.1-terra" | "gpt-6-sol" | "gpt-6-luna") => {
            option(&[Off, Low, Medium, High, XHigh, Max], Medium)
        }
        ("api.openai.com", "", "gpt-5.6" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna") => {
            option(&[Off, Low, Medium, High, XHigh, Max], Medium)
        }
        ("api.openai.com", "", "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.4-nano") => {
            option(&[Off, Low, Medium, High, XHigh], Off)
        }
        ("api.openai.com", "", "gpt-5.5") => option(&[Off, Low, Medium, High, XHigh], Medium),
        ("api.openai.com", "", "gpt-5.4-pro" | "gpt-5.5-pro" | "gpt-5.2-pro")
            if protocol == Protocol::Responses =>
        {
            option(&[Medium, High, XHigh], High)
        }
        ("api.openai.com", "", "gpt-5.2") => option(&[Off, Low, Medium, High, XHigh], Off),
        ("api.openai.com", "", "gpt-5.1") => option(&[Off, Low, Medium, High], Off),
        ("api.openai.com", "", "gpt-5" | "gpt-5-mini" | "gpt-5-nano") => {
            option(&[Minimal, Low, Medium, High], Medium)
        }
        ("api.openai.com", "", "o1" | "o3" | "o3-mini" | "o4-mini") => {
            option(&[Low, Medium, High], Medium)
        }
        ("openrouter.ai", "/api", "z-ai/glm-5.3" | "z-ai/glm-5.3-prime") => {
            option(&[Low, High, Max], Max)
        }
        (_, "/compatible-mode", id) if http::is_model_studio(base) => match id {
            models::QWEN4_MAX => option(&[Low, High, XHigh], High),
            "qwen4-max-preview" | "qwen4.0-max-preview" if models::is_aliyun_token_plan(base) => {
                option(&[Low, Medium, XHigh], XHigh)
            }
            "qwen3.8-max" | "qwen3.8-max-0902" | "qwen3.8-flash" | "qwen3.8-27b" => {
                option(&[Off, Low, Medium, XHigh], XHigh)
            }
            "qwen3.8-2.4t-a95b" => option(&[Low, Medium, XHigh], XHigh),
            "glm-5.3" => option(&[Low, High, Max], Max),
            "glm-5.2" | "deepseek-v4-pro" if protocol == Protocol::Responses => {
                option(&[Off, High, Max], High)
            }
            "glm-5.2" | "glm-5.1" | "glm-5" | "deepseek-v4-pro" => option(&[High, Max], High),
            "deepseek-v4.1-flash"
            | "deepseek-v4.1-flash-1017"
            | "deepseek-v4-pro-0813"
            | "deepseek-v4-flash-0731" => option(
                if protocol == Protocol::Responses {
                    &[Off, Low, High, Max]
                } else {
                    &[Low, High, Max]
                },
                High,
            ),
            "kimi-k3.1-pro" => option(&[Low, Medium, High, Max], Max),
            "kimi-k3" if protocol == Protocol::ChatCompletions => option(&[Low, High, Max], Max),
            "minimax/minimax-m3.1-flash" if protocol == Protocol::ChatCompletions => {
                option(&EffortLevel::STANDARD, Max)
            }
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn kimi_requires_thinking(account: &Account, protocol: Protocol, model: &str) -> bool {
    let Ok(base) = http::provider_base(&endpoint(account, protocol)) else {
        return false;
    };
    matches!(base.host_str(), Some("api.kimi.com" | "api.kimi.ai"))
        && base.path().trim_end_matches('/') == "/coding"
        && matches!(model, "k3.1-pro" | "k3.1-pro-highspeed")
}

pub(crate) async fn discover<'a>(
    root: &Path,
    client: Client,
    store: &Store,
    selected: impl IntoIterator<Item = &'a Model>,
) -> Accounts {
    let mut groups = BTreeMap::<String, (Account, Vec<String>)>::new();
    for model in selected {
        let Ok((_, account)) = store.model(&model.id) else {
            continue;
        };
        let names = &mut groups
            .entry(account.id.clone())
            .or_insert_with(|| (account.clone(), Vec::new()))
            .1;
        if let Ok(name) = http::provider_api_model(&model.model) {
            names.push(name);
        }
        if client == Client::Code {
            names.extend(
                model
                    .code_models
                    .values()
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .filter_map(|name| http::provider_api_model(name).ok()),
            );
        }
    }
    let Ok(transport) = http::ReqwestProviderModels::new(None) else {
        return Accounts::new();
    };
    let fetch = stream::iter(groups)
        .map(|(id, (account, names))| {
            let transport = &transport;
            async move {
                let protocol = account.protocol_for(client);
                let base = http::provider_base(&endpoint(&account, protocol)).ok()?;
                let host = base.host_str()?;
                if (protocol == Protocol::Anthropic && http::is_model_studio(&base))
                    || (host != "openrouter.ai"
                        && names
                            .iter()
                            .all(|name| known(&base, protocol, name).is_some()))
                {
                    return None;
                }
                let key = models::key(root, &account).ok()?;
                let auth = account.auth_for(protocol);
                let header_value = if auth == AuthMode::ApiKey {
                    key
                } else {
                    format!("Bearer {key}")
                };
                let header = (
                    if auth == AuthMode::ApiKey {
                        "x-api-key"
                    } else {
                        "authorization"
                    },
                    header_value.as_str(),
                );
                let mut url = base.clone();
                if host == "api.deepseek.com" && matches!(base.path(), "/" | "/anthropic") {
                    url.set_path("/models");
                } else {
                    url.set_path(&format!("{}/v1/models", base.path().trim_end_matches('/')));
                }
                let urls = [url.to_string()];
                let catalog = http::fetch_provider_catalog_from_urls(transport, &urls, header)
                    .await
                    .ok()?;
                let options = catalog
                    .models
                    .into_iter()
                    .filter(|row| names.contains(&row.id))
                    .filter_map(|row| {
                        row.effort_levels.map(|levels| {
                            (
                                row.id,
                                Options {
                                    levels,
                                    default: row.default_effort,
                                },
                            )
                        })
                    })
                    .collect::<Catalog>();
                Some((id, options))
            }
        })
        .buffer_unordered(4);
    futures_util::pin_mut!(fetch);
    let mut catalogs = Accounts::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(entry) = fetch.next().await {
            catalogs.extend(entry);
        }
    })
    .await;
    catalogs
}

#[cfg(test)]
mod tests {
    use super::*;
    use EffortLevel::{High, Low, Max, Medium, XHigh};

    #[test]
    fn fallback_levels_follow_the_provider_and_protocol() {
        for (protocol, url) in [
            (Protocol::Anthropic, "https://dashscope.aliyuncs.com/apps/anthropic"),
            (Protocol::Responses, "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"),
            (Protocol::ChatCompletions, "https://dashscope.aliyuncs.com/compatible-mode/v1"),
        ] {
            let base = http::provider_base(url).unwrap();
            let official = known(&base, protocol, models::QWEN4_MAX).unwrap();
            assert_eq!(official.levels, [Low, High, XHigh]);
            assert_eq!(official.default, Some(High));
        }
        let qwen = http::provider_base(
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        )
        .unwrap();
        assert_eq!(
            known(&qwen, Protocol::Responses, "qwen4-max-preview")
                .unwrap()
                .levels,
            [Low, Medium, XHigh]
        );
        assert_eq!(
            known(&qwen, Protocol::ChatCompletions, "deepseek-v4-pro")
                .unwrap()
                .levels,
            [High, Max]
        );
        assert_eq!(
            known(&qwen, Protocol::Responses, "deepseek-v4-pro")
                .unwrap()
                .levels,
            [EffortLevel::None, High, Max]
        );
        assert!(known(&qwen, Protocol::Responses, "auto")
            .unwrap()
            .levels
            .is_empty());
        assert!(known(&qwen, Protocol::Responses, "qwen3.7-plus").is_none());
        let kimi = http::provider_base("https://api.kimi.com/coding/v1").unwrap();
        assert!(
            known(&kimi, Protocol::Responses, "kimi-for-coding-highspeed")
                .unwrap()
                .levels
                .is_empty()
        );
        let custom = http::provider_base("https://proxy.example/v1").unwrap();
        assert!(known(&custom, Protocol::Responses, "deepseek-v4-pro").is_none());
    }
}
