use crate::{AuthMode, Profile, ProfileError};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum SettingsError {
    Profile(ProfileError),
    Io(std::io::Error),
    Json(String),
    NotConfirmed,
    ForbiddenField,
    ConcurrentChange,
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Profile(err) => write!(f, "{err}"),
            SettingsError::Io(err) => write!(f, "{err}"),
            SettingsError::Json(err) => write!(f, "配置不是合法 JSON: {err}"),
            SettingsError::NotConfirmed => write!(f, "写入 settings.json 前需要确认"),
            SettingsError::ForbiddenField => write!(f, "档案不能包含 machineID 或 userID"),
            SettingsError::ConcurrentChange => write!(f, "配置在应用期间被外部修改，请重新预览后重试；未覆盖外部变更"),
        }
    }
}

pub struct ProfileApply {
    pub secret: Option<String>,
    pub proxy_http: Option<String>,
    pub confirmed: bool,
}

pub fn apply_profile(path: &Path, profile: &Profile, apply: &ProfileApply, backup_dir: &Path) -> Result<(), SettingsError> {
    let (before, after) = prepare_profile(path, profile, apply)?;
    backup_file(path, backup_dir)?;
    let now = read_settings_bytes(path)?;
    if now != before { return Err(SettingsError::ConcurrentChange); }
    write_atomic(path, &after)
}

/// Construct the complete write before mutating any target in a multi-file operation.
pub fn prepare_profile(path: &Path, profile: &Profile, apply: &ProfileApply) -> Result<(Option<Vec<u8>>, Vec<u8>), SettingsError> {
    if !apply.confirmed {
        return Err(SettingsError::NotConfirmed);
    }
    profile.validate().map_err(SettingsError::Profile)?;
    if profile.auth == AuthMode::Official && apply.secret.is_some() {
        return Err(SettingsError::Profile(ProfileError::OfficialHasSecret));
    }
    let before = read_settings_bytes(path)?;
    let existing = match &before { Some(bytes) => serde_json::from_slice(bytes).map_err(|err| SettingsError::Json(err.to_string()))?, None => Value::Object(Map::new()) };
    if json_contains_forbidden(&existing) {
        // 只拒绝我们自己写入这些字段；读取到的旧文件允许保留，但不会被本函数改写。
    }
    let mut root = existing;
    let env = profile_env(profile, apply)?;
    let obj = root.as_object_mut().ok_or_else(|| SettingsError::Json("根节点必须是对象".into()))?;
    if obj.contains_key("machineID") || obj.contains_key("userID") {
        return Err(SettingsError::ForbiddenField);
    }
    let slot = obj.entry("env").or_insert_with(|| Value::Object(Map::new()));
    let env_obj = slot.as_object_mut().ok_or_else(|| SettingsError::Json("env 必须是对象".into()))?;
    for key in managed_keys() {
        env_obj.remove(*key);
    }
    for (key, value) in env {
        env_obj.insert(key, Value::String(value));
    }
    if let Some(lang) = profile.optional.iter().find(|item| item.key == "language" && item.enabled && !item.value.is_empty()) {
        obj.insert("language".into(), Value::String(lang.value.clone()));
    }
    Ok((before, serde_json::to_vec_pretty(&root).map_err(|err| SettingsError::Json(err.to_string()))?))
}

pub fn read_settings_bytes(path: &Path) -> Result<Option<Vec<u8>>, SettingsError> {
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                // macOS's standard /tmp and /var aliases are the only accepted links.
                #[cfg(target_os = "macos")]
                if (ancestor == Path::new("/tmp") || ancestor == Path::new("/var")) && ancestor.canonicalize().is_ok_and(|p| p == Path::new("/private").join(ancestor.strip_prefix("/").unwrap())) { continue; }
                #[cfg(windows)]
                { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 { return Err(SettingsError::Json("配置路径不能包含重解析点".into())); } }
                if meta.file_type().is_symlink() { return Err(SettingsError::Json("配置路径不能包含符号链接".into())); }
                if ancestor == path && (!meta.is_file() || meta.len() > 4 * 1024 * 1024) { return Err(SettingsError::Json("配置必须是 4 MiB 以内的普通文件".into())); }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(SettingsError::Io(error)),
        }
    }
    match fs::read(path) { Ok(bytes) => Ok(Some(bytes)), Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None), Err(error) => Err(SettingsError::Io(error)) }
}

