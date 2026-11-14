use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Mac,
    Windows,
    Linux,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub os: HostOs,
    pub home: PathBuf,
    pub appdata: PathBuf,
    pub local_appdata: PathBuf,
    pub program_data: PathBuf,
}

pub fn current_os() -> HostOs {
    if cfg!(target_os = "windows") { HostOs::Windows } else if cfg!(target_os = "linux") { HostOs::Linux } else { HostOs::Mac }
}

pub fn desktop_layout(os: HostOs, home: &Path, appdata: Option<&Path>, local_appdata: Option<&Path>, program_data: Option<&Path>) -> Layout {
    match os {
        HostOs::Windows => Layout {
            os,
            home: home.to_path_buf(),
            appdata: appdata.map(Path::to_path_buf).unwrap_or_else(|| home.join("AppData").join("Roaming")),
            local_appdata: local_appdata.map(Path::to_path_buf).unwrap_or_else(|| home.join("AppData").join("Local")),
            program_data: program_data.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")),
        },
        HostOs::Mac => Layout {
            os,
            home: home.to_path_buf(),
            appdata: home.to_path_buf(),
            local_appdata: home.to_path_buf(),
            program_data: PathBuf::from("/Library"),
        },
        HostOs::Linux => Layout {
            os,
            home: home.to_path_buf(),
            appdata: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".config")),
            local_appdata: std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/share")),
            program_data: PathBuf::from("/etc"),
        },
    }
}

impl Layout {
    pub fn user_settings(&self) -> PathBuf {
        self.home.join(".claude").join("settings.json")
    }

    pub fn global_state(&self) -> PathBuf {
        self.home.join(".claude.json")
    }

    pub fn managed_settings(&self) -> PathBuf {
        match self.os {
            HostOs::Mac => PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
            HostOs::Windows => self.program_data.join("ClaudeCode").join("managed-settings.json"),
            HostOs::Linux => PathBuf::from("/etc/claude-code/managed-settings.json"),
        }
    }

    pub fn cli_bin(&self) -> PathBuf {
        match self.os {
            HostOs::Mac | HostOs::Linux => self.home.join(".local/bin/claude"),
            HostOs::Windows => self.home.join(".local").join("bin").join("claude.exe"),
        }
    }

    pub fn desktop_dir(&self) -> PathBuf {
        match self.os {
            HostOs::Mac => self.home.join("Library/Application Support/Claude"),
            HostOs::Windows => self.appdata.join("Claude"),
            HostOs::Linux => self.appdata.join("Claude"),
        }
    }

    pub fn project_settings(&self, project: &Path) -> PathBuf {
        project.join(".claude").join("settings.json")
    }

    pub fn project_local_settings(&self, project: &Path) -> PathBuf {
        project.join(".claude").join("settings.local.json")
    }

    pub fn project_mcp(&self, project: &Path) -> PathBuf {
        project.join(".mcp.json")
    }

