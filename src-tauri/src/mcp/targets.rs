use crate::{agents, codex, desktop, files, transaction::Client, workbuddy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    Project,
    Local,
}

pub fn scopes(client: Client) -> &'static [Scope] {
    use Scope::*;
    match client {
        Client::Code => &[User, Project, Local],
        Client::Codex | Client::Qwen | Client::Codebuddy | Client::Kimi => &[User, Project],
        _ => &[User],
    }
}

#[derive(Deserialize)]
pub struct Selection {
    pub scope: Scope,
    directory: Option<PathBuf>,
}

#[derive(Deserialize)]
pub struct Destination {
    #[serde(flatten)]
    selection: Selection,
    location: String,
}

pub struct Location {
    pub path: PathBuf,
    section: Vec<String>,
    pub scope: Scope,
    pub label: &'static str,
    pub directory: Option<PathBuf>,
}

impl Location {
    pub fn sections(&self) -> Vec<&str> {
        self.section.iter().map(String::as_str).collect()
    }

    pub fn id(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.path.as_os_str().as_encoded_bytes());
        for section in &self.section {
            hash.update([0]);
            hash.update(section.as_bytes());
        }
        format!("{:x}", hash.finalize())
    }

    pub fn backup_slot(&self) -> Option<String> {
        (self.scope != Scope::User).then(|| self.id())
    }
}

impl Destination {
    pub fn resolve(&self, client: Client) -> Result<Location, String> {
        let location = self.selection.resolve(client)?;
        if location.id() != self.location {
            return Err("MCP 配置位置已变化，请刷新后重试".into());
        }
        Ok(location)
    }
}

pub fn directory(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("请选择一个已存在的绝对目录".into());
    }
    let path = dunce::canonicalize(path).map_err(|_| "找不到该目录，请重新选择")?;
    if !path.is_dir() || path.to_str().is_none() {
        return Err("请选择路径有效的文件夹".into());
    }
    Ok(path)
}

impl Selection {
    pub fn resolve(&self, client: Client) -> Result<Location, String> {
        if !scopes(client).contains(&self.scope) {
            return Err("此客户端不支持所选 MCP 配置范围".into());
        }
        if self.scope == Scope::User {
            return user(client);
        }
        let path = self.directory.as_deref().ok_or("请先选择项目根目录")?;
        project(client, self.scope, directory(path)?)
    }
}

fn project(client: Client, scope: Scope, root: PathBuf) -> Result<Location, String> {
    let (path, section) = match (client, scope) {
        (Client::Code, Scope::Local) => (
            claude_code()?,
            vec![
                "projects".into(),
                root.to_str()
                    .ok_or("项目路径编码无效")?
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                "mcpServers".into(),
            ],
        ),
        (Client::Code, Scope::Project) => (root.join(".mcp.json"), vec!["mcpServers".into()]),
        (Client::Codex, Scope::Project) => (
            project_file(&root, ".codex", "config.toml")?,
            vec!["mcp_servers".into()],
        ),
        (Client::Qwen, Scope::Project) => (
            project_file(&root, ".qwen", "settings.json")?,
            vec!["mcpServers".into()],
        ),
        (Client::Kimi, Scope::Project) => (
            project_file(&root, ".kimi-code", "mcp.json")?,
            vec!["mcpServers".into()],
        ),
        (Client::Codebuddy, Scope::Project) => (
            agents::first_present([root.join(".mcp.json"), root.join("mcp.json")]),
            vec!["mcpServers".into()],
        ),
        _ => return Err("此客户端不支持所选 MCP 配置范围".into()),
    };
    Ok(Location {
        path,
        section,
        scope,
        directory: Some(root),
        label: if scope == Scope::Local {
            "本机项目"
        } else {
            "项目共享"
        },
    })
}

fn project_file(root: &Path, subdir: &str, filename: &str) -> Result<PathBuf, String> {
    let parent = root.join(subdir);
    match fs::symlink_metadata(&parent) {
        Ok(meta) if meta.is_dir() && !files::linked(&meta) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        _ => return Err("项目的配置目录不是普通文件夹，请在客户端中核对".into()),
    }
    Ok(parent.join(filename))
}

