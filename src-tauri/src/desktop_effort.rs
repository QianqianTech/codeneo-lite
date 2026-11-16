//! Reasoning levels for Claude Desktop's Anthropic Messages routes.
use crate::model_transport::{self, EffortLevel};
use claudekeeper_claude::AuthMode;
use reqwest::Url;
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

pub(crate) type Catalog = BTreeMap<String, Vec<EffortLevel>>;

#[derive(Clone, Copy)]
pub(crate) struct Effort {
    behaves_as: &'static str,
    ceiling: Option<&'static str>,
    mapping: Mapping,
}

#[derive(Clone, Copy)]
enum Mapping {
    Native,
    LowHighMax { xhigh: &'static str },
    HighMax,
    LowMediumHighMax,
    LowMediumXhigh,
    Catalog([bool; EffortLevel::ALL.len()]),
}

const SONNET_46: &str = "claude-sonnet-4-6";

impl Effort {
    fn four_levels(mapping: Mapping) -> Self {
        Self {
            behaves_as: SONNET_46,
            ceiling: Some("max"),
            mapping,
        }
    }

    fn qwen() -> Self {
        Self {
            behaves_as: "claude-sonnet-5",
            ceiling: Some("xhigh"),
            mapping: Mapping::LowMediumXhigh,
        }
    }

    fn claude(behaves_as: &'static str) -> Self {
        Self {
            behaves_as,
            ceiling: None,
            mapping: Mapping::Native,
        }
    }

    fn from_catalog(levels: &[EffortLevel], native: Option<Self>) -> Option<Self> {
        let available = EffortLevel::ALL.map(|level| {
            EffortLevel::STANDARD.contains(&level) && levels.contains(&level)
        });
        if available.iter().filter(|&&supported| supported).count() < 2 {
            return None;
        }
        let ceiling = EffortLevel::ALL
            .into_iter()
            .rev()
            .find(|&level| available[level as usize])?;
        let behaves_as = native
            .filter(|effort| matches!(effort.mapping, Mapping::Native))
            .map(|effort| effort.behaves_as)
            .unwrap_or_else(|| {
                if levels.contains(&EffortLevel::XHigh) {
                    "claude-sonnet-5"
                } else {
                    SONNET_46
                }
            });
        Some(Self {
            behaves_as,
            ceiling: Some(ceiling.as_str()),
            mapping: Mapping::Catalog(available),
        })
    }

    pub(crate) fn annotate(self, entry: &mut Value) {
        entry["behavesAs"] = json!(self.behaves_as);
        if let Some(ceiling) = self.ceiling {
            entry["maxEffort"] = json!(ceiling);
        }
    }

    pub(crate) fn adapt(self, request: &mut Value) -> bool {
        let Some(Value::String(effort)) = request.pointer_mut("/output_config/effort") else {
            return false;
        };
        let mapped = match (self.mapping, effort.as_str()) {
            (Mapping::LowHighMax { .. } | Mapping::HighMax, "medium") => "high",
            (Mapping::LowHighMax { xhigh }, "xhigh") => xhigh,
            (Mapping::HighMax, "low") => "high",
            (Mapping::HighMax | Mapping::LowMediumHighMax, "xhigh") => "max",
            (Mapping::LowMediumXhigh, "high" | "max") => "xhigh",
            (Mapping::Catalog(available), value) => {
                let Some(level) = EffortLevel::parse(value)
                    .filter(|level| EffortLevel::STANDARD.contains(level))
                else {
                    return false;
                };
                let index = level as usize;
                if available[index] {
                    return false;
                }
                // Native pickers cap the top level but can still offer gaps
                // below it. Use the closest supported effort in that case.
                let nearest = EffortLevel::ALL
                    .into_iter()
                    .filter(|level| available[*level as usize])
                    .min_by_key(|level| (*level as usize).abs_diff(index))
                    .expect("an advertised effort level");
                nearest.as_str()
            }
            _ => return false,
        };
        *effort = mapped.into();
        true
    }
}

fn claude_model(model: &str) -> Option<&'static str> {
    let canonical = model
        .strip_prefix("anthropic/")
        .unwrap_or(model)
        .replace('.', "-");
    let undated = canonical
        .rsplit_once('-')
        .filter(|(_, date)| {
            date.len() == 8 && date.starts_with("20") && date.bytes().all(|b| b.is_ascii_digit())
        })
        .map_or(canonical.as_str(), |(name, _)| name);
    [
        "claude-opus-4-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-opus-5",
        "claude-opus-5-5",
        SONNET_46,
        "claude-sonnet-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-fable-5",
        "claude-fable-5-1",
        "claude-fable-5-2",
    ]
    .into_iter()
    .find(|name| *name == undated)
}

