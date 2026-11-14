use crate::{AuthMode, Profile};
use serde_json::{json, Value};

/// Desktop 3P 使用供应商真实模型名。不要把 Kimi / GLM 标成 Claude 模型。
pub fn desktop_gateway_config(
    profile: &Profile,
    secret: &str,
    http_port: u16,
) -> Result<Value, String> {
    profile.validate().map_err(|e| e.to_string())?;
    if profile.auth == AuthMode::Official || secret.is_empty() {
        return Err("Desktop 网关配置需要供应商接口和密钥".into());
    }
    let models: Vec<_> = desktop_models(profile)
        .into_iter()
        .map(|(name, supports1m)| json!({ "name": name, "supports1m": supports1m }))
        .collect();
    if models.is_empty() {
        return Err("请先填写要给 Cowork 使用的模型名".into());
    }
    let mut config = json!({
        "inferenceProvider": "gateway",
        "inferenceGatewayBaseUrl": profile.base_url,
        "inferenceGatewayApiKey": secret,
        "inferenceGatewayAuthScheme": if profile.auth == AuthMode::ApiKey { "x-api-key" } else { "bearer" },
        "inferenceModels": models,
        "modelDiscoveryEnabled": false,
        "isClaudeCodeForDesktopEnabled": true,
        "desktopHome": "standard",
    });
    if http_port != 0 {
        config["egressProxyUrl"] = json!(format!("http://127.0.0.1:{http_port}"));
    }
    Ok(config)
}

pub fn desktop_models(profile: &Profile) -> Vec<(String, bool)> {
    let role_models = [
        &profile.model,
        &profile.opus_model,
        &profile.sonnet_model,
        &profile.haiku_model,
        &profile.subagent_model,
        &profile.fable_model,
    ];
    let mut models = Vec::new();
    let configured = profile.desktop_models.iter();
    let defaults = role_models.into_iter().filter(|_| profile.desktop_models.is_empty());
    for raw in configured.chain(defaults) {
        let Some((name, supports1m)) = context_name(raw) else {
            continue;
        };
        if !models.iter().any(|(existing, _)| existing == &name) {
            models.push((name, supports1m));
        }
    }
    models
}

fn context_name(raw: &str) -> Option<(String, bool)> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return None;
    }
    if let Some(name) = raw.strip_suffix("[1m]") {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        Some((name.to_string(), true))
    } else {
        Some((raw.to_string(), false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_accepts_vendor_model_ids_without_relabeling_them() {
        let mut p = Profile::official();
        p.id = "kimi".into();
        p.auth = AuthMode::ApiKey;
        p.base_url = "https://api.kimi.com/coding".into();
        p.model = "k3".into();
        p.haiku_model = "k3".into();
        let config = desktop_gateway_config(&p, "fixture-key", 0).unwrap();
        assert_eq!(config["inferenceGatewayAuthScheme"], "x-api-key");
        assert_eq!(config["inferenceModels"][0]["name"], "k3");
        assert_eq!(config["inferenceModels"][0]["supports1m"], false);
        assert_eq!(config["modelDiscoveryEnabled"], false);
        assert_eq!(config["desktopHome"], "standard");
        assert!(config.get("chatTabEnabled").is_none());
        assert!(config.get("egressProxyUrl").is_none());
        assert!(config.get("behavesAs").is_none());
        assert!(config["inferenceModels"][0].get("behavesAs").is_none());

        p.auth = AuthMode::AuthToken;
        p.base_url = "https://open.bigmodel.cn/api/anthropic".into();
        p.model = "glm-5.3[1m]".into();
        p.haiku_model = "glm-5.3-flash[1m]".into();
        let config = desktop_gateway_config(&p, "fixture-key", 3128).unwrap();
        assert_eq!(config["inferenceGatewayAuthScheme"], "bearer");
        assert_eq!(config["inferenceModels"][0]["name"], "glm-5.3");
        assert_eq!(config["inferenceModels"][0]["supports1m"], true);
        assert_eq!(config["inferenceModels"][1]["name"], "glm-5.3-flash");
        assert_eq!(config["egressProxyUrl"], "http://127.0.0.1:3128");

        p.model = "".into();
        p.haiku_model = "".into();
        p.opus_model.clear();
        p.sonnet_model.clear();
        p.subagent_model.clear();
        p.fable_model.clear();
        assert!(desktop_gateway_config(&p, "fixture-key", 3128).is_err());
    }
}