/// Network changes update only routing variables, never a saved-but-unapplied model or key.
pub fn update_proxy_settings(path:&Path, proxy:Option<&str>, backups:&Path)->Result<(),SettingsError> {
    let Some(before)=read_settings_bytes(path)? else { return Ok(()); };
    let mut root:Value=serde_json::from_slice(&before).map_err(|e|SettingsError::Json(e.to_string()))?;
    let object=root.as_object_mut().ok_or_else(||SettingsError::Json("根节点必须是对象".into()))?;
    let env=object.entry("env").or_insert_with(||Value::Object(Map::new())).as_object_mut().ok_or_else(||SettingsError::Json("env 必须是对象".into()))?;
    for key in ["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","NO_PROXY"] { env.remove(key); }
    if let Some(proxy)=proxy { for key in ["HTTP_PROXY","HTTPS_PROXY"] { env.insert(key.into(),Value::String(proxy.into())); } env.insert("NO_PROXY".into(),Value::String("localhost,127.0.0.1,::1".into())); }
    let after=serde_json::to_vec_pretty(&root).map_err(|e|SettingsError::Json(e.to_string()))?;
    if serde_json::from_slice::<Value>(&before).ok()==Some(root) { return Ok(()); }
    backup_file(path,backups)?;
    if read_settings_bytes(path)?.as_deref()!=Some(before.as_slice()){return Err(SettingsError::ConcurrentChange);}
    write_atomic(path,&after)
}

pub fn profile_env(profile: &Profile, apply: &ProfileApply) -> Result<Vec<(String, String)>, SettingsError> {
    let mut env = Vec::new();
    if !profile.base_url.is_empty() {
        env.push(("ANTHROPIC_BASE_URL".into(), profile.base_url.clone()));
    }
    match profile.auth {
        AuthMode::Official => {}
        AuthMode::AuthToken => {
            let secret = apply.secret.clone().filter(|s| !s.is_empty()).ok_or(SettingsError::Profile(ProfileError::MissingSecret))?;
            env.push(("ANTHROPIC_AUTH_TOKEN".into(), secret));
            if crate::url_host(&profile.base_url).is_some_and(|host| host.eq_ignore_ascii_case("openrouter.ai")) {
                env.push(("ANTHROPIC_API_KEY".into(), String::new()));
                env.push(("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(), "1".into()));
            }
        }
        AuthMode::ApiKey => {
            let secret = apply.secret.clone().filter(|s| !s.is_empty()).ok_or(SettingsError::Profile(ProfileError::MissingSecret))?;
            env.push(("ANTHROPIC_API_KEY".into(), secret));
        }
    }
    push_if(&mut env, "ANTHROPIC_MODEL", &profile.model);
    push_if(&mut env, "ANTHROPIC_DEFAULT_SONNET_MODEL", &profile.sonnet_model);
    push_if(&mut env, "ANTHROPIC_DEFAULT_OPUS_MODEL", &profile.opus_model);
    push_if(&mut env, "ANTHROPIC_DEFAULT_HAIKU_MODEL", &profile.haiku_model);
    push_if(&mut env, "ANTHROPIC_SMALL_FAST_MODEL", &profile.small_fast_model);
    push_if(&mut env, "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES", &profile.sonnet_capabilities);
    push_if(&mut env, "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES", &profile.opus_capabilities);
    push_if(&mut env, "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES", &profile.haiku_capabilities);
    push_if(&mut env, "CLAUDE_CODE_SUBAGENT_MODEL", &profile.subagent_model);
    push_if(&mut env, "ANTHROPIC_REASONING_MODEL", &profile.reasoning_model);
    push_if(&mut env, "CLAUDE_CODE_EFFORT_LEVEL", &profile.effort_level);
    push_if(&mut env, "ANTHROPIC_DEFAULT_FABLE_MODEL", &profile.fable_model);
    push_if(&mut env, "CLAUDE_CODE_AUTO_COMPACT_WINDOW", &profile.auto_compact_window);
    push_if(&mut env, "CLAUDE_CODE_MAX_CONTEXT_TOKENS", &profile.max_context_tokens);
    if profile.follow_proxy {
        if let Some(http) = &apply.proxy_http {
            env.push(("HTTP_PROXY".into(), http.clone()));
            env.push(("HTTPS_PROXY".into(), http.clone()));
            env.push(("NO_PROXY".into(), "localhost,127.0.0.1,::1".into()));
        }
    }
    for item in &profile.optional {
        if item.key == "language" || !item.enabled || item.value.is_empty() {
            continue;
        }
        env.push((item.key.clone(), item.value.clone()));
    }
    if env.iter().any(|(k, _)| k == "machineID" || k == "userID") {
        return Err(SettingsError::ForbiddenField);
    }
    Ok(env)
}

fn push_if(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    if !value.trim().is_empty() {
        env.push((key.into(), value.trim().to_string()));
    }
}

fn managed_keys() -> &'static [&'static str] {
    &[
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "API_TIMEOUT_MS",
        "CLAUDE_CODE_MAX_RETRIES",
        "DISABLE_TELEMETRY",
        "DISABLE_AUTOUPDATER",
        "DISABLE_ERROR_REPORTING",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "ANTHROPIC_REASONING_MODEL",
        "CLAUDE_CODE_EFFORT_LEVEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
    ]
}

pub fn managed_key_names() -> &'static [&'static str] {
    managed_keys()
}