pub(crate) fn model_studio_host(base: &Url) -> bool {
    base.host_str().is_some_and(|host| {
            matches!(
                host,
                "dashscope.aliyuncs.com" | "dashscope-intl.aliyuncs.com"
            ) || host.ends_with(".maas.aliyuncs.com")
        })
}

fn model_studio(base: &Url) -> bool {
    base.path().trim_end_matches('/') == "/apps/anthropic" && model_studio_host(base)
}

pub(crate) fn known_levels(base: &Url, model: &str) -> Option<&'static [EffortLevel]> {
    use EffortLevel::*;
    let effort = support(base, model)?;
    Some(match effort.mapping {
        Mapping::LowHighMax { .. } => &[Low, High, Max],
        Mapping::HighMax => &[High, Max],
        Mapping::LowMediumHighMax => &[Low, Medium, High, Max],
        Mapping::LowMediumXhigh => &[Low, Medium, XHigh],
        Mapping::Native => match effort.behaves_as {
            "claude-opus-4-5" => &[Low, Medium, High],
            "claude-opus-4-6" | SONNET_46 => &[Low, Medium, High, Max],
            _ => &EffortLevel::STANDARD,
        },
        Mapping::Catalog(_) => return Option::None,
    })
}

pub(crate) fn resolve(base: &Url, model: &str, catalog: &Catalog) -> Option<Effort> {
    let known = support(base, model);
    match catalog.get(model) {
        Some(levels) => Effort::from_catalog(levels, known),
        None => known,
    }
}

fn support(base: &Url, model: &str) -> Option<Effort> {
    if base.scheme() != "https" {
        return None;
    }
    let path = base.path().trim_end_matches('/');
    let model = model.to_ascii_lowercase();
    let three = || Effort::four_levels(Mapping::LowHighMax { xhigh: "max" });
    let deepseek = || Effort::four_levels(Mapping::LowHighMax { xhigh: "high" });
    let two = || Effort::four_levels(Mapping::HighMax);
    let four = || Effort::four_levels(Mapping::LowMediumHighMax);

    // Model IDs can be identical on services with different effort parameters.
    if model_studio(base) {
        return match model.as_str() {
            "qwen4-max-preview" | "qwen4.0-max-preview"
                if crate::models::is_aliyun_token_plan(base) =>
            {
                Some(Effort::qwen())
            }
            "qwen3.8-max" | "qwen3.8-max-0902" | "qwen3.8-max-2026-09-02" | "qwen3.8-flash" => {
                Some(Effort::qwen())
            }
            "glm-5.3" => Some(three()),
            "glm-5.2" | "deepseek-v4-pro" => Some(two()),
            "kimi-k3.1-pro" => Some(four()),
            _ => None,
        };
    }
    match (base.host_str()?, path, model.as_str()) {
        (
            "api.deepseek.com",
            "/anthropic",
            "deepseek-flash"
            | "deepseek-v4-pro"
            | "deepseek-v4-flash"
            | "deepseek-v4-flash-vision-exp",
        ) => Some(deepseek()),
        (
            "api.kimi.com" | "api.kimi.ai",
            "/coding",
            "k3.1-pro" | "k3.1-pro-highspeed" | "k3" | "k3-256k" | "kimi-for-coding",
        ) => Some(three()),
        (
            "open.bigmodel.cn" | "api.z.ai",
            "/api/anthropic",
            "glm-5.3" | "glm-5.3-flash" | "glm-5.3-flashx" | "glm-5.3-flashx-legal",
        ) => Some(three()),
        ("openrouter.ai", "/api", "z-ai/glm-5.3" | "z-ai/glm-5.3-prime") => Some(three()),
        ("openrouter.ai", "/api", id) if id.starts_with("anthropic/") => {
            claude_model(id).map(Effort::claude)
        }
        ("api.anthropic.com", "", id) => claude_model(id).map(Effort::claude),
        _ => None,
    }
}

