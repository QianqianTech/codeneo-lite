use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Official,
    AuthToken,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalVar {
    pub key: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub auth: AuthMode,
    pub model: String,
    pub sonnet_model: String,
    pub opus_model: String,
    pub haiku_model: String,
    pub small_fast_model: String,
    pub sonnet_capabilities: String,
    pub opus_capabilities: String,
    pub haiku_capabilities: String,
    pub timeout_ms: String,
    pub follow_proxy: bool,
    pub optional: Vec<OptionalVar>,
    #[serde(default)]
    pub preset_id: String,
    #[serde(default)]
    pub reasoning_model: String,
    #[serde(default)]
    pub subagent_model: String,
    #[serde(default)]
    pub effort_level: String,
    #[serde(default)]
    pub fable_model: String,
    #[serde(default)]
    pub auto_compact_window: String,
    #[serde(default)]
    pub max_context_tokens: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktop_models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    BaseUrl,
    BothSecrets,
    OfficialHasSecret,
    MissingSecret,
    EmptyName,
    Effort,
    CompactWindow,
    OfficialFields,
    ProbePath,
    DesktopModels,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::BaseUrl => write!(f, "Base URL 必须是 https 地址，末尾的 /v1 会去掉"),
            ProfileError::BothSecrets => write!(f, "ANTHROPIC_API_KEY 和 ANTHROPIC_AUTH_TOKEN 不能同时填写，Claude Code 会优先采用 ANTHROPIC_API_KEY"),
            ProfileError::OfficialHasSecret => write!(f, "官方登录档案不写入 API 密钥"),
            ProfileError::MissingSecret => write!(f, "这个认证方式需要填写密钥"),
            ProfileError::EmptyName => write!(f, "档案需要一个名称"),
            ProfileError::Effort => write!(f, "CLAUDE_CODE_EFFORT_LEVEL 只能是 low、medium、high、xhigh、max"),
            ProfileError::CompactWindow => write!(f, "上下文窗口只能是数字"),
            ProfileError::OfficialFields => write!(f, "官方档案不能填写供应商地址、模型或能力"),
            ProfileError::ProbePath => write!(f, "探测地址不能指向账号或账单接口"),
            ProfileError::DesktopModels => write!(f, "Desktop 模型列表每行填写一个具体的模型 ID，请去掉 auto 和空格"),
        }
    }
}

pub fn normalize_base_url(raw: &str) -> Result<String, ProfileError> {
    let mut text = raw.trim().trim_end_matches('/').to_string();
    if text.is_empty() {
        return Ok(String::new());
    }
    if let Some(stripped) = text.strip_suffix("/v1") {
        text = stripped.trim_end_matches('/').to_string();
    }
    let ok = text.starts_with("https://") && text.len() > "https://".len() && !text[8..].contains(' ');
    if !ok {
        return Err(ProfileError::BaseUrl);
    }
    Ok(text)
}

impl Profile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.name.trim().is_empty() {
            return Err(ProfileError::EmptyName);
        }
        if !self.base_url.is_empty() {
            let normalized = normalize_base_url(&self.base_url)?;
            if normalized != self.base_url {
                return Err(ProfileError::BaseUrl);
            }
        }
        if self.auth != AuthMode::Official && self.base_url.is_empty() {
            return Err(ProfileError::BaseUrl);
        }
        if !self.effort_level.is_empty() && !matches!(self.effort_level.as_str(), "low" | "medium" | "high" | "xhigh" | "max") {
            return Err(ProfileError::Effort);
        }
        if !self.auto_compact_window.is_empty() && !self.auto_compact_window.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(ProfileError::CompactWindow);
        }
        if !self.max_context_tokens.is_empty() && !self.max_context_tokens.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(ProfileError::CompactWindow);
        }
        normalize_desktop_models(&self.desktop_models)?;
        Ok(())
    }

    pub fn official() -> Self {
        Self {
            id: "official".into(),
            name: "官方".into(),
            kind: "官方".into(),
            base_url: String::new(),
            auth: AuthMode::Official,
            model: String::new(),
            sonnet_model: String::new(),
            opus_model: String::new(),
            haiku_model: String::new(),
            small_fast_model: String::new(),
            sonnet_capabilities: String::new(),
            opus_capabilities: String::new(),
            haiku_capabilities: String::new(),
            timeout_ms: String::new(),
            follow_proxy: true,
            optional: default_optional(),
            preset_id: String::new(),
            reasoning_model: String::new(),
            subagent_model: String::new(),
            effort_level: String::new(),
            fable_model: String::new(),
            auto_compact_window: String::new(),
            max_context_tokens: String::new(),
            desktop_models: Vec::new(),
        }
    }
}

