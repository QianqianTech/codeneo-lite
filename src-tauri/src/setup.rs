use crate::{
    desktop::{self, Target},
    files,
    remote::Remote,
    secrets,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{fs, path::Path, time::Duration};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Unconfigured,
    Configured,
    Existing,
    NeedsRepair,
}

#[derive(Serialize)]
pub struct Status {
    mode: desktop::Mode,
    state: State,
}

#[derive(Serialize)]
pub struct Configured {
    mode: desktop::Mode,
    tool_count: usize,
}

fn entry(executable: &str, slot: &str) -> Value {
    json!({"command": executable, "args": [desktop::BRIDGE_FLAG, slot]})
}

fn state(target: &Target, root: &Path, executable: &str) -> Result<State, String> {
    let bytes = files::read(&target.path)?;
    let doc = desktop::document(bytes.as_deref())?;
    let Some(current) = desktop::mcp(&doc)?.and_then(|servers| servers.get("frameleap")) else {
        return Ok(State::Unconfigured);
    };
    let slot = target.slot();
    if current == &entry(executable, &slot) {
        Ok(match secrets::load(root, &slot) {
            Ok(Some(_)) => State::Configured,
            _ => State::NeedsRepair,
        })
    } else if current.pointer("/args/0").and_then(Value::as_str) == Some(desktop::BRIDGE_FLAG) {
        Ok(State::NeedsRepair)
    } else {
        Ok(State::Existing)
    }
}

pub fn status() -> Result<Status, String> {
    let target = desktop::target()?;
    Ok(Status {
        mode: target.mode,
        state: state(&target, &secrets::root()?, &desktop::launcher()?)?,
    })
}

pub async fn configure(key: &str) -> Result<Configured, String> {
    let key = secrets::validated(key)?;
    let target = desktop::target()?;
    let before = files::read(&target.path)?;
    desktop::mcp(&desktop::document(before.as_deref())?)?;
    let executable = desktop::launcher()?;
    let remote = Remote::new(key).map_err(|err| err.to_string())?;
    let count = tokio::time::timeout(Duration::from_secs(55), remote.verify())
        .await
        .map_err(|_| "FrameLeap 验证超时，请检查网络后重试")?
        .map_err(|err| err.to_string())?;
    if desktop::target()? != target {
        return Err("Claude Desktop 的当前模式已改变，请重新配置".into());
    }
    install(
        &target,
        &secrets::root()?,
        before.as_deref(),
        &executable,
        key,
    )?;
    Ok(Configured {
        mode: target.mode,
        tool_count: count,
    })
}