pub(crate) async fn discover(
    base: &Url,
    auth: AuthMode,
    key: &str,
    names: &[String],
) -> Option<Catalog> {
    let openrouter =
        base.host_str() == Some("openrouter.ai") && base.path().trim_end_matches('/') == "/api";
    // Model Studio's Anthropic endpoint does not offer a model catalogue.
    // Other known deployments only need discovery for unrecognised models.
    if model_studio(base) || (!openrouter && names.iter().all(|name| support(base, name).is_some()))
    {
        return None;
    }
    // Catalogue reads are optional: a slow or unavailable endpoint must not
    // delay applying a model indefinitely.
    let transport = model_transport::ReqwestProviderModels::new(None).ok()?;
    let bearer;
    let header = if auth == AuthMode::ApiKey {
        ("x-api-key", key)
    } else {
        bearer = format!("Bearer {key}");
        ("authorization", bearer.as_str())
    };
    let fetched = tokio::time::timeout(
        Duration::from_secs(2),
        model_transport::fetch_provider_catalog(&transport, base, header),
    )
    .await;
    let catalog = fetched.ok()?.ok()?;
    // A fallback to an OpenAI endpoint isn't evidence about Anthropic
    // Messages. DeepSeek's official /models endpoint describes both protocols.
    let anthropic_route = format!("{}/v1/models", base.path().trim_end_matches('/'));
    let permitted_catalog = Url::parse(&catalog.source_url).is_ok_and(|url| {
        url.path() == anthropic_route
            || (base.host_str() == Some("api.deepseek.com") && url.path() == "/models")
    });
    if !permitted_catalog {
        return None;
    }
    Some(
        catalog
            .models
            .into_iter()
            .filter(|item| names.contains(&item.id))
            .filter_map(|item| item.effort_levels.map(|levels| (item.id, levels)))
            .collect(),
    )
}

