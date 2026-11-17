use crate::{default_optional, AuthMode, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub docs_url: &'static str,
    pub base_url: &'static str,
    pub auth: AuthMode,
    pub model: &'static str,
    pub sonnet_model: &'static str,
    pub opus_model: &'static str,
    pub haiku_model: &'static str,
    pub small_fast_model: &'static str,
    pub subagent_model: &'static str,
    pub reasoning_model: &'static str,
    pub fable_model: &'static str,
    pub effort_level: &'static str,
    pub auto_compact_window: &'static str,
    pub max_context_tokens: &'static str,
    pub verified_on: &'static str,
}

const VERIFIED_ON: &str = "2026-10-26";

pub fn provider_presets() -> &'static [ProviderPreset] {
    &[
        ProviderPreset {
            id: "deepseek",
            name: "DeepSeek",
            kind: "DeepSeek",
            docs_url: "https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code",
            base_url: "https://api.deepseek.com/anthropic",
            auth: AuthMode::AuthToken,
            model: "deepseek-flash[1m]",
            sonnet_model: "deepseek-flash[1m]",
            opus_model: "deepseek-flash[1m]",
            haiku_model: "deepseek-flash",
            small_fast_model: "",
            subagent_model: "deepseek-flash",
            reasoning_model: "",
            fable_model: "",
            effort_level: "max",
            auto_compact_window: "786432",
            max_context_tokens: "",
            verified_on: VERIFIED_ON,
        },
        ProviderPreset {
            id: "kimi",
            name: "Kimi K3",
            kind: "Kimi Code",
            docs_url: "https://www.kimi.com/code/docs/third-party-tools/claude-code.html",
            base_url: "https://api.kimi.com/coding",
            auth: AuthMode::ApiKey,
            model: "k3",
            sonnet_model: "k3",
            opus_model: "k3",
            haiku_model: "k3",
            small_fast_model: "",
            subagent_model: "k3",
            reasoning_model: "",
            fable_model: "k3",
            effort_level: "high",
            auto_compact_window: "262144",
            max_context_tokens: "262144",
            verified_on: "2026-11-11",
        },
        ProviderPreset {
            id: "kimi-pro",
            name: "Kimi K3.1 Pro",
            kind: "Kimi Code",
            docs_url: "https://www.kimi.com/code/docs/third-party-tools/claude-code.html",
            base_url: "https://api.kimi.com/coding",
            auth: AuthMode::ApiKey,
            model: "k3.1-pro",
            sonnet_model: "k3.1-pro",
            opus_model: "k3.1-pro",
            haiku_model: "k3.1-pro",
            small_fast_model: "",
            subagent_model: "k3.1-pro",
            reasoning_model: "",
            fable_model: "k3.1-pro",
            effort_level: "high",
            auto_compact_window: "262144",
            max_context_tokens: "262144",
            verified_on: "2026-11-11",
        },
        ProviderPreset {
            id: "openrouter",
            name: "OpenRouter",
            kind: "OpenRouter",
            docs_url: "https://openrouter.ai/docs/guides/coding-agents/claude-code-integration",
            base_url: "https://openrouter.ai/api",
            auth: AuthMode::AuthToken,
            model: "~anthropic/claude-sonnet-latest[1m]",
            sonnet_model: "~anthropic/claude-sonnet-latest[1m]",
            opus_model: "~anthropic/claude-opus-latest[1m]",
            haiku_model: "~anthropic/claude-haiku-latest",
            small_fast_model: "",
            subagent_model: "~anthropic/claude-opus-latest[1m]",
            reasoning_model: "",
            fable_model: "~anthropic/claude-fable-latest[1m]",
            effort_level: "",
            auto_compact_window: "",
            max_context_tokens: "",
            verified_on: "2026-10-27",
        },
        ProviderPreset {
            id: "glm",
            name: "智谱 GLM",
            kind: "智谱 GLM",
            docs_url: "https://docs.bigmodel.cn/cn/coding-plan/tool/claude",
            base_url: "https://open.bigmodel.cn/api/anthropic",
            auth: AuthMode::AuthToken,
            model: "glm-5.3[1m]",
            sonnet_model: "glm-5.3[1m]",
            opus_model: "glm-5.3[1m]",
            haiku_model: "glm-5.3-flash[1m]",
            small_fast_model: "",
            subagent_model: "glm-5.3-flash[1m]",
            reasoning_model: "",
            fable_model: "",
            effort_level: "",
            auto_compact_window: "1000000",
            max_context_tokens: "1000000",
            verified_on: "2026-11-11",
        },
        ProviderPreset {
            id: "qwen",
            name: "千问 Token Plan",
            kind: "通义千问",
            docs_url: "https://help.aliyun.com/zh/model-studio/claude-code",
            base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic",
            auth: AuthMode::AuthToken,
            model: "auto",
            sonnet_model: "qwen3.8-flash",
            opus_model: "qwen4-max",
            haiku_model: "qwen3.6-flash",
            small_fast_model: "",
            subagent_model: "auto",
            reasoning_model: "",
            fable_model: "",
            effort_level: "",
            auto_compact_window: "",
            max_context_tokens: "983616",
            verified_on: "2026-11-17",
        },
    ]
}