pub fn reject_official_supplier_fields(profile: &Profile) -> Result<(), ProfileError> {
    if profile.id != "official" {
        return Ok(());
    }
    if profile.auth != AuthMode::Official {
        return Err(ProfileError::OfficialFields);
    }
    let slots = [
        profile.base_url.as_str(),
        profile.model.as_str(),
        profile.sonnet_model.as_str(),
        profile.opus_model.as_str(),
        profile.haiku_model.as_str(),
        profile.small_fast_model.as_str(),
        profile.sonnet_capabilities.as_str(),
        profile.opus_capabilities.as_str(),
        profile.haiku_capabilities.as_str(),
        profile.reasoning_model.as_str(),
        profile.subagent_model.as_str(),
        profile.fable_model.as_str(),
        profile.effort_level.as_str(),
        profile.auto_compact_window.as_str(),
        profile.max_context_tokens.as_str(),
        profile.preset_id.as_str(),
    ];
    if slots.iter().any(|value| !value.is_empty()) || !profile.desktop_models.is_empty() {
        return Err(ProfileError::OfficialFields);
    }
    for item in &profile.optional {
        if !item.enabled {
            continue;
        }
        if item.key.starts_with("ANTHROPIC_") || matches!(item.key.as_str(), "CLAUDE_CODE_SUBAGENT_MODEL" | "CLAUDE_CODE_EFFORT_LEVEL" | "CLAUDE_CODE_AUTO_COMPACT_WINDOW" | "CLAUDE_CODE_MAX_CONTEXT_TOKENS") {
            return Err(ProfileError::OfficialFields);
        }
    }
    Ok(())
}

pub fn normalize_desktop_models(models: &[String]) -> Result<Vec<String>, ProfileError> {
    let mut names = Vec::new();
    for raw in models {
        let name = raw.trim();
        let name = name.strip_suffix("[1m]").unwrap_or(name).trim();
        if name.is_empty() || name.len() > 256 || name.eq_ignore_ascii_case("auto")
            || name.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return Err(ProfileError::DesktopModels);
        }
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

pub fn check_secrets(auth: AuthMode, api_key: &str, auth_token: &str) -> Result<(), ProfileError> {
    let has_key = !api_key.trim().is_empty();
    let has_token = !auth_token.trim().is_empty();
    if has_key && has_token {
        return Err(ProfileError::BothSecrets);
    }
    match auth {
        AuthMode::Official if has_key || has_token => Err(ProfileError::OfficialHasSecret),
        AuthMode::AuthToken if !has_token => Err(ProfileError::MissingSecret),
        AuthMode::ApiKey if !has_key => Err(ProfileError::MissingSecret),
        _ => Ok(()),
    }
}

pub fn merge_optional(input: &[OptionalVar]) -> Vec<OptionalVar> {
    default_optional()
        .into_iter()
        .map(|mut item| {
            if let Some(found) = input.iter().find(|candidate| candidate.key == item.key) {
                item.enabled = found.enabled;
                item.value = found.value.clone();
            }
            item
        })
        .collect()
}

pub fn default_optional() -> Vec<OptionalVar> {
    ["API_TIMEOUT_MS", "CLAUDE_CODE_MAX_RETRIES", "DISABLE_TELEMETRY", "DISABLE_AUTOUPDATER", "DISABLE_ERROR_REPORTING", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "language"]
        .into_iter()
        .map(|key| OptionalVar { key: key.into(), value: String::new(), enabled: false })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_v1_and_rejects_http() {
        assert_eq!(normalize_base_url("https://example.com/v1").unwrap(), "https://example.com");
        assert!(normalize_base_url("http://example.com").is_err());
        for url in ["https://api.deepseek.com/anthropic", "https://api.moonshot.cn/anthropic", "https://open.bigmodel.cn/api/anthropic"] {
            assert_eq!(normalize_base_url(url).unwrap(), url);
        }
        assert_eq!(normalize_base_url("https://api.deepseek.com/anthropic/v1").unwrap(), "https://api.deepseek.com/anthropic");
    }

    #[test]
    fn old_profile_json_keeps_its_fields_when_new_slots_are_absent() {
        let profile: Profile = serde_json::from_str(r#"{"id":"gw","name":"网关","kind":"自定义兼容网关","base_url":"https://gateway.example","auth":"auth_token","model":"m","sonnet_model":"","opus_model":"","haiku_model":"","small_fast_model":"","sonnet_capabilities":"","opus_capabilities":"","haiku_capabilities":"","timeout_ms":"","follow_proxy":true,"optional":[]}"#).unwrap();
        assert_eq!(profile.base_url, "https://gateway.example");
        assert_eq!(profile.auth, AuthMode::AuthToken);
        assert!(profile.preset_id.is_empty());
        assert!(profile.effort_level.is_empty());
        assert!(profile.auto_compact_window.is_empty());
    }

    #[test]
    fn effort_and_compact_window_are_checked() {
        let mut profile = Profile::official();
        profile.auth = AuthMode::AuthToken;
        profile.base_url = "https://gateway.example".into();
        profile.effort_level = "turbo".into();
        assert!(matches!(profile.validate(), Err(ProfileError::Effort)));
        profile.effort_level = "max".into();
        profile.auto_compact_window = "1m".into();
        assert!(matches!(profile.validate(), Err(ProfileError::CompactWindow)));
        profile.auto_compact_window = "786432".into();
        assert!(profile.validate().is_ok());
    }
}