pub(crate) fn annotate_direct(base: &Url, config: &mut Value) {
    if let Some(entries) = config["inferenceModels"].as_array_mut() {
        for entry in entries {
            if let Some(effort) = entry["name"].as_str().and_then(|name| support(base, name)) {
                effort.annotate(entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN_PLAN: &str = "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic";

    #[test]
    fn uses_the_actual_endpoint_and_model_when_choosing_effort_levels() {
        for (base, model, ceiling, levels) in [
            (
                TOKEN_PLAN,
                "qwen4-max-preview",
                "xhigh",
                ["low", "medium", "xhigh", "xhigh", "xhigh"],
            ),
            (
                TOKEN_PLAN,
                "qwen3.8-max",
                "xhigh",
                ["low", "medium", "xhigh", "xhigh", "xhigh"],
            ),
            (
                TOKEN_PLAN,
                "glm-5.3",
                "max",
                ["low", "high", "high", "max", "max"],
            ),
            (
                TOKEN_PLAN,
                "deepseek-v4-pro",
                "max",
                ["high", "high", "high", "max", "max"],
            ),
            (
                "https://api.deepseek.com/anthropic",
                "deepseek-v4-pro",
                "max",
                ["low", "high", "high", "high", "max"],
            ),
            (
                TOKEN_PLAN,
                "kimi-k3.1-pro",
                "max",
                ["low", "medium", "high", "max", "max"],
            ),
            (
                "https://api.kimi.com/coding",
                "k3.1-pro",
                "max",
                ["low", "high", "high", "max", "max"],
            ),
            (
                "https://open.bigmodel.cn/api/anthropic",
                "glm-5.3-flash",
                "max",
                ["low", "high", "high", "max", "max"],
            ),
        ] {
            let spec = support(&Url::parse(base).unwrap(), model).unwrap();
            let mut entry = json!({"name":"alias"});
            spec.annotate(&mut entry);
            assert_eq!(entry["maxEffort"], ceiling, "{base} {model}");
            for (from, to) in ["low", "medium", "high", "xhigh", "max"]
                .into_iter()
                .zip(levels)
            {
                let mut request = json!({"output_config":{"effort":from,"format":{"type":"json_schema"}},"thinking":{"type":"adaptive"}});
                assert_eq!(
                    spec.adapt(&mut request),
                    from != to,
                    "{base} {model} {from}"
                );
                assert_eq!(request["output_config"]["effort"], to, "{base} {model}");
                assert_eq!(
                    request["output_config"]["format"],
                    json!({"type":"json_schema"})
                );
                assert_eq!(request["thinking"], json!({"type":"adaptive"}));
            }
        }
    }

    #[test]
    fn omits_effort_for_automatic_fixed_and_unverified_models() {
        for (base, model) in [
            (TOKEN_PLAN, "auto"),
            (TOKEN_PLAN, "MiniMax-M2.5"),
            (TOKEN_PLAN, "qwen3.7-plus"),
            ("https://api.kimi.ai/coding", "kimi-for-coding-highspeed"),
            ("https://proxy.example/apps/anthropic", "glm-5.3"),
        ] {
            assert!(
                support(&Url::parse(base).unwrap(), model).is_none(),
                "{base} {model}"
            );
        }
        let spec = support(&Url::parse(TOKEN_PLAN).unwrap(), "glm-5.3").unwrap();
        let mut defaults = json!({"messages":[]});
        assert!(!spec.adapt(&mut defaults));
        assert_eq!(defaults, json!({"messages":[]}));
    }

    #[test]
    fn maps_openrouter_claude_spelling_to_the_clients_model_catalog() {
        let base = Url::parse("https://openrouter.ai/api").unwrap();
        let mut config = json!({"inferenceModels":[
            {"name":"anthropic/claude-sonnet-5.5"}, {"name":"anthropic/claude-haiku-4.5"}
        ]});
        annotate_direct(&base, &mut config);
        assert_eq!(
            config["inferenceModels"][0]["behavesAs"],
            "claude-sonnet-5-5"
        );
        assert!(config["inferenceModels"][0].get("maxEffort").is_none());
        assert!(config["inferenceModels"][1].get("behavesAs").is_none());
    }

    #[test]
    fn uses_advertised_efforts_for_arbitrary_models_and_skips_fixed_models() {
        let base = Url::parse("https://example.invalid/anthropic").unwrap();
        let catalog = Catalog::from([
            ("new".into(), vec![EffortLevel::Medium, EffortLevel::XHigh]),
            ("fixed".into(), vec![EffortLevel::Max]),
        ]);
        let chosen = resolve(&base, "new", &catalog).unwrap();
        let mut entry = json!({"name":"model"});
        chosen.annotate(&mut entry);
        assert_eq!(entry["maxEffort"], "xhigh");
        assert_eq!(entry["behavesAs"], "claude-sonnet-5");
        let mut request = json!({"output_config":{"effort":"low"}});
        assert!(chosen.adapt(&mut request));
        assert_eq!(request["output_config"]["effort"], "medium");
        let mut request = json!({"output_config":{"effort":"xhigh"}});
        assert!(!chosen.adapt(&mut request));
        assert!(resolve(&base, "fixed", &catalog).is_none());
        assert!(resolve(&base, "absent", &catalog).is_none());
    }
}