pub fn preset_by_id(id: &str) -> Option<&'static ProviderPreset> {
    provider_presets().iter().find(|preset| preset.id == id)
}

pub fn profile_from_preset(preset: &ProviderPreset) -> Profile {
    Profile {
        id: preset.id.into(),
        name: preset.name.into(),
        kind: preset.kind.into(),
        base_url: preset.base_url.into(),
        auth: preset.auth,
        model: preset.model.into(),
        sonnet_model: preset.sonnet_model.into(),
        opus_model: preset.opus_model.into(),
        haiku_model: preset.haiku_model.into(),
        small_fast_model: preset.small_fast_model.into(),
        sonnet_capabilities: String::new(),
        opus_capabilities: String::new(),
        haiku_capabilities: String::new(),
        timeout_ms: String::new(),
        follow_proxy: true,
        optional: default_optional(),
        preset_id: preset.id.into(),
        reasoning_model: preset.reasoning_model.into(),
        subagent_model: preset.subagent_model.into(),
        effort_level: preset.effort_level.into(),
        fable_model: preset.fable_model.into(),
        auto_compact_window: preset.auto_compact_window.into(),
        max_context_tokens: preset.max_context_tokens.into(),
        desktop_models: Vec::new(),
    }
}

/// 国内接口不走代理。官方和境外网关走应用代理。
pub fn direct_provider(base_url: &str) -> bool {
    url_host(base_url).is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        matches!(
            host.as_str(),
            "api.kimi.com" | "api.moonshot.cn" | "open.bigmodel.cn" | "api.deepseek.com" | "api.z.ai" | "dashscope.aliyuncs.com"
        ) || host.ends_with(".dashscope.aliyuncs.com")
            || host.ends_with(".maas.aliyuncs.com")
    })
}

pub fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let hostport = rest.split(['/', '?', '#']).next()?;
    let host = hostport.split('@').next_back()?;
    Some(host.split(':').next().unwrap_or(host))
}

