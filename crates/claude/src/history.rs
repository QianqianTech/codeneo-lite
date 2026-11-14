use crate::{backup_claude_cleanup, write_atomic};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize)]
pub struct ConfigBackup {
    pub id: String,
    pub bytes: Option<u64>,
    pub kind: String,
    pub created_at_ms: u64,
    pub contents: Vec<String>,
    pub model: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub safety_backup: String,
    pub restored: Vec<String>,
}

fn regular(path: &Path, directory: bool) -> bool {
    fs::symlink_metadata(path).map(|m| !m.file_type().is_symlink() && if directory { m.is_dir() } else { m.is_file() }).unwrap_or(false)
}

fn selected(root: &Path, id: &str) -> Result<PathBuf, String> {
    let mut parts = Path::new(id).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() || id.contains(['/', '\\']) {
        return Err("无效的备份编号".into());
    }
    if fs::symlink_metadata(root).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        return Err("备份目录不能是符号链接".into());
    }
    let path = root.join(id);
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if canonical.parent() != Some(canonical_root.as_path()) || fs::symlink_metadata(&path).map_err(|e| e.to_string())?.file_type().is_symlink() {
        return Err("只能恢复本机备份目录中的普通备份".into());
    }
    Ok(path)
}

fn modified(path: &Path) -> u64 {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn read_settings_snapshot(path: &Path) -> Result<Vec<u8>, String> {
    if !regular(path, false) { return Err("配置快照不是普通文件".into()); }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_slice(&bytes).map_err(|_| "备份中的配置不是有效 JSON".to_string())?;
    if !json.is_object() || json.get("env").is_some_and(|env| !env.is_object()) {
        return Err("备份中的配置格式不正确".into());
    }
    Ok(bytes)
}

fn model_name(path: &Path) -> String {
    let Ok(bytes) = read_settings_snapshot(path) else { return "无法读取配置摘要".into(); };
    let Ok(json) = serde_json::from_slice::<Value>(&bytes) else { return String::new(); };
    let env = &json["env"];
    if let Some(model) = env["ANTHROPIC_MODEL"].as_str().filter(|s| !s.is_empty()) {
        let mut summary = model.to_string();
        for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
            if let Some(secret) = env[key].as_str().filter(|value| !value.is_empty()) { summary = summary.replace(secret, "***"); }
        }
        return summary.chars().take(100).collect();
    }
    if env["ANTHROPIC_BASE_URL"].as_str().is_some_and(|s| !s.is_empty()) { "第三方模型".into() } else { "官方登录".into() }
}

fn backup_bytes(path:&Path,budget:&mut usize)->Option<u64>{
    if *budget==0{return None;}*budget-=1;
    let meta=fs::symlink_metadata(path).ok()?;if meta.file_type().is_symlink(){return None;}
    if meta.is_file(){return Some(meta.len());}if !meta.is_dir(){return None;}
    let mut size=0u64;for entry in fs::read_dir(path).ok()?{size=size.checked_add(backup_bytes(&entry.ok()?.path(),budget)?)?;}Some(size)
}

pub fn list_config_backups(home: &Path, settings_root: &Path) -> Result<Vec<ConfigBackup>, String> {
    let mut entries = Vec::new();
    for (root, kind) in [(home.join("ClaudeBackups"), "full"), (settings_root.to_path_buf(), "settings")] {
        if !root.exists() { continue; }
        if !regular(&root, true) { return Err("备份目录不是普通目录".into()); }
        for item in fs::read_dir(&root).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            let id = item.file_name().to_string_lossy().to_string();
            let path = item.path();
            let (contents, source, model) = if kind == "full" {
                if !id.starts_with("claude-cleanup-") || !regular(&path, true) { continue; }
                let mut contents = Vec::new();
                if regular(&path.join(".claude"), true) { contents.push(".claude 目录".into()); }
                if regular(&path.join(".claude.json"), false) { contents.push(".claude.json".into()); }
                if contents.is_empty() { continue; }
                (contents, "完整备份".to_string(), model_name(&path.join(".claude/settings.json")))
            } else {
                if !id.ends_with("-settings.json") || !regular(&path, false) { continue; }
                let source = if id.contains("-manual-") { "手动快照" } else if id.contains("-user-") { "切换前自动备份" } else if id.contains("-project-") { "项目配置快照" } else { "历史快照（来源未标记）" };
                (vec!["settings.json".into()], source.to_string(), model_name(&path))
            };
            entries.push(ConfigBackup { bytes:backup_bytes(&path, &mut 20000), id, kind: kind.into(), created_at_ms: modified(&path), contents, model, source });
        }
    }
    entries.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms).then_with(|| b.id.cmp(&a.id)));
    Ok(entries)
}

pub fn create_settings_snapshot(settings: &Path, root: &Path, reason: &str) -> Result<PathBuf, String> {
    let bytes = read_settings_snapshot(settings)?;
    save_snapshot_bytes(settings, root, reason, &bytes)
}