pub fn verify_profile_settings(path: &Path, profile: &Profile, secret: Option<String>, proxy_http: &str) -> Result<(), String> {
    verify_profile_settings_route(path, profile, secret, Some(proxy_http))
}

pub fn verify_profile_settings_for_tun(path: &Path, profile: &Profile, secret: Option<String>) -> Result<(), String> {
    verify_profile_settings_route(path, profile, secret, None)
}

fn verify_profile_settings_route(path: &Path, profile: &Profile, secret: Option<String>, proxy_http: Option<&str>) -> Result<(), String> {
    if !path.is_file() { return Err("Claude 用户设置不存在，请先启用一份模型配置".into()); }
    if proxy_http.is_some() && !profile.follow_proxy { return Err("当前模型没有跟随本机代理，请在高级设置中开启后重新应用".into()); }
    let root = read_json(path).map_err(|e| e.to_string())?;
    let actual = root.get("env").and_then(Value::as_object).ok_or("Claude 用户设置的 env 无效")?;
    let expected: Map<String, Value> = profile_env(profile, &ProfileApply { secret, proxy_http: proxy_http.map(str::to_owned), confirmed: false })
        .map_err(|e| e.to_string())?.into_iter().map(|(key, value)| (key, Value::String(value))).collect();
    let mismatches: Vec<&str> = managed_keys().iter().copied().filter(|key| actual.get(*key) != expected.get(*key)).collect();
    if !mismatches.is_empty() { return Err(format!("Claude 设置与当前模型配置不一致：{}。请重新启用配置后再检查", mismatches.join("、"))); }
    if root.get("model").and_then(Value::as_str).is_some_and(|model| !model.is_empty() && !profile.model.is_empty() && model != profile.model) {
        return Err("用户设置中的顶层 model 与目标模型不一致，请先核对".into());
    }
    Ok(())
}

pub fn read_env(path: &Path) -> Result<Map<String, Value>, SettingsError> {
    let value = read_json(path)?;
    Ok(value.get("env").and_then(|v| v.as_object()).cloned().unwrap_or_default())
}

pub fn conflict_keys(user_env: &Map<String, Value>, other_env: &Map<String, Value>, profile: &Profile) -> Vec<String> {
    let watched = ["ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL", "HTTP_PROXY", "HTTPS_PROXY"];
    let mut out = Vec::new();
    for key in watched {
        let left = user_env.get(key).and_then(|v| v.as_str());
        let right = other_env.get(key).and_then(|v| v.as_str());
        if let (Some(a), Some(b)) = (left, right) {
            if a != b {
                out.push(key.to_string());
            }
        } else if right.is_some() && key == "ANTHROPIC_BASE_URL" && !profile.base_url.is_empty() && right != Some(profile.base_url.as_str()) {
            out.push(key.to_string());
        }
    }
    for key in ["CLAUDE_CODE_SUBAGENT_MODEL", "ANTHROPIC_REASONING_MODEL", "CLAUDE_CODE_EFFORT_LEVEL", "ANTHROPIC_DEFAULT_FABLE_MODEL", "CLAUDE_CODE_AUTO_COMPACT_WINDOW", "CLAUDE_CODE_MAX_CONTEXT_TOKENS"] {
        let Some(local) = other_env.get(key).and_then(|value| value.as_str()) else { continue };
        if local.is_empty() {
            continue;
        }
        let field = match key {
            "CLAUDE_CODE_SUBAGENT_MODEL" => profile.subagent_model.as_str(),
            "ANTHROPIC_REASONING_MODEL" => profile.reasoning_model.as_str(),
            "CLAUDE_CODE_EFFORT_LEVEL" => profile.effort_level.as_str(),
            "ANTHROPIC_DEFAULT_FABLE_MODEL" => profile.fable_model.as_str(),
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW" => profile.auto_compact_window.as_str(),
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS" => profile.max_context_tokens.as_str(),
            _ => "",
        };
        if field.is_empty() || field != local {
            out.push(key.to_string());
        }
    }
    out
}