/// 带密钥探测时要钉住的目录。`preset_id` 优先；空则用档案 id 对照目录，覆盖旧种子行。
pub fn pinned_preset(profile_id: &str, preset_id: &str) -> Option<&'static ProviderPreset> {
    if !preset_id.is_empty() {
        return preset_by_id(preset_id);
    }
    preset_by_id(profile_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize_base_url;

    #[test]
    fn catalog_matches_vendor_claude_code_pages() {
        let deepseek = preset_by_id("deepseek").unwrap();
        assert_eq!(deepseek.base_url, "https://api.deepseek.com/anthropic");
        assert_eq!(deepseek.auth, AuthMode::AuthToken);
        assert_eq!(deepseek.model, "deepseek-flash[1m]");
        assert_eq!(deepseek.opus_model, "deepseek-flash[1m]");
        assert_eq!(deepseek.sonnet_model, "deepseek-flash[1m]");
        assert_eq!(deepseek.haiku_model, "deepseek-flash");
        assert_eq!(deepseek.subagent_model, "deepseek-flash");
        assert_eq!(deepseek.effort_level, "max");
        assert_eq!(deepseek.auto_compact_window, "786432");
        assert!(deepseek.model != "deepseek-chat" && deepseek.model != "deepseek-reasoner" && deepseek.model != "deepseek-v4-flash");

        let kimi = preset_by_id("kimi").unwrap();
        assert_eq!(kimi.name, "Kimi K3");
        assert_eq!(kimi.kind, "Kimi Code");
        assert_eq!(kimi.base_url, "https://api.kimi.com/coding");
        assert_eq!(kimi.auth, AuthMode::ApiKey);
        assert_eq!(kimi.model, "k3");
        for slot in [kimi.opus_model, kimi.sonnet_model, kimi.haiku_model, kimi.fable_model, kimi.subagent_model] {
            assert_eq!(slot, "k3");
        }
        assert_eq!(kimi.effort_level, "high");
        assert_eq!(kimi.auto_compact_window, "262144");
        assert_eq!(kimi.max_context_tokens, "262144");
        let pro = preset_by_id("kimi-pro").unwrap();
        assert_eq!(pro.base_url, kimi.base_url);
        assert_eq!(pro.auth, AuthMode::ApiKey);
        assert_eq!(pro.model, "k3.1-pro");
        assert_eq!(pro.haiku_model, "k3.1-pro");
        assert!(direct_provider(kimi.base_url));
        assert!(direct_provider("https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic"));
        assert!(!direct_provider("https://openrouter.ai/api"));
        let qwen = preset_by_id("qwen").unwrap();
        assert_eq!(qwen.base_url, "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic");
        assert_eq!(qwen.auth, AuthMode::AuthToken);
        assert_eq!(qwen.model, "auto");
        assert_eq!(qwen.haiku_model, "qwen3.6-flash");
        assert_eq!(qwen.sonnet_model, "qwen3.8-flash");
        assert_eq!(qwen.opus_model, "qwen4-max");
        assert_eq!(qwen.subagent_model, "auto");
        assert_eq!(qwen.max_context_tokens, "983616");

        let openrouter = preset_by_id("openrouter").unwrap();
        assert_eq!(openrouter.base_url, "https://openrouter.ai/api");
        assert_eq!(openrouter.auth, AuthMode::AuthToken);
        assert_eq!(openrouter.model, "~anthropic/claude-sonnet-latest[1m]");
        assert_eq!(openrouter.opus_model, "~anthropic/claude-opus-latest[1m]");
        assert_eq!(openrouter.haiku_model, "~anthropic/claude-haiku-latest");
        assert!(openrouter.effort_level.is_empty());
        assert!(openrouter.auto_compact_window.is_empty());
        assert!(preset_by_id("kimi-global").is_none());

        let glm = preset_by_id("glm").unwrap();
        assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/anthropic");
        assert_eq!(glm.auth, AuthMode::AuthToken);
        assert_eq!(glm.model, "glm-5.3[1m]");
        assert_eq!(glm.opus_model, "glm-5.3[1m]");
        assert_eq!(glm.sonnet_model, "glm-5.3[1m]");
        assert_eq!(glm.haiku_model, "glm-5.3-flash[1m]");
        assert_eq!(glm.subagent_model, "glm-5.3-flash[1m]");
        assert_eq!(glm.auto_compact_window, "1000000");
        assert_eq!(glm.max_context_tokens, "1000000");
        assert!(glm.effort_level.is_empty());
        assert_ne!(glm.model, "glm-5.3-flash[1m]");

        for preset in provider_presets() {
            assert_eq!(normalize_base_url(preset.base_url).unwrap(), preset.base_url);
            let verified = match preset.id {
                "openrouter" => "2026-10-27",
                "kimi" | "kimi-pro" | "glm" => "2026-11-11",
                "qwen" => "2026-11-17",
                _ => VERIFIED_ON,
            };
            assert_eq!(preset.verified_on, verified);
            assert!(preset.reasoning_model.is_empty());
            assert!(preset.small_fast_model.is_empty());
            let profile = profile_from_preset(preset);
            assert_eq!(profile.preset_id, preset.id);
            assert_eq!(profile.id, preset.id);
            assert!(profile.sonnet_capabilities.is_empty());
            assert!(profile.opus_capabilities.is_empty());
            assert!(profile.haiku_capabilities.is_empty());
            assert!(profile.validate().is_ok());
            assert!(!format!("{preset:?}").contains("sk-"));
        }
        assert!(preset_by_id("official").is_none());
    }
}