    pub fn project_claude_md(&self, project: &Path) -> PathBuf {
        project.join("CLAUDE.md")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopTarget {
    pub path: PathBuf,
    pub reason: String,
}

pub fn choose_desktop_dir(appdata_claude: &Path, store_claude: Option<&Path>) -> DesktopTarget {
    match store_claude {
        Some(store) if store.join("settings.json").exists() || store.exists() && appdata_claude.join("settings.json").exists() => {
            let store_time = mtime(&store.join("settings.json"));
            let app_time = mtime(&appdata_claude.join("settings.json"));
            if store_time >= app_time && store.exists() {
                DesktopTarget {
                    path: store.to_path_buf(),
                    reason: "Windows 商店版实际读取包容器里的 LocalCache\\Roaming\\Claude".into(),
                }
            } else {
                DesktopTarget { path: appdata_claude.to_path_buf(), reason: "使用 %APPDATA%\\Claude".into() }
            }
        }
        Some(store) if store.exists() => DesktopTarget {
            path: store.to_path_buf(),
            reason: "检测到商店版虚拟化目录".into(),
        },
        _ => DesktopTarget { path: appdata_claude.to_path_buf(), reason: "使用桌面版用户数据目录".into() },
    }
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn find_store_claude(local_appdata: &Path) -> Option<PathBuf> {
    let packages = local_appdata.join("Packages");
    let entries = std::fs::read_dir(&packages).ok()?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path().join("LocalCache").join("Roaming").join("Claude");
        if candidate.is_dir() {
            found.push(candidate);
        }
    }
    found.sort();
    found.pop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn linux_uses_its_own_policy_and_desktop_directories() {
        let home = Path::new("/home/alice");
        let layout = desktop_layout(HostOs::Linux, home, None, None, None);
        assert_eq!(layout.managed_settings(), PathBuf::from("/etc/claude-code/managed-settings.json"));
        assert_eq!(layout.cli_bin(), home.join(".local/bin/claude"));
        assert!(!layout.desktop_dir().to_string_lossy().contains("Library"));
        assert_eq!(layout.user_settings(), home.join(".claude/settings.json"));
    }

    #[test]
    fn mac_and_windows_paths() {
        let mac = Layout {
            os: HostOs::Mac,
            home: PathBuf::from("/Users/ada"),
            appdata: PathBuf::from("/unused"),
            local_appdata: PathBuf::from("/unused"),
            program_data: PathBuf::from("/unused"),
        };
        assert_eq!(mac.user_settings(), PathBuf::from("/Users/ada/.claude/settings.json"));
        assert_eq!(mac.cli_bin(), PathBuf::from("/Users/ada/.local/bin/claude"));
        assert!(mac.managed_settings().ends_with("managed-settings.json"));

        let win = Layout {
            os: HostOs::Windows,
            home: PathBuf::from(r"C:\Users\ada"),
            appdata: PathBuf::from(r"C:\Users\ada\AppData\Roaming"),
            local_appdata: PathBuf::from(r"C:\Users\ada\AppData\Local"),
            program_data: PathBuf::from(r"C:\ProgramData"),
        };
        assert_eq!(win.cli_bin(), win.home.join(".local").join("bin").join("claude.exe"));
        assert_eq!(win.managed_settings(), win.program_data.join("ClaudeCode").join("managed-settings.json"));
        assert_eq!(win.desktop_dir(), win.appdata.join("Claude"));
        let built = desktop_layout(HostOs::Windows, Path::new(r"C:\Users\ada"), None, None, None);
        assert_eq!(built.os, HostOs::Windows);
        assert!(built.desktop_dir().ends_with("Claude"));
        assert!(built.appdata.ends_with("Roaming"));
        assert!(built.local_appdata.ends_with("Local"));
        assert!(built.cli_bin().ends_with("claude.exe"));
        assert_eq!(built.program_data, PathBuf::from(r"C:\ProgramData"));
        let mac = desktop_layout(HostOs::Mac, Path::new("/Users/ada"), None, None, None);
        assert_eq!(mac.os, HostOs::Mac);
        assert_eq!(mac.program_data, PathBuf::from("/Library"));
        assert_eq!(current_os(), if cfg!(target_os = "windows") { HostOs::Windows } else if cfg!(target_os = "linux") { HostOs::Linux } else { HostOs::Mac });
    }

    #[test]
    fn prefers_store_dir_when_it_has_settings() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("ck-paths-{stamp}"));
        let app = root.join("Roaming").join("Claude");
        let store = root.join("Packages").join("Claude_abc").join("LocalCache").join("Roaming").join("Claude");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("settings.json"), "{}").unwrap();
        let target = choose_desktop_dir(&app, Some(&store));
        assert_eq!(target.path, store);
        assert!(target.reason.contains("商店"));
        let _ = fs::remove_dir_all(root);
    }
}