fn read_json(path: &Path) -> Result<Value, SettingsError> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = fs::read_to_string(path).map_err(SettingsError::Io)?;
    serde_json::from_str(&text).map_err(|err| SettingsError::Json(err.to_string()))
}

fn json_contains_forbidden(value: &Value) -> bool {
    value.get("machineID").is_some() || value.get("userID").is_some()
}

pub fn backup_file(path: &Path, backup_dir: &Path) -> Result<(), SettingsError> {
    if !path.exists() {
        return Ok(());
    }
    crate::create_settings_snapshot(path, backup_dir, "auto").map_err(SettingsError::Json)?;
    trim_backups(backup_dir, 20)?;
    Ok(())
}

fn trim_backups(dir: &Path, keep: usize) -> Result<(), SettingsError> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir).map_err(SettingsError::Io)?.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.ends_with("-settings.json") && !name.contains("-manual-")
    })).collect();
    files.sort();
    let extra = files.len().saturating_sub(keep);
    for path in files.into_iter().take(extra) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SettingsError> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(SettingsError::Io)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(SettingsError::Io)?;
    use std::io::Write;
    tmp.write_all(bytes).map_err(SettingsError::Io)?;
    tmp.as_file().sync_all().map_err(SettingsError::Io)?;
    tmp.persist(path).map_err(|err| SettingsError::Io(err.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Profile, ProfileError};

    #[test]
    fn tun_profile_verification_preserves_model_checks_without_forcing_http_proxy() {
        let root = std::env::temp_dir().join(format!("ck-tun-profile-verification-{}", std::process::id()));
        let path = root.join("settings.json");
        let _ = fs::remove_dir_all(&root);
        let profile = Profile::official();
        apply_profile(&path, &profile, &ProfileApply { secret: None, proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        assert!(verify_profile_settings_for_tun(&path, &profile, None).is_ok());
        assert!(verify_profile_settings(&path, &profile, None, "http://127.0.0.1:3128").is_err());
        apply_profile(&path, &profile, &ProfileApply { secret: None, proxy_http: Some("http://127.0.0.1:3128".into()), confirmed: true }, &root.join("backups")).unwrap();
        assert!(verify_profile_settings_for_tun(&path, &profile, None).is_err());
        assert!(verify_profile_settings(&path, &profile, None, "http://127.0.0.1:3128").is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn openrouter_uses_bearer_auth_and_explicitly_clears_direct_anthropic_auth() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let profile = crate::profile_from_preset(crate::preset_by_id("openrouter").unwrap());
        apply_profile(&path, &profile, &ProfileApply { secret: Some("openrouter-test-token".into()), proxy_http: None, confirmed: true }, &root.path().join("backups")).unwrap();
        let env = read_env(&path).unwrap();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://openrouter.ai/api");
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "openrouter-test-token");
        assert_eq!(env["ANTHROPIC_API_KEY"], "");
        assert_eq!(env["CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"], "1");
        assert!(verify_profile_settings_for_tun(&path, &profile, Some("openrouter-test-token".into())).is_ok());
        let mut wrong = read_json(&path).unwrap();
        wrong["env"]["ANTHROPIC_API_KEY"] = Value::String("stale-key".into());
        fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(verify_profile_settings_for_tun(&path, &profile, Some("openrouter-test-token".into())).is_err());
        apply_profile(&path, &Profile::official(), &ProfileApply { secret: None, proxy_http: None, confirmed: true }, &root.path().join("backups")).unwrap();
        let env = read_env(&path).unwrap();
        for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"] { assert!(!env.contains_key(key)); }
    }

    #[test]
    fn launch_verification_checks_model_credentials_and_proxy_without_disclosing_values() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let mut profile = Profile::official();
        assert!(verify_profile_settings(&path, &profile, None, "http://127.0.0.1:3128").is_err());
        profile.id = "custom".into(); profile.auth = AuthMode::AuthToken;
        profile.base_url = "https://gateway.example".into(); profile.model = "model-a".into();
        apply_profile(&path, &profile, &ProfileApply { secret: Some("real-secret".into()), proxy_http: Some("http://127.0.0.1:3128".into()), confirmed: true }, &root.path().join("backups")).unwrap();
        assert!(verify_profile_settings(&path, &profile, Some("real-secret".into()), "http://127.0.0.1:3128").is_ok());
        let error = verify_profile_settings(&path, &profile, Some("other-secret".into()), "http://127.0.0.1:3128").unwrap_err();
        assert!(error.contains("ANTHROPIC_AUTH_TOKEN"));
        assert!(!error.contains("real-secret") && !error.contains("other-secret"));
        profile.model = "model-b".into();
        assert!(verify_profile_settings(&path, &profile, Some("real-secret".into()), "http://127.0.0.1:3128").is_err());
        profile.follow_proxy = false;
        assert!(verify_profile_settings(&path, &profile, Some("real-secret".into()), "http://127.0.0.1:3128").unwrap_err().contains("跟随"));
    }

    fn dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ck-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn apply_preserves_permissions_and_machine_id_file() {
        let root = dir("settings");
        let settings = root.join("settings.json");
        let state = root.join(".claude.json");
        fs::write(&settings, r#"{"permissions":{"allow":["Bash(ls)"]},"env":{"KEEP":"1","HTTP_PROXY":"http://old"}}"#).unwrap();
        fs::write(&state, r#"{"machineID":"abc","userID":"def"}"#).unwrap();
        let mut profile = Profile::official();
        profile.base_url = "https://gateway.example".into();
        profile.auth = crate::AuthMode::AuthToken;
        profile.follow_proxy = false;
        apply_profile(&settings, &profile, &ProfileApply { secret: Some("token-value".into()), proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        let text = fs::read_to_string(&settings).unwrap();
        assert!(text.contains("Bash(ls)"));
        assert!(text.contains("https://gateway.example"));
        assert!(text.contains("token-value"));
        assert!(!text.contains("http://old"));
        assert!(!text.contains("machineID"));
        assert_eq!(fs::read_to_string(&state).unwrap(), r#"{"machineID":"abc","userID":"def"}"#);
        apply_profile(&settings, &Profile::official(), &ProfileApply { secret: None, proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        let cleared = fs::read_to_string(&settings).unwrap();
        assert!(!cleared.contains("ANTHROPIC_AUTH_TOKEN"));
        let err = apply_profile(&settings, &profile, &ProfileApply { secret: Some("x".into()), proxy_http: None, confirmed: false }, &root.join("backups")).unwrap_err();
        assert!(err.to_string().contains("确认"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn optional_vars_stay_off_until_enabled_and_capabilities_are_not_prefilled() {
        let root = dir("optional");
        let settings = root.join("settings.json");
        let mut profile = Profile::official();
        profile.optional = crate::merge_optional(&[
            crate::OptionalVar { key: "language".into(), value: "zh".into(), enabled: true },
            crate::OptionalVar { key: "DISABLE_TELEMETRY".into(), value: "1".into(), enabled: false },
            crate::OptionalVar { key: "machineID".into(), value: "nope".into(), enabled: true },
        ]);
        assert!(profile.sonnet_capabilities.is_empty());
        apply_profile(&settings, &profile, &ProfileApply { secret: None, proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        let text = fs::read_to_string(&settings).unwrap();
        assert!(text.contains("\"language\": \"zh\"") || text.contains("\"language\":\"zh\""));
        assert!(!text.contains("DISABLE_TELEMETRY"));
        assert!(!text.contains("machineID"));
        assert!(!text.contains("SUPPORTED_CAPABILITIES"));
        profile.sonnet_capabilities = "tool_use".into();
        profile.model = "claude-sonnet".into();
        apply_profile(&settings, &profile, &ProfileApply { secret: None, proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        let text = fs::read_to_string(&settings).unwrap();
        assert!(text.contains("tool_use"));
        assert!(text.contains("ANTHROPIC_MODEL"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_both_secret_fields_in_profile_validation_message() {
        let err = ProfileError::BothSecrets.to_string();
        assert!(err.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn deepseek_apply_then_official_shape_clears_new_keys() {
        let root = dir("deepseek");
        let settings = root.join("settings.json");
        let state = root.join(".claude.json");
        fs::write(&settings, r#"{"permissions":{"allow":["Bash(ls)"]},"env":{"KEEP":"1","ALL_PROXY":"socks5://old","CLAUDE_CODE_SUBAGENT_MODEL":"old","ANTHROPIC_REASONING_MODEL":"old","CLAUDE_CODE_EFFORT_LEVEL":"low","ANTHROPIC_DEFAULT_FABLE_MODEL":"old","CLAUDE_CODE_AUTO_COMPACT_WINDOW":"1"}}"#).unwrap();
        fs::write(&state, r#"{"machineID":"abc","userID":"def"}"#).unwrap();
        let profile = crate::profile_from_preset(crate::preset_by_id("deepseek").unwrap());
        apply_profile(&settings, &profile, &ProfileApply { secret: Some("token-value".into()), proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        let text = fs::read_to_string(&settings).unwrap();
        assert!(text.contains("Bash(ls)"));
        assert!(text.contains("https://api.deepseek.com/anthropic"));
        assert!(text.contains("ANTHROPIC_AUTH_TOKEN"));
        assert!(text.contains("token-value"));
        assert!(text.contains("deepseek-flash[1m]"));
        assert!(text.contains("deepseek-flash"));
        assert!(text.contains("\"max\"") || text.contains("max"));
        assert!(text.contains("786432"));
        assert!(text.contains("KEEP"));
        assert!(!text.contains("ANTHROPIC_API_KEY"));
        assert!(!text.contains("SUPPORTED_CAPABILITIES"));
        assert!(!text.contains("ALL_PROXY"));
        assert_eq!(fs::read_to_string(&state).unwrap(), r#"{"machineID":"abc","userID":"def"}"#);
        apply_profile(&settings, &Profile::official(), &ProfileApply { secret: None, proxy_http: None, confirmed: true }, &root.join("backups")).unwrap();
        let cleared = fs::read_to_string(&settings).unwrap();
        for key in ["CLAUDE_CODE_SUBAGENT_MODEL", "ANTHROPIC_REASONING_MODEL", "CLAUDE_CODE_EFFORT_LEVEL", "ANTHROPIC_DEFAULT_FABLE_MODEL", "CLAUDE_CODE_AUTO_COMPACT_WINDOW", "CLAUDE_CODE_MAX_CONTEXT_TOKENS", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
            assert!(!cleared.contains(key), "{key} still in {cleared}");
        }
        assert!(cleared.contains("Bash(ls)"));
        assert!(cleared.contains("KEEP"));
        let mut following = Profile::official();
        following.follow_proxy = true;
        apply_profile(&settings, &following, &ProfileApply { secret: None, proxy_http: Some("http://127.0.0.1:3128".into()), confirmed: true }, &root.join("backups")).unwrap();
        let proxied = fs::read_to_string(&settings).unwrap();
        assert!(proxied.contains("http://127.0.0.1:3128"));
        assert!(proxied.contains("localhost,127.0.0.1,::1"));
        assert!(!proxied.contains("ALL_PROXY"));
        let err = apply_profile(&settings, &profile, &ProfileApply { secret: Some("x".into()), proxy_http: None, confirmed: false }, &root.join("backups")).unwrap_err();
        assert!(err.to_string().contains("确认"));
        let err = apply_profile(&settings, &Profile::official(), &ProfileApply { secret: Some("x".into()), proxy_http: None, confirmed: true }, &root.join("backups")).unwrap_err();
        assert!(matches!(err, SettingsError::Profile(ProfileError::OfficialHasSecret)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn new_conflict_keys_follow_the_profile_and_old_model_key_does_not() {
        let mut user = Map::new();
        let mut local = Map::new();
        local.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(), Value::String("left-over".into()));
        local.insert("ANTHROPIC_MODEL".into(), Value::String("only-local".into()));
        let official = Profile::official();
        let keys = conflict_keys(&user, &local, &official);
        assert!(keys.contains(&"CLAUDE_CODE_SUBAGENT_MODEL".into()));
        assert!(!keys.contains(&"ANTHROPIC_MODEL".into()));
        user.insert("ANTHROPIC_MODEL".into(), Value::String("only-local".into()));
        let same = conflict_keys(&user, &local, &official);
        assert!(!same.contains(&"ANTHROPIC_MODEL".into()));
    }
}