fn install(
    target: &Target,
    root: &Path,
    before: Option<&[u8]>,
    executable: &str,
    key: &str,
) -> Result<(), String> {
    let _lock = files::lock(root)?;
    if files::read(&target.path)?.as_deref() != before {
        return Err("Claude Desktop 的配置已被修改，请重试".into());
    }
    let mut doc = desktop::document(before)?;
    desktop::mcp(&doc)?;
    let slot = target.slot();
    let desired = entry(executable, &slot);
    let servers = doc
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("Claude Desktop 的 mcpServers 不是 JSON 对象")?;
    let changed = servers.get("frameleap") != Some(&desired);
    servers.insert("frameleap".into(), desired.clone());
    let mut next = serde_json::to_vec_pretty(&doc).map_err(|_| "无法生成 Claude Desktop 配置")?;
    next.push(b'\n');

    if let Some(bytes) = before.filter(|_| changed) {
        let backups = root.join("backups");
        files::private_dir(&backups)?;
        let backup = backups.join(format!("{slot}.bak"));
        let old_backup = files::read(&backup)?;
        files::replace(&backup, old_backup.as_deref(), &secrets::seal(bytes)?)?;
    }
    let secret_path = secrets::key_path(root, &slot)?;
    files::private_dir(secret_path.parent().ok_or("FrameLeap 凭据路径无效")?)?;
    let old_secret = files::read(&secret_path)?;
    let sealed = secrets::seal(key.as_bytes())?;
    files::replace(&secret_path, old_secret.as_deref(), &sealed)?;

    if changed {
        if let Err(error) = files::replace(&target.path, before, &next) {
            let rollback = if let Some(bytes) = old_secret {
                files::replace(&secret_path, Some(&sealed), &bytes).is_ok()
            } else {
                matches!(files::read(&secret_path), Ok(Some(current)) if current == sealed)
                    && fs::remove_file(&secret_path).is_ok()
            };
            return Err(if rollback {
                error
            } else {
                "未能写入 Claude 配置，也未能恢复本机 Key；请重试配置".into()
            });
        }
    }
    let committed = files::read(&target.path)?;
    if desktop::mcp(&desktop::document(committed.as_deref())?)?
        .and_then(|servers| servers.get("frameleap"))
        != Some(&desired)
    {
        return Err("Claude Desktop 的配置未能通过回读，请重试".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "fl_example0000000000000000000000000000000000000";
    const REPLACEMENT: &str = "fl_replacement000000000000000000000000000000000";
    const EXE: &str = "/Applications/codeneo-lite.app/Contents/MacOS/codeneo-lite";

    fn fixture() -> (tempfile::TempDir, Target, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let target = Target {
            mode: desktop::Mode::Gateway,
            path: dir.path().join(desktop::CONFIG),
        };
        let root = dir.path().join("lite");
        (dir, target, root)
    }

    #[test]
    fn migrates_an_existing_entry_without_copying_the_key_into_desktop_config() {
        let (_dir, target, root) = fixture();
        let previous = json!({
            "deploymentMode": "3p",
            "otherSetting": {"keep": true},
            "mcpServers": {"other": {"command": "other-server"}, "frameleap": {"type": "http"}}
        });
        let original = serde_json::to_vec(&previous).unwrap();
        fs::write(&target.path, &original).unwrap();
        assert_eq!(state(&target, &root, EXE).unwrap(), State::Existing);

        install(&target, &root, Some(&original), EXE, KEY).unwrap();
        let written = files::read(&target.path).unwrap().unwrap();
        let parsed: Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(parsed["deploymentMode"], previous["deploymentMode"]);
        assert_eq!(parsed["otherSetting"], previous["otherSetting"]);
        assert_eq!(
            parsed["mcpServers"]["other"],
            previous["mcpServers"]["other"]
        );
        assert_eq!(
            parsed["mcpServers"]["frameleap"],
            entry(EXE, &target.slot())
        );
        assert!(!String::from_utf8_lossy(&written).contains(KEY));
        assert_eq!(
            secrets::load(&root, &target.slot()).unwrap().as_deref(),
            Some(KEY)
        );
        assert!(root
            .join("backups")
            .join(format!("{}.bak", target.slot()))
            .is_file());
        assert_eq!(state(&target, &root, EXE).unwrap(), State::Configured);

        install(&target, &root, Some(&written), EXE, REPLACEMENT).unwrap();
        assert_eq!(files::read(&target.path).unwrap().unwrap(), written);
        assert_eq!(
            secrets::load(&root, &target.slot()).unwrap().as_deref(),
            Some(REPLACEMENT)
        );
        assert_eq!(
            state(&target, &root, "/moved/codeneo-lite").unwrap(),
            State::NeedsRepair
        );
    }

    #[test]
    fn stops_before_saving_a_credential_if_config_is_invalid_or_changed() {
        let (_dir, target, root) = fixture();
        let invalid = br#"{"mcpServers":[]}"#;
        fs::write(&target.path, invalid).unwrap();
        assert!(install(&target, &root, Some(invalid), EXE, KEY).is_err());
        assert!(install(&target, &root, Some(b"{}"), EXE, KEY).is_err());
        assert_eq!(
            files::read(&target.path).unwrap().as_deref(),
            Some(invalid.as_slice())
        );
        assert!(secrets::load(&root, &target.slot()).unwrap().is_none());
    }
}