fn save_snapshot_bytes(settings: &Path, root: &Path, reason: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    if !regular(root, true) { return Err("备份目录不是普通目录".into()); }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let reason = if reason == "manual" { "manual" } else if settings.file_name().is_some_and(|name| name == "settings.local.json") { "project" } else { "user" };
    let mut temp = tempfile::Builder::new().prefix(&format!("{stamp}-{reason}-")).suffix("-settings.json").tempfile_in(root).map_err(|e| e.to_string())?;
    temp.write_all(bytes).and_then(|_| temp.as_file().sync_all()).map_err(|e| e.to_string())?;
    let (_, path) = temp.keep().map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn config_backup_settings(home: &Path, settings_root: &Path, kind: &str, id: &str) -> Result<Option<PathBuf>, String> {
    match kind {
        "settings" if id.ends_with("-settings.json") => {
            let path = selected(settings_root, id)?;
            read_settings_snapshot(&path)?;
            Ok(Some(path))
        }
        "full" if id.starts_with("claude-cleanup-") => {
            let path = selected(&home.join("ClaudeBackups"), id)?;
            if !regular(&path, true) { return Err("完整备份不是普通目录".into()); }
            validate_tree(&path)?;
            let settings = path.join(".claude/settings.json");
            if settings.exists() { read_settings_snapshot(&settings)?; Ok(Some(settings)) } else { Ok(None) }
        }
        _ => Err("未知的备份类型".into()),
    }
}

fn validate_tree(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !(meta.is_dir() || meta.is_file()) { return Err("备份包含符号链接或特殊文件，未恢复".into()); }
    if meta.is_dir() {
        for item in fs::read_dir(path).map_err(|e| e.to_string())? { validate_tree(&item.map_err(|e| e.to_string())?.path())?; }
    }
    Ok(())
}

fn stage_copy(from: &Path, to: &Path) -> Result<(), String> {
    validate_tree(from)?;
    if from.is_dir() {
        fs::create_dir(to).map_err(|e| e.to_string())?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(to, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        }
        for item in fs::read_dir(from).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            stage_copy(&item.path(), &to.join(item.file_name()))?;
        }
    } else {
        fs::copy(from, to).map_err(|e| e.to_string())?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(to, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn restore_config_backup(home: &Path, settings_root: &Path, kind: &str, id: &str, stamp: &str, confirmed: bool) -> Result<RestoreResult, String> {
    if !confirmed { return Err("恢复配置需要明确确认".into()); }
    let settings_source = config_backup_settings(home, settings_root, kind, id)?;
    let settings = home.join(".claude/settings.json");
    if kind == "settings" {
        let bytes = read_settings_snapshot(&settings_source.ok_or("快照缺少配置文件")?)?;
        if settings.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) || settings.parent().is_some_and(|p| p.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false)) {
            return Err("当前配置使用了符号链接，未恢复".into());
        }
        let safety = if settings.exists() {
            if !regular(&settings, false) { return Err("当前配置不是普通文件，未恢复".into()); }
            let original = fs::read(&settings).map_err(|e| e.to_string())?;
            save_snapshot_bytes(&settings, settings_root, "manual", &original)?.display().to_string()
        } else if home.join(".claude").exists() || home.join(".claude.json").exists() {
            backup_claude_cleanup(home, stamp).map_err(|e| e.to_string())?.display().to_string()
        } else { String::new() };
        write_atomic(&settings, &bytes).map_err(|e| e.to_string())?;
        return Ok(RestoreResult { safety_backup: safety, restored: vec!["用户级 settings.json".into()] });
    }
    let source = selected(&home.join("ClaudeBackups"), id)?;
    let stage = tempfile::tempdir_in(home).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for name in [".claude", ".claude.json"] {
        let from = source.join(name);
        if !from.exists() { continue; }
        let target = home.join(name);
        if target.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) { return Err("当前 Claude 配置使用了符号链接，未恢复".into()); }
        if name == ".claude.json" {
            let bytes = fs::read(&from).map_err(|e| e.to_string())?;
            if !serde_json::from_slice::<Value>(&bytes).map(|v| v.is_object()).unwrap_or(false) { return Err("备份中的 .claude.json 无效，未恢复".into()); }
        }
        stage_copy(&from, &stage.path().join(name))?;
        names.push(name.to_string());
    }
    if names.is_empty() { return Err("备份中没有可恢复的配置".into()); }
    let safety = if home.join(".claude").exists() || home.join(".claude.json").exists() { Some(backup_claude_cleanup(home, stamp).map_err(|e| e.to_string())?) } else { None };
    let rollback = stage.path().join("rollback");
    fs::create_dir(&rollback).map_err(|e| e.to_string())?;
    let mut moved = Vec::new();
    let result = (|| -> Result<(), String> {
        for name in &names {
            let target = home.join(name);
            if target.exists() { fs::rename(&target, rollback.join(name)).map_err(|e| e.to_string())?; }
            moved.push(name.clone());
            fs::rename(stage.path().join(name), &target).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failed = false;
        for name in moved.iter().rev() {
            let target = home.join(name);
            if target.is_dir() { if fs::remove_dir_all(&target).is_err() { failed = true; continue; } }
            else if target.exists() && fs::remove_file(&target).is_err() { failed = true; continue; }
            if rollback.join(name).exists() && fs::rename(rollback.join(name), &target).is_err() { failed = true; }
        }
        return Err(format!("恢复未完成：{error}。{}；恢复前的备份：{}", if failed { "部分文件未能回退" } else { "已回退当前文件" }, safety.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "恢复前没有配置".into())));
    }
    Ok(RestoreResult { safety_backup: safety.map(|p| p.display().to_string()).unwrap_or_default(), restored: names })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_list_without_secrets_and_restore_with_safety_copy() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let history = root.path().join("history");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let settings = home.join(".claude/settings.json");
        fs::write(&settings, r#"{"env":{"ANTHROPIC_MODEL":"model-a","ANTHROPIC_AUTH_TOKEN":"private-token"}}"#).unwrap();
        let saved = create_settings_snapshot(&settings, &history, "manual").unwrap();
        let id = saved.file_name().unwrap().to_str().unwrap();
        fs::write(&settings, r#"{"env":{"ANTHROPIC_MODEL":"model-b"}}"#).unwrap();
        assert!(restore_config_backup(&home, &history, "settings", id, "20261027-120000", false).is_err());
        assert!(fs::read_to_string(&settings).unwrap().contains("model-b"));
        let result = restore_config_backup(&home, &history, "settings", id, "20261027-120000", true).unwrap();
        assert!(fs::read_to_string(&settings).unwrap().contains("private-token"));
        assert!(fs::read_to_string(result.safety_backup).unwrap().contains("model-b"));
        let entries = list_config_backups(&home, &history).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(!serde_json::to_string(&entries).unwrap().contains("private-token"));
        assert!(restore_config_backup(&home, &history, "settings", "../outside-settings.json", "20261027-120000", true).is_err());
    }

    #[test]
    fn full_restore_preserves_current_files_in_safety_backup_and_ignores_network() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let history = root.path().join("history");
        fs::create_dir_all(home.join(".claude/projects")).unwrap();
        fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        fs::write(home.join(".claude.json"), r#"{"machineID":"before"}"#).unwrap();
        let saved = backup_claude_cleanup(&home, "20261027-100000").unwrap();
        fs::write(home.join(".claude.json"), r#"{"machineID":"after"}"#).unwrap();
        fs::write(home.join(".claude/projects/new.txt"), "new").unwrap();
        fs::write(home.join("route.txt"), "untouched").unwrap();
        let result = restore_config_backup(&home, &history, "full", saved.file_name().unwrap().to_str().unwrap(), "20261027-100001", true).unwrap();
        assert!(fs::read_to_string(home.join(".claude.json")).unwrap().contains("before"));
        assert!(fs::read_to_string(Path::new(&result.safety_backup).join(".claude.json")).unwrap().contains("after"));
        assert!(Path::new(&result.safety_backup).join(".claude/projects/new.txt").is_file());
        assert!(!home.join(".claude/projects/new.txt").exists());
        assert_eq!(fs::read_to_string(home.join("route.txt")).unwrap(), "untouched");
    }

    #[test]
    fn invalid_backups_do_not_touch_current_configuration() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let history = root.path().join("history");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&history).unwrap();
        fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        fs::write(history.join("bad-settings.json"), "not json").unwrap();
        assert!(restore_config_backup(&home, &history, "settings", "bad-settings.json", "20261027-100001", true).is_err());
        assert_eq!(fs::read_to_string(home.join(".claude/settings.json")).unwrap(), "{}");
    }

    #[test]
    fn a_valid_snapshot_can_repair_corrupt_or_deleted_current_settings() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let history = root.path().join("history");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let settings = home.join(".claude/settings.json");
        fs::write(&settings, "{}").unwrap();
        let saved = create_settings_snapshot(&settings, &history, "manual").unwrap();
        let id = saved.file_name().unwrap().to_str().unwrap();
        fs::write(&settings, "damaged-json").unwrap();
        let result = restore_config_backup(&home, &history, "settings", id, "20261027-120000", true).unwrap();
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{}");
        assert_eq!(fs::read_to_string(result.safety_backup).unwrap(), "damaged-json");
        fs::remove_dir_all(home.join(".claude")).unwrap();
        let result = restore_config_backup(&home, &history, "settings", id, "20261027-120001", true).unwrap();
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{}");
        assert!(result.safety_backup.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_backups_and_targets_are_rejected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let history = root.path().join("history");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        let saved = create_settings_snapshot(&home.join(".claude/settings.json"), &history, "manual").unwrap();
        symlink(&saved, history.join("link-settings.json")).unwrap();
        assert!(config_backup_settings(&home, &history, "settings", "link-settings.json").is_err());
        let full = backup_claude_cleanup(&home, "20261027-100000").unwrap();
        symlink(home.join(".claude"), full.join(".claude/link")).unwrap();
        assert!(config_backup_settings(&home, &history, "full", full.file_name().unwrap().to_str().unwrap()).is_err());
    }
}