fn claude_code() -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os("CLAUDE_CONFIG_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return Err("CLAUDE_CONFIG_DIR 必须是绝对路径".into());
        }
        Ok(dir.join(".claude.json"))
    } else {
        Ok(agents::home()?.join(".claude.json"))
    }
}

fn user(client: Client) -> Result<Location, String> {
    use Client::*;
    let (path, section, label) = match client {
        Desktop => {
            let target = desktop::target()?;
            let label = match target.mode {
                desktop::Mode::Native => "官方模式",
                desktop::Mode::Gateway => "第三方模型模式",
                desktop::Mode::Custom => "自定义资料目录",
            };
            (target.path, "mcpServers", label)
        }
        Code => (claude_code()?, "mcpServers", "用户级"),
        Codex => (codex::path()?, "mcp_servers", "用户级"),
        Workbuddy => (
            workbuddy::configured_file("mcp.json")?,
            "mcpServers",
            "用户级",
        ),
        Opencode => (agents::settings(client)?, "mcp", "用户级"),
        Qwen => (agents::settings(client)?, "mcpServers", "用户级"),
        Codebuddy => {
            let home = agents::home()?;
            let dir = agents::codebuddy_home()?;
            (
                agents::first_present([
                    dir.join(".mcp.json"),
                    dir.join("mcp.json"),
                    home.join(".codebuddy.json"),
                ]),
                "mcpServers",
                "用户级",
            )
        }
        Kimi => (
            agents::settings(client)?.with_file_name("mcp.json"),
            "mcpServers",
            "用户级",
        ),
        Copilot => (
            agents::copilot_home()?.join("mcp-config.json"),
            "mcpServers",
            "用户级",
        ),
    };
    Ok(Location {
        path,
        section: vec![section.into()],
        scope: Scope::User,
        label,
        directory: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_config_uses_native_paths_and_keeps_scope_ids_separate() {
        let dir = tempfile::tempdir().unwrap();
        let root = directory(dir.path()).unwrap();
        let code = project(Client::Code, Scope::Project, root.clone()).unwrap();
        assert_eq!(code.path, root.join(".mcp.json"));
        let local = project(Client::Code, Scope::Local, root.clone()).unwrap();
        let project_key = root
            .to_str()
            .unwrap()
            .replace(std::path::MAIN_SEPARATOR, "/");
        assert_eq!(local.sections(), ["projects", &project_key, "mcpServers"]);
        let user = user(Client::Code).unwrap();
        assert_eq!(local.path, user.path);
        assert_ne!(local.id(), user.id());
        assert_ne!(code.id(), local.id());
        assert_eq!(
            project(Client::Codex, Scope::Project, root.clone())
                .unwrap()
                .path,
            root.join(".codex/config.toml")
        );
        assert_eq!(
            project(Client::Qwen, Scope::Project, root.clone())
                .unwrap()
                .path,
            root.join(".qwen/settings.json")
        );
        assert_eq!(
            project(Client::Kimi, Scope::Project, root.clone())
                .unwrap()
                .path,
            root.join(".kimi-code/mcp.json")
        );
        fs::write(root.join("mcp.json"), "{}").unwrap();
        assert_eq!(
            project(Client::Codebuddy, Scope::Project, root.clone())
                .unwrap()
                .path,
            root.join("mcp.json")
        );
        fs::write(root.join(".mcp.json"), "{}").unwrap();
        assert_eq!(
            project(Client::Codebuddy, Scope::Project, root.clone())
                .unwrap()
                .path,
            root.join(".mcp.json")
        );
        assert!(project(Client::Workbuddy, Scope::Project, root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_config_refuses_linked_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), dir.path().join(".codex")).unwrap();
        assert!(project(Client::Codex, Scope::Project, dir.path().into()).is_err());
    }
}
