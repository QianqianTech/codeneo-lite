use super::{connection, revision, validate_connection};
use crate::{
    clients, desktop,
    transaction::{self, Client, Plan},
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const LOCAL: &str = "mcpServers";
const MANAGED: &str = "managedMcpServers";
const TOOL_POLICY: &str = "toolPolicy";
const CHANGED: &str = "MCP 服务配置已改变，请刷新后重试";
const EXTERNAL: &str = "此服务已由第三方档案管理。请先在 Claude Desktop 中检查同名服务";
pub(super) const MAX_TOOLS: usize = 512;

pub(super) fn tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && !matches!(name, "__proto__" | "constructor" | "prototype")
}

fn supported(name: &str, server: &Value) -> bool {
    tool_name(name)
        && !name.contains('.')
        && server.as_object().is_some_and(|fields| {
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "command" | "args" | "env"))
        })
        && server["command"].is_string()
        && server.get("args").is_none_or(Value::is_array)
        && server.get("env").is_none_or(Value::is_object)
        && connection(Client::Desktop, server)
            .is_ok_and(|config| validate_connection(&config).is_ok())
}

fn paths(root: &Path, config: &Path) -> Result<[PathBuf; 3], String> {
    let library = config
        .parent()
        .ok_or("Claude Desktop 配置目录无效")?
        .join("configLibrary");
    Ok([
        config.into(),
        library.join(format!("{}.json", clients::OWNED)),
        root.join(format!(
            "desktop-mcp-tools-{}.json",
            revision(config, &Value::Null)
        )),
    ])
}

// Run under the same lock as other settings writes. Recovery is also possible
// after a switch back to the official profile, without granting it any tools.
pub(crate) fn recover(root: &Path) -> Result<(), String> {
    if !transaction::mcp_profile_pending(root).is_file() {
        return Ok(());
    }
    if clients::desktop_managed() {
        return Err("Claude Desktop 现在由组织管理，无法恢复本机工具权限变更".into());
    }
    let config = desktop::locations()?.gateway.join(desktop::CONFIG);
    transaction::recover_mcp_profile(root, &paths(root, &config)?)
}

#[derive(Serialize)]
pub(super) struct Summary {
    pub(super) always_allow: Vec<String>,
}

pub(crate) struct Profile {
    paths: [PathBuf; 3],
    local: Plan,
    profile: Plan,
    owned: Plan,
    managed: Vec<Value>,
}

impl Profile {
    pub(crate) fn open(root: &Path, target: &desktop::Target) -> Result<Option<Self>, String> {
        if target.mode != desktop::Mode::Gateway || clients::desktop_managed() {
            return Ok(None);
        }
        Self::open_at(root, &target.path)
    }

    fn open_at(root: &Path, config: &Path) -> Result<Option<Self>, String> {
        let paths = paths(root, config)?;
        let local = Plan::open(config)?;
        if local.get(None, "deploymentMode")?.and_then(Value::as_str) != Some("3p") {
            return Ok(None);
        }
        let library = paths[1].parent().ok_or("Claude Desktop 档案目录无效")?;
        let metadata = Plan::open(&library.join("_meta.json"))?;
        if metadata.get(None, "appliedId")?.and_then(Value::as_str) != Some(clients::OWNED)
            || metadata
                .get(None, "hybridPointer")?
                .is_some_and(|value| !value.is_null())
            || !paths[1].is_file()
        {
            return Ok(None);
        }
        let profile = Plan::open(&paths[1])?;
        if profile.get(None, "bootstrapEnabled")? == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        let managed = match profile.get(None, MANAGED)? {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => value
                .as_array()
                .ok_or("第三方档案的 MCP 服务列表格式无效")?
                .clone(),
        };
        let owned = Plan::open(&paths[2])?;
        Ok(Some(Self {
            paths,
            local,
            profile,
            owned,
            managed,
        }))
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.managed
            .iter()
            .any(|value| Self::same_name(value, name))
    }

    fn same_name(value: &Value, name: &str) -> bool {
        value["name"]
            .as_str()
            .is_some_and(|key| key.eq_ignore_ascii_case(name))
    }

    fn local_conflict(&self, name: &str) -> Result<bool, String> {
        let Some(servers) = self.local.get(None, LOCAL)? else {
            return Ok(false);
        };
        Ok(servers
            .as_object()
            .ok_or("MCP 服务列表格式无效")?
            .keys()
            .any(|key| key.eq_ignore_ascii_case(name)))
    }

