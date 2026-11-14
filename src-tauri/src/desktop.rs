use crate::files;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    env,
    path::{Path, PathBuf},
};

pub const CONFIG: &str = "claude_desktop_config.json";
pub const BRIDGE_FLAG: &str = "--frameleap-mcp";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Native,
    Gateway,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub mode: Mode,
    pub path: PathBuf,
}

impl Target {
    pub fn slot(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(self.path.as_os_str().as_encoded_bytes())
        )
    }
}

pub fn document(bytes: Option<&[u8]>) -> Result<Map<String, Value>, String> {
    if let Some(bytes) = bytes {
        match serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)) {
            Ok(Value::Object(doc)) => Ok(doc),
            _ => Err("Claude Desktop 的配置不是有效的 JSON 对象，未修改".into()),
        }
    } else {
        Ok(Map::new())
    }
}

fn profile(candidates: &[PathBuf]) -> Option<&Path> {
    candidates
        .iter()
        .find(|dir| dir.join(CONFIG).symlink_metadata().is_ok())
        .or_else(|| candidates.iter().find(|dir| dir.is_dir()))
        .map(PathBuf::as_path)
}

fn explicit_mode(dir: Option<&Path>) -> Result<Option<Mode>, String> {
    let Some(dir) = dir else { return Ok(None) };
    let bytes = files::read(&dir.join(CONFIG))?;
    let doc = document(bytes.as_deref())?;
    Ok(match doc.get("deploymentMode").and_then(Value::as_str) {
        Some("1p") => Some(Mode::Native),
        Some("3p") => Some(Mode::Gateway),
        _ => None,
    })
}

fn gateway_applied(dir: &Path) -> Result<bool, String> {
    let library = dir.join("configLibrary");
    let bytes = files::read(&library.join("_meta.json"))?;
    let meta = document(bytes.as_deref())?;
    let Some(id) = meta.get("appliedId").and_then(Value::as_str) else {
        return Ok(false);
    };
    Ok(id.len() == 36
        && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        && library.join(format!("{id}.json")).is_file())
}

fn choose(native: &[PathBuf], gateway: &[PathBuf]) -> Result<Target, String> {
    let native = profile(native);
    let gateway = profile(gateway);
    let mode = if let Some(mode) = explicit_mode(gateway)? {
        mode
    } else if let Some(mode) = explicit_mode(native)? {
        mode
    } else if let Some(dir) = gateway {
        if native.is_none() || gateway_applied(dir)? {
            Mode::Gateway
        } else {
            Mode::Native
        }
    } else {
        Mode::Native
    };
    let dir = if mode == Mode::Gateway {
        gateway
    } else {
        native
    };
    let dir = dir.ok_or("未找到当前 Claude Desktop 的资料目录，请先启动一次 Claude Desktop")?;
    Ok(Target {
        mode,
        path: dir.join(CONFIG),
    })
}

pub(crate) fn custom_dir() -> Result<Option<PathBuf>, String> {
    if let Some(path) = env::var_os("CLAUDE_USER_DATA_DIR") {
        let dir = PathBuf::from(path);
        if !dir.is_absolute() || !dir.is_dir() {
            return Err("CLAUDE_USER_DATA_DIR 必须是已存在的绝对目录".into());
        }
        return Ok(Some(dir));
    }
    Ok(None)
}