    pub(super) fn eligible(&self, name: &str) -> Result<bool, String> {
        if self.profile.get(None, "isLocalDevMcpEnabled")? == Some(&Value::Bool(false)) {
            return Ok(false);
        }
        let Some(server) = self.local.get(Some(LOCAL), name)? else {
            return Ok(false);
        };
        let other = self
            .local
            .get(None, LOCAL)?
            .and_then(Value::as_object)
            .is_some_and(|servers| {
                servers
                    .keys()
                    .any(|key| key != name && key.eq_ignore_ascii_case(name))
            });
        let disabled = self
            .local
            .get(None, "disabledMcpServers")?
            .and_then(Value::as_array)
            .is_some_and(|list| {
                list.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|key| key.eq_ignore_ascii_case(name))
                })
            });
        Ok(!other && !disabled && !self.contains(name) && supported(name, server))
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = (&str, &Value, Summary)> {
        self.managed.iter().filter_map(move |server| {
            let name = server["name"].as_str()?;
            if !self.is_owned(name, server)
                || self.local_conflict(name).ok()?
                || self
                    .managed
                    .iter()
                    .filter(|value| Self::same_name(value, name))
                    .count()
                    != 1
            {
                return None;
            }
            let always_allow = server[TOOL_POLICY].as_object()?.keys().cloned().collect();
            Some((name, server, Summary { always_allow }))
        })
    }

    fn is_owned(&self, name: &str, server: &Value) -> bool {
        // Match exactly what Lite wrote. Do not adopt or overwrite rules saved elsewhere.
        self.owned
            .get(None, name)
            .ok()
            .flatten()
            .and_then(Value::as_str)
            == Some(revision(&self.paths[1], server).as_str())
    }

    pub(super) fn can_undo(&self, root: &Path) -> bool {
        transaction::can_undo_mcp_profile(root, &self.paths)
    }

    pub(super) fn undo(&self, root: &Path) -> Result<(), String> {
        transaction::undo_mcp_profile(root, &self.paths)
    }

    pub(super) fn save(
        self,
        root: &Path,
        name: &str,
        expected: &str,
        names: Vec<String>,
    ) -> Result<(), String> {
        if names.len() > MAX_TOOLS || names.iter().any(|name| !tool_name(name)) {
            return Err("请选择清单中的具体工具；不支持通配授权".into());
        }
        let tools: BTreeSet<_> = names.into_iter().collect();
        self.change(root, name, Some((expected, tools)))
    }

    pub(super) fn remove(self, root: &Path, name: &str) -> Result<(), String> {
        self.change(root, name, None)
    }

    fn change(
        mut self,
        root: &Path,
        name: &str,
        request: Option<(&str, BTreeSet<String>)>,
    ) -> Result<(), String> {
        let old = self
            .managed
            .iter()
            .position(|value| Self::same_name(value, name));
        let source = if old.is_some() {
            self.entries()
                .find(|(key, _, _)| *key == name)
                .map(|(_, value, _)| value)
                .ok_or(EXTERNAL)?
        } else {
            if !self.eligible(name)? {
                return Err("此服务暂不支持在第三方档案中设置工具权限".into());
            }
            self.local.get(Some(LOCAL), name)?.ok_or(CHANGED)?
        };
        let mut local = source.as_object().ok_or(CHANGED)?.clone();
        if old.is_some() {
            for key in ["name", "transport", TOOL_POLICY] {
                local.remove(key);
            }
        }
        let keep_server = request.is_some();
        let next = if let Some((expected, tools)) = request {
            if revision(&self.paths[0], source) != expected {
                return Err(CHANGED.into());
            }
            if tools.is_empty() {
                None
            } else {
                let mut fields = local.clone();
                fields.insert("name".into(), json!(name));
                fields.insert("transport".into(), json!("stdio"));
                fields.insert(
                    TOOL_POLICY.into(),
                    Value::Object(
                        tools
                            .into_iter()
                            .map(|name| (name, json!("allow")))
                            .collect::<Map<String, Value>>(),
                    ),
                );
                Some(Value::Object(fields))
            }
        } else {
            None
        };

        if old.is_none() && next.is_none() {
            return Ok(());
        }
        let owner = next
            .as_ref()
            .map(|value| json!(revision(&self.paths[1], value)));
        let restore = old.is_some() && next.is_none() && keep_server;
        let had_managed = old.is_some();
        match (old, next) {
            (Some(index), Some(value)) => self.managed[index] = value,
            (None, Some(value)) => self.managed.push(value),
            (Some(index), None) => {
                self.managed.remove(index);
            }
            (None, None) => unreachable!(),
        }
        self.profile.set(
            None,
            MANAGED,
            (!self.managed.is_empty()).then_some(Value::Array(self.managed)),
        )?;
        self.owned.set(None, name, owner)?;
        if restore {
            self.local
                .set(Some(LOCAL), name, Some(Value::Object(local)))?;
        } else if !had_managed {
            self.local.set(Some(LOCAL), name, None)?;
        }
        // Remove the old registration first, so Claude never sees duplicate servers.
        let ordered = if restore {
            [self.profile, self.local, self.owned]
        } else {
            [self.local, self.profile, self.owned]
        };
        transaction::commit_mcp_profile(
            root,
            ordered.into_iter().filter_map(Plan::finish).collect(),
            &self.paths,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, value: &Value) {
        fs::write(path, transaction::json_bytes(value).unwrap()).unwrap();
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn exact_permissions_follow_the_profile_and_can_be_cleared_or_undone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config = root.join(desktop::CONFIG);
        let [_, profile, owners] = paths(root, &config).unwrap();
        let library = profile.parent().unwrap();
        fs::create_dir(library).unwrap();
        write(
            &library.join("_meta.json"),
            &json!({"appliedId":clients::OWNED,"hybridPointer":null}),
        );
        let playwright =
            json!({"command":"node","args":["fixture.js"],"env":{"FIXTURE":"fixture-value"}});
        let native = json!({"deploymentMode":"3p","mcpServers":{"playwright":playwright,"other":{"command":"other"}},"keep":true});
        let other = json!({"name":"external","transport":"stdio","command":"external","toolPolicy":{"query":"deny"}});
        write(&config, &native);
        write(
            &profile,
            &json!({"managedMcpServers":[other],"inferenceModels":[{"name":"kimi"}]}),
        );

        let current = || Profile::open_at(root, &config).unwrap().unwrap();
        assert!(current().eligible("playwright").unwrap());
        let before = revision(&config, &playwright);
        current()
            .save(root, "playwright", &before, vec!["browser_snapshot".into()])
            .unwrap();
        let configured = read(&profile);
        assert_eq!(configured[MANAGED][0], other);
        let moved = &configured[MANAGED][1];
        assert_eq!(moved["transport"], "stdio");
        assert_eq!(moved["env"], playwright["env"]);
        assert_eq!(moved[TOOL_POLICY], json!({"browser_snapshot":"allow"}));
        assert_eq!(read(&config).pointer("/mcpServers/playwright"), None);
        assert_eq!(
            read(&config)["mcpServers"]["other"],
            native["mcpServers"]["other"]
        );
        assert!(!fs::read_to_string(owners)
            .unwrap()
            .contains("fixture-value"));
        let after = revision(&config, moved);

        let mut model_switch = Plan::open(&profile).unwrap();
        model_switch
            .set(None, "inferenceModels", Some(json!([{"name":"glm"}])))
            .unwrap();
        transaction::write_single(model_switch).unwrap();
        assert_eq!(
            current().entries().next().unwrap().2.always_allow,
            ["browser_snapshot"]
        );
        assert!(current().save(root, "playwright", &before, vec![]).is_err());
        assert!(current()
            .save(root, "playwright", &after, vec!["*".into()])
            .is_err());
        current().save(root, "playwright", &after, vec![]).unwrap();
        assert_eq!(read(&config), native);
        assert_eq!(read(&profile)[MANAGED], json!([other]));
        assert!(current().can_undo(root));
        current().undo(root).unwrap();
        assert_eq!(read(&profile)[MANAGED][1], *moved);
        assert_eq!(read(&profile)["inferenceModels"][0]["name"], "glm");
        current().remove(root, "playwright").unwrap();
        assert_eq!(read(&config).pointer("/mcpServers/playwright"), None);
        assert_eq!(read(&profile)[MANAGED], json!([other]));
        current().undo(root).unwrap();
        assert_eq!(current().entries().next().unwrap().0, "playwright");
    }

    #[test]
    fn external_or_unsupported_definitions_are_not_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config = root.join(desktop::CONFIG);
        let [_, profile, _] = paths(root, &config).unwrap();
        let library = profile.parent().unwrap();
        fs::create_dir(library).unwrap();
        let meta = library.join("_meta.json");
        let native = json!({"deploymentMode":"3p","mcpServers":{
            "playwright":{"command":"fixture"}, "remote":{"url":"https://example.invalid/mcp"},
            "filesystem":{"command":"fixture","cwd":"/fixture"}
        }});
        let external = json!({MANAGED:[{"name":"Playwright","transport":"stdio","command":"other","toolPolicy":{"query":"allow"}}]});
        write(&config, &native);
        write(&profile, &external);
        write(&meta, &json!({"appliedId":"some-other-profile"}));
        assert!(Profile::open_at(root, &config).unwrap().is_none());
        write(&meta, &json!({"appliedId":clients::OWNED}));
        let current = || Profile::open_at(root, &config).unwrap().unwrap();
        for name in ["playwright", "remote", "filesystem"] {
            assert!(!current().eligible(name).unwrap());
            assert!(current()
                .save(
                    root,
                    name,
                    &revision(&config, &native[LOCAL][name]),
                    vec!["query".into()]
                )
                .is_err());
        }
        assert_eq!(read(&profile), external);
        assert_eq!(read(&config), native);
    }
}