fn candidates() -> Result<(Vec<PathBuf>, Vec<PathBuf>, PathBuf), String> {
    let root = dirs::config_dir().ok_or("找不到当前用户的配置目录")?;
    #[cfg(windows)]
    {
        let local = dirs::data_local_dir().ok_or("找不到当前用户的本机资料目录")?;
        let mut native: Vec<_> = [
            "Claude_pzs8sxrjxfjjc",
            "Claude-Desktop_pzs8sxrjxfjjc",
            "AnthropicPBC.Claude_fnn82j28hfe8t",
        ]
        .iter()
        .map(|family| {
            local
                .join("Packages")
                .join(family)
                .join("LocalCache/Roaming/Claude")
        })
        .collect();
        native.extend([
            root.join("Claude"),
            local.join("Claude"),
            local.join("Claude-Data"),
        ]);
        Ok((
            native,
            vec![local.join("Claude-3p"), root.join("Claude-3p")],
            root.join("Claude"),
        ))
    }
    #[cfg(target_os = "linux")]
    {
        Ok((
            vec![root.join("Claude"), root.join("claude-desktop")],
            vec![root.join("Claude-3p")],
            root.join("Claude"),
        ))
    }
    #[cfg(target_os = "macos")]
    {
        Ok((
            vec![root.join("Claude")],
            vec![root.join("Claude-3p")],
            root.join("Claude"),
        ))
    }
}

pub struct Locations {
    pub native: PathBuf,
    pub gateway: PathBuf,
}

pub fn locations() -> Result<Locations, String> {
    if let Some(dir) = custom_dir()? {
        return Ok(Locations {
            native: dir.clone(),
            gateway: dir,
        });
    }
    let (native, gateway, default_native) = candidates()?;
    let found_native = profile(&native);
    let found_gateway = profile(&gateway);
    if found_native.is_none() && found_gateway.is_none() {
        return Err("请先启动一次 Claude Desktop，再管理它的模型配置".into());
    }
    Ok(Locations {
        native: found_native
            .map(Path::to_path_buf)
            .unwrap_or(default_native),
        gateway: found_gateway
            .map(Path::to_path_buf)
            .unwrap_or_else(|| gateway[0].clone()),
    })
}

pub fn target() -> Result<Target, String> {
    if let Some(dir) = custom_dir()? {
        return Ok(Target {
            mode: Mode::Custom,
            path: dir.join(CONFIG),
        });
    }
    let (native, gateway, _) = candidates()?;
    choose(&native, &gateway)
}

pub fn launcher() -> Result<String, String> {
    let executable = env::current_exe().map_err(|_| "无法定位 CodeNeo Lite 启动程序")?;
    // An AppImage mounts its executable at a temporary path on every launch.
    #[cfg(target_os = "linux")]
    let executable = if executable
        .components()
        .any(|part| part.as_os_str().to_string_lossy().starts_with(".mount_"))
    {
        env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_file())
            .ok_or("无法定位 Lite 的 AppImage，请把它放在固定目录后再配置")?
    } else {
        executable
    };
    executable
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Lite 启动程序路径不是有效的 Unicode".into())
}

pub fn mcp(doc: &Map<String, Value>) -> Result<Option<&Map<String, Value>>, String> {
    doc.get("mcpServers")
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| "Claude Desktop 的 mcpServers 不是 JSON 对象，未修改".into())
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn selects_the_active_profile_instead_of_the_first_config_file() {
        let fixture = tempfile::tempdir().unwrap();
        let native = fixture.path().join("Claude");
        let gateway = fixture.path().join("Claude-3p");
        fs::create_dir(&native).unwrap();
        fs::create_dir(&gateway).unwrap();
        fs::write(native.join(CONFIG), br#"{"mcpServers":{"existing":{}}}"#).unwrap();
        fs::write(gateway.join(CONFIG), br#"{"deploymentMode":"3p"}"#).unwrap();

        let selected = choose(&[native.clone()], &[gateway.clone()]).unwrap();
        assert_eq!(selected.mode, Mode::Gateway);
        assert_eq!(selected.path, gateway.join(CONFIG));

        fs::write(gateway.join(CONFIG), br#"{"deploymentMode":"1p"}"#).unwrap();
        let selected = choose(&[native.clone()], &[gateway]).unwrap();
        assert_eq!(selected.mode, Mode::Native);
        assert_eq!(selected.path, native.join(CONFIG));
    }
}
