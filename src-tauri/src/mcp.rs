use crate::{
    agents, files,
    transaction::{self, Client, Plan},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub mod catalog;
mod import;
pub mod options;
mod probe;
mod targets;
pub use import::{parse_import, Imported};
pub use probe::{check, Check};
pub use targets::{directory, scopes, Destination, Location, Scope, Selection};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Stdio,
    Http,
    Sse,
}

impl Transport {
    fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }

    fn from_entry(client: Client, server: &Value) -> Option<Self> {
        match server["transport"]
            .as_str()
            .or_else(|| server["type"].as_str())
        {
            Some("sse") => Some(Self::Sse),
            Some("http" | "streamable-http" | "streamableHttp") => Some(Self::Http),
            Some("stdio" | "local") => Some(Self::Stdio),
            Some(kind) if kind != "remote" => None,
            _ if server.get("command").is_some() => Some(Self::Stdio),
            _ if server.get("httpUrl").is_some() || server.get("serverUrl").is_some() => {
                Some(Self::Http)
            }
            _ if server.get("url").is_some() => Some(if client == Client::Qwen {
                Self::Sse
            } else {
                Self::Http
            }),
            _ => None,
        }
    }

    fn endpoint_keys(self, client: Client) -> &'static [&'static str] {
        match (self, client) {
            (Self::Stdio, _) => &["command"],
            (Self::Http, Client::Qwen) => &["httpUrl", "url", "serverUrl"],
            _ => &["url", "httpUrl", "serverUrl"],
        }
    }

    fn option_keys(self, client: Client) -> &'static [&'static str] {
        match (self, client) {
            (Self::Stdio, Client::Opencode) => &["environment", "env"],
            (Self::Stdio, _) => &["env", "environment"],
            (_, Client::Codex) => &["http_headers", "headers"],
            _ => &["headers", "http_headers"],
        }
    }
}

pub fn transports(client: Client) -> &'static [Transport] {
    agents::spec(client).transports
}

#[derive(Serialize)]
struct Server {
    name: String,
    transport: &'static str,
    enabled: bool,
}

#[derive(Serialize)]
pub struct Configured {
    location: String,
    path: String,
    label: &'static str,
    can_undo: bool,
    servers: Vec<Server>,
}

pub fn list(root: &Path, client: Client, selection: Selection) -> Result<Configured, String> {
    agents::ensure_compatible(client)?;
    let location = selection.resolve(client)?;
    let plan = open(client, &location.path)?;
    let mut servers = list_at(&plan, &location.sections(), client)?;
    if client == Client::Copilot {
        let settings = Plan::open(&agents::copilot_home()?.join("settings.json"))?;
        if let Some(disabled) = settings
            .get(None, "disabledMcpServers")?
            .and_then(Value::as_array)
        {
            for server in &mut servers {
                server.enabled &= !disabled.iter().any(|id| id == &server.name);
            }
        }
    }
    Ok(Configured {
        location: location.id(),
        path: location.path.to_str().ok_or("MCP 配置路径编码无效")?.into(),
        label: location.label,
        can_undo: transaction::can_undo_mcp(
            root,
            client,
            location.backup_slot().as_deref(),
            &location.path,
        ),
        servers,
    })
}

fn open(client: Client, path: &Path) -> Result<Plan, String> {
    Plan::open_as(path, agents::is_added(client))
}

fn list_at(plan: &Plan, sections: &[&str], client: Client) -> Result<Vec<Server>, String> {
    let (section, parents) = sections.split_last().ok_or("找不到 MCP 配置项")?;
    let Some(servers) = plan.get_at(parents, section)? else {
        return Ok(Vec::new());
    };
    let servers = servers.as_object().ok_or("MCP 服务器列表格式无效")?;
    let disabled = plan
        .get(None, "disabledMcpServers")?
        .and_then(Value::as_array);
    Ok(servers
        .iter()
        .map(|(name, server)| Server {
            name: name.clone(),
            transport: Transport::from_entry(client, server).map_or("other", Transport::label),
            enabled: server["enabled"] != false
                && server["disabled"] != true
                && !disabled.is_some_and(|entries| entries.iter().any(|entry| entry == name)),
        })
        .collect())
}

#[derive(Deserialize, Serialize)]
pub struct Input {
    name: String,
    #[serde(flatten)]
    connection: Connection,
    revision: Option<String>,
    #[serde(default)]
    advanced: BTreeMap<String, Option<Value>>,
}

#[derive(Deserialize, Serialize)]
struct Connection {
    transport: Transport,
    endpoint: String,
    args: Vec<String>,
    options: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct Editable {
    #[serde(flatten)]
    connection: Connection,
    revision: String,
    advanced: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Verification {
    Saved { name: String },
    Draft { input: Input },
}

fn revision(path: &Path, server: &Value) -> String {
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(server.to_string().as_bytes());
    format!("{:x}", hash.finalize())
}

fn configured<'a>(plan: &'a Plan, section: &[&str], name: &str) -> Result<&'a Value, String> {
    if !name_valid(name) {
        return Err("MCP 服务名称无效".into());
    }
    plan.get_at(section, name)?
        .ok_or_else(|| "找不到此 MCP 服务，请刷新列表".into())
}

pub fn editable(client: Client, target: Destination, name: &str) -> Result<Editable, String> {
    agents::ensure_compatible(client)?;
    let location = target.resolve(client)?;
    let plan = open(client, &location.path)?;
    let server = configured(&plan, &location.sections(), name)?;
    let connection = connection(client, server)?;
    Ok(Editable {
        advanced: options::read(client, connection.transport, server),
        connection,
        revision: revision(&location.path, server),
    })
}

fn existing_key<'a>(server: &Value, keys: &'a [&'a str]) -> &'a str {
    keys.iter()
        .copied()
        .find(|key| server.get(key).is_some())
        .unwrap_or(keys[0])
}

fn string_list(value: &Value, error: &str) -> Result<Vec<String>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(value.clone()).map_err(|_| error.to_owned())
}

fn connection(client: Client, server: &Value) -> Result<Connection, String> {
    let transport = Transport::from_entry(client, server).ok_or("无法识别此 MCP 服务的连接方式")?;
    let endpoint = &server[existing_key(server, transport.endpoint_keys(client))];
    let (endpoint, args) = if transport == Transport::Stdio && endpoint.is_array() {
        let command = string_list(endpoint, "MCP 启动命令数组格式无效")?;
        let mut command = command.into_iter();
        (command.next().unwrap_or_default(), command.collect())
    } else {
        let endpoint = match endpoint {
            Value::String(value) => value.clone(),
            Value::Null => String::new(),
            _ => return Err("MCP 启动命令或服务地址格式无效".into()),
        };
        let args = if transport == Transport::Stdio {
            string_list(&server["args"], "MCP 启动参数格式无效")?
        } else {
            Vec::new()
        };
        (endpoint, args)
    };
    let options = &server[existing_key(server, transport.option_keys(client))];
    let options = if options.is_null() {
        BTreeMap::new()
    } else {
        serde_json::from_value(options.clone()).map_err(|_| "MCP 环境变量或请求头格式无效")?
    };
    Ok(Connection {
        transport,
        endpoint,
        args,
        options,
    })
}

fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}

fn checked_url(endpoint: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "请输入 MCP 服务提供的完整 HTTP 地址")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || endpoint.len() > 8192
    {
        return Err("MCP 地址需要有效的 HTTP 或 HTTPS URL。认证信息可填在请求头中".into());
    }
    Ok(url)
}

fn entry(client: Client, input: &Connection) -> Result<Value, String> {
    use Client::*;
    validate_connection(input)?;
    let endpoint = input.endpoint.trim();
    if input.transport == Transport::Stdio {
        let mut server = json!({"command": endpoint, "args": input.args});
        if matches!(client, Code | Codebuddy) {
            server["type"] = json!("stdio");
        } else if client == Opencode {
            let command: Vec<_> = std::iter::once(endpoint)
                .chain(input.args.iter().map(String::as_str))
                .collect();
            server = json!({"type":"local","command":command});
        } else if client == Copilot {
            server["type"] = json!("local");
            server["tools"] = json!(["*"]);
        }
        if !input.options.is_empty() {
            server[input.transport.option_keys(client)[0]] = json!(input.options);
        }
        return Ok(server);
    }
    let url = checked_url(endpoint)?;
    let sse = input.transport == Transport::Sse;
    let kind = if sse { "sse" } else { "http" };
    let mut server = match client {
        Opencode => json!({"type":"remote", "url":url.as_str()}),
        Qwen if !sse => json!({"httpUrl":url.as_str()}),
        Kimi if sse => json!({"transport":"sse", "url":url.as_str()}),
        Code | Codebuddy | Workbuddy => json!({"type":kind, "url":url.as_str()}),
        Copilot => json!({"type":kind, "url":url.as_str(), "tools":["*"]}),
        _ => json!({"url":url.as_str()}),
    };
    if !input.options.is_empty() {
        server[input.transport.option_keys(client)[0]] = json!(input.options);
    }
    Ok(server)
}

fn validate_connection(input: &Connection) -> Result<(), String> {
    if input.transport == Transport::Stdio {
        let endpoint = input.endpoint.trim();
        if endpoint.is_empty() || endpoint.len() > 4096 || endpoint.chars().any(char::is_control) {
            return Err("请输入本地 MCP 的命令或可执行文件路径".into());
        }
        if input.args.len() > 128
            || input
                .args
                .iter()
                .any(|arg| arg.len() > 8192 || arg.contains('\0'))
        {
            return Err("启动参数过长或含无效字符".into());
        }
        if input
            .options
            .iter()
            .any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0'))
        {
            return Err("环境变量格式无效".into());
        }
    } else {
        let mut names = std::collections::HashSet::new();
        for (name, value) in &input.options {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "自定义请求头名称无效")?;
            reqwest::header::HeaderValue::from_str(value).map_err(|_| "自定义请求头值无效")?;
            if !names.insert(name) {
                return Err("MCP 请求头名称重复（名称不区分大小写）".into());
            }
        }
    }
    Ok(())
}

fn prepared_entry(
    path: &Path,
    client: Client,
    input: &Input,
    existing: Option<&Value>,
) -> Result<Value, String> {
    let ascii = input
        .name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !name_valid(&input.name)
        || (input.revision.is_none() && !ascii && client != Client::Workbuddy)
    {
        return Err("服务名称无效。添加服务时可使用英文、数字、短横线或下划线".into());
    }
    let server = match (existing, input.revision.as_deref()) {
        (None, None) => {
            if !transports(client).contains(&input.connection.transport) {
                return Err("所选客户端不支持这种 MCP 连接方式".into());
            }
            let mut created = entry(client, &input.connection)?;
            options::apply(
                client,
                None,
                input.connection.transport,
                &input.advanced,
                created.as_object_mut().expect("MCP server object"),
            )?;
            return Ok(created);
        }
        (Some(_), None) => return Err("已有同名 MCP 服务，请选择编辑或换一个名称".into()),
        (Some(server), Some(version)) if revision(path, server) == version => server,
        _ => return Err("MCP 配置已变化，请重新打开编辑器".into()),
    };
    let before = connection(client, server)?;
    let next = &input.connection;
    if next.transport != before.transport && !transports(client).contains(&next.transport) {
        return Err("所选客户端不支持这种 MCP 连接方式".into());
    }
    let generated = entry(client, next)?;
    let mut updated = server.as_object().ok_or("MCP 服务配置格式无效")?.clone();
    if before.transport == next.transport {
        update_connection_fields(client, &mut updated, &before, next, &generated);
    } else {
        for key in [
            "command",
            "args",
            "env",
            "environment",
            "url",
            "httpUrl",
            "serverUrl",
            "http_headers",
            "headers",
            "type",
            "transport",
        ] {
            updated.remove(key);
        }
        if client == Client::Codex {
            let incompatible: &[&str] = if next.transport == Transport::Stdio {
                &[
                    "auth",
                    "bearer_token_env_var",
                    "env_http_headers",
                    "http_headers_helper",
                    "oauth",
                    "oauth_resource",
                    "scopes",
                ]
            } else {
                &["cwd", "env_vars"]
            };
            for key in incompatible {
                updated.remove(*key);
            }
        }
        let tools = updated.get("tools").cloned();
        updated.extend(generated.as_object().expect("MCP server object").clone());
        if let Some(tools) = tools {
            updated.insert("tools".into(), tools);
        }
    }
    options::apply(
        client,
        Some(before.transport),
        next.transport,
        &input.advanced,
        &mut updated,
    )?;
    Ok(Value::Object(updated))
}

fn update_connection_fields(
    client: Client,
    updated: &mut Map<String, Value>,
    before: &Connection,
    next: &Connection,
    generated: &Value,
) {
    let endpoint_keys = next.transport.endpoint_keys(client);
    let endpoint_key = endpoint_keys
        .iter()
        .copied()
        .find(|key| updated.contains_key(*key));
    let command_array = next.transport == Transport::Stdio
        && updated
            .get("command")
            .map_or(client == Client::Opencode, Value::is_array);
    if before.endpoint != next.endpoint || (command_array && before.args != next.args) {
        let value = if command_array {
            json!(std::iter::once(next.endpoint.trim())
                .chain(next.args.iter().map(String::as_str))
                .collect::<Vec<_>>())
        } else {
            generated[endpoint_keys[0]].clone()
        };
        updated.insert(endpoint_key.unwrap_or(endpoint_keys[0]).into(), value);
    }
    if next.transport == Transport::Stdio && !command_array && before.args != next.args {
        updated.insert("args".into(), json!(next.args));
    }
    if before.options != next.options {
        let option_keys = next.transport.option_keys(client);
        if next.options.is_empty() {
            for key in option_keys {
                updated.remove(*key);
            }
        } else {
            let key = option_keys
                .iter()
                .copied()
                .find(|key| updated.contains_key(*key));
            updated.insert(key.unwrap_or(option_keys[0]).into(), json!(next.options));
        }
    }
}

pub fn verification_entry(
    client: Client,
    destination: Destination,
    target: Verification,
) -> Result<(Value, Location), String> {
    agents::ensure_compatible(client)?;
    let location = destination.resolve(client)?;
    let section = location.sections();
    let plan = open(client, &location.path)?;
    let server = match target {
        Verification::Saved { name } => configured(&plan, &section, &name)?.clone(),
        Verification::Draft { input } => prepared_entry(
            &location.path,
            client,
            &input,
            plan.get_at(&section, &input.name)?,
        )?,
    };
    Ok((server, location))
}

pub fn save(
    root: &Path,
    client: Client,
    destination: Destination,
    input: Input,
) -> Result<(), String> {
    agents::ensure_compatible(client)?;
    let _lock = files::lock(root)?;
    let location = destination.resolve(client)?;
    let plan = save_at(
        open(client, &location.path)?,
        &location.path,
        &location.sections(),
        client,
        &input,
    )?;
    commit(root, client, &location, plan)
}

fn commit(root: &Path, client: Client, location: &Location, plan: Plan) -> Result<(), String> {
    transaction::commit_mcp(root, client, location.backup_slot().as_deref(), plan)
}

fn save_at(
    mut plan: Plan,
    path: &Path,
    section: &[&str],
    client: Client,
    input: &Input,
) -> Result<Plan, String> {
    let existing = plan.get_at(section, &input.name)?;
    let server = prepared_entry(path, client, input, existing)?;
    if let Some(existing) = existing.and_then(Value::as_object) {
        let fields = server.as_object().expect("MCP server object");
        let removed: Vec<_> = existing
            .keys()
            .filter(|key| !fields.contains_key(*key))
            .cloned()
            .collect();
        let parent: Vec<_> = section
            .iter()
            .copied()
            .chain([input.name.as_str()])
            .collect();
        for key in removed {
            plan.set_at(&parent, &key, None)?;
        }
        for (key, value) in fields {
            plan.set_at(&parent, key, Some(value.clone()))?;
        }
    } else {
        plan.set_at(section, &input.name, Some(server))?;
    }
    Ok(plan)
}

pub fn remove(
    root: &Path,
    client: Client,
    destination: Destination,
    name: &str,
) -> Result<(), String> {
    agents::ensure_compatible(client)?;
    if !name_valid(name) {
        return Err("MCP 服务名称无效".into());
    }
    let _lock = files::lock(root)?;
    let location = destination.resolve(client)?;
    let mut plan = open(client, &location.path)?;
    plan.set_at(&location.sections(), name, None)?;
    commit(root, client, &location, plan)
}

pub fn supports_toggle(client: Client) -> bool {
    agents::spec(client).toggle
}

pub fn set_enabled(
    root: &Path,
    client: Client,
    destination: Destination,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    agents::ensure_compatible(client)?;
    if !supports_toggle(client) || !name_valid(name) {
        return Err("此客户端请在自己的 MCP 设置中启停服务".into());
    }
    let _lock = files::lock(root)?;
    let location = destination.resolve(client)?;
    let plan = toggle_at(
        open(client, &location.path)?,
        &location.sections(),
        name,
        client,
        enabled,
    )?;
    commit(root, client, &location, plan)
}

pub fn undo(root: &Path, client: Client, destination: Destination) -> Result<(), String> {
    let _lock = files::lock(root)?;
    let location = destination.resolve(client)?;
    transaction::undo_mcp(
        root,
        client,
        location.backup_slot().as_deref(),
        &location.path,
    )
}

fn toggle_at(
    mut plan: Plan,
    section: &[&str],
    name: &str,
    client: Client,
    enabled: bool,
) -> Result<Plan, String> {
    let server = plan
        .get_at(section, name)?
        .and_then(Value::as_object)
        .ok_or("此 MCP 服务已移除，请刷新列表")?;
    let key = if matches!(client, Client::Codex | Client::Kimi | Client::Opencode) {
        "enabled"
    } else {
        "disabled"
    };
    let path: Vec<_> = section.iter().copied().chain([name]).collect();
    let other = if key == "enabled" {
        "disabled"
    } else {
        "enabled"
    };
    if server.contains_key(other) {
        plan.set_at(&path, other, None)?;
    }
    plan.set_at(
        &path,
        key,
        Some(json!(if key == "enabled" { enabled } else { !enabled })),
    )?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn input(name: &str, transport: Transport, endpoint: &str) -> Input {
        Input {
            name: name.into(),
            connection: Connection {
                transport,
                endpoint: endpoint.into(),
                args: Vec::new(),
                options: BTreeMap::new(),
            },
            revision: None,
            advanced: BTreeMap::new(),
        }
    }

    #[test]
    fn adds_independent_mcp_entries_without_exposing_headers_or_changing_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# preferences\nmodel = 'gpt-6.1-sol'\n").unwrap();
        let mut remote = input("search", Transport::Http, "https://example.invalid/mcp");
        remote
            .connection
            .options
            .insert("Authorization".into(), "Bearer fixture-secret".into());
        let native = entry(Client::Codex, &remote.connection).unwrap();
        assert!(native["http_headers"].is_object());
        assert!(native.get("type").is_none());
        let open = || Plan::open(&path).unwrap();
        let save = |plan| transaction::write_single(plan).unwrap();
        let section = &["mcp_servers"];
        save(save_at(open(), &path, section, Client::Codex, &remote).unwrap());
        assert!(save_at(open(), &path, section, Client::Codex, &remote).is_err());
        let mut local = input("docs", Transport::Stdio, "node");
        local.connection.args = vec!["docs.mjs".into()];
        save(save_at(open(), &path, section, Client::Codex, &local).unwrap());
        save(toggle_at(open(), &["mcp_servers"], "search", Client::Codex, false).unwrap());
        let result =
            serde_json::to_value(list_at(&open(), &["mcp_servers"], Client::Codex).unwrap())
                .unwrap();
        assert_eq!(result.as_array().unwrap().len(), 2);
        assert_eq!(result[1]["enabled"], false);
        assert!(!result.to_string().contains("fixture-secret"));
        let doc = fs::read_to_string(path).unwrap();
        assert!(doc.contains("# preferences"));
        assert!(doc.contains("model = 'gpt-6.1-sol'"));
    }

    #[test]
    fn edit_reads_inline_codex_fields_and_preserves_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[mcp_servers]\nsearch = { url = 'https://example.invalid/mcp', http_headers = { Authorization = 'Bearer fixture-secret' }, enabled = false, disabled_tools = ['one'] } # keep this\n").unwrap();
        let open = || Plan::open(&path).unwrap();
        let section = &["mcp_servers"];
        let plan = open();
        let saved = configured(&plan, section, "search").unwrap();
        let mut edit = Input {
            name: "search".into(),
            connection: connection(Client::Codex, saved).unwrap(),
            revision: Some(revision(&path, saved)),
            advanced: BTreeMap::new(),
        };
        assert_eq!(edit.connection.endpoint, "https://example.invalid/mcp");
        assert_eq!(
            edit.connection.options["Authorization"],
            "Bearer fixture-secret"
        );
        assert!(save_at(open(), &path, section, Client::Codex, &edit)
            .unwrap()
            .finish()
            .is_none());
        edit.connection.endpoint = "https://example.invalid/updated".into();
        edit.connection.options.clear();
        let root = dir.path().join("lite");
        let _lock = files::lock(&root).unwrap();
        let plan = save_at(open(), &path, section, Client::Codex, &edit).unwrap();
        transaction::commit_mcp(&root, Client::Codex, None, plan).unwrap();
        let updated = open();
        let server = configured(&updated, section, "search").unwrap();
        assert_eq!(server["url"], "https://example.invalid/updated");
        assert!(server.get("http_headers").is_none());
        assert_eq!(server["enabled"], false);
        assert_eq!(server["disabled_tools"], json!(["one"]));
        assert!(fs::read_to_string(&path).unwrap().contains("# keep this"));
        assert!(save_at(open(), &path, section, Client::Codex, &edit).is_err());
        transaction::undo_mcp(&root, Client::Codex, None, &path).unwrap();
        assert_eq!(configured(&open(), section, "search").unwrap(), saved);
    }

    #[test]
    fn edits_command_arrays_and_legacy_remote_fields_without_changing_their_shape() {
        let path = Path::new("config.jsonc");
        let local = json!({"type":"local", "command":["node", "", "server.js"], "environment":{"A":"first\nsecond"}, "enabled":false});
        let mut edit = Input {
            name: "local".into(),
            connection: connection(Client::Opencode, &local).unwrap(),
            revision: Some(revision(path, &local)),
            advanced: BTreeMap::new(),
        };
        assert_eq!(edit.connection.args, ["", "server.js"]);
        assert_eq!(edit.connection.options["A"], "first\nsecond");
        edit.connection.args[1] = "new.js".into();
        let updated = prepared_entry(path, Client::Opencode, &edit, Some(&local)).unwrap();
        assert_eq!(updated["command"], json!(["node", "", "new.js"]));
        assert_eq!(updated["environment"], local["environment"]);
        assert_eq!(updated["enabled"], false);

        let remote =
            json!({"type":"sse", "serverUrl":"https://example.invalid/sse", "disabled":true});
        let mut edit = Input {
            name: "remote".into(),
            connection: connection(Client::Workbuddy, &remote).unwrap(),
            revision: Some(revision(path, &remote)),
            advanced: BTreeMap::new(),
        };
        edit.connection.endpoint = "https://example.invalid/sse2".into();
        let updated = prepared_entry(path, Client::Workbuddy, &edit, Some(&remote)).unwrap();
        assert_eq!(updated["serverUrl"], "https://example.invalid/sse2");
        assert_eq!(updated["type"], "sse");
        assert_eq!(updated["disabled"], true);
    }

    #[test]
    fn switching_codex_transport_removes_old_connection_fields() {
        let path = Path::new("config.toml");
        let previous = json!({"url":"https://example.invalid/mcp", "bearer_token_env_var":"MCP_TOKEN",
            "env_http_headers":{"X-Token":"OTHER_TOKEN"}, "startup_timeout_sec":12, "enabled":false});
        let mut edit = input("search", Transport::Stdio, "node");
        edit.revision = Some(revision(path, &previous));
        let updated = prepared_entry(path, Client::Codex, &edit, Some(&previous)).unwrap();
        assert_eq!(
            updated,
            json!({"command":"node", "args":[], "startup_timeout_sec":12, "enabled":false})
        );
    }

    #[test]
    fn project_save_and_undo_leave_global_and_other_project_settings_intact() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lite");
        let workspace = dir.path().join("project with spaces");
        let other = dir.path().join("other");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&other).unwrap();
        let select = |directory: &Path| {
            serde_json::from_value::<Selection>(json!({"scope":"project", "directory":directory}))
                .unwrap()
        };
        let listed = list(&root, Client::Code, select(&workspace)).unwrap();
        let destination = || {
            serde_json::from_value::<Destination>(json!({
                "scope":"project", "directory":workspace, "location": listed.location
            }))
            .unwrap()
        };
        let mismatched = serde_json::from_value::<Destination>(json!({
            "scope":"project", "directory":other, "location": listed.location
        }))
        .unwrap();
        let config = workspace.join(".mcp.json");
        fs::write(
            &config,
            br#"{"custom":{"keep":true},"mcpServers":{"existing":{"command":"keep"}}}"#,
        )
        .unwrap();
        let make = || input("added", Transport::Http, "https://example.invalid/mcp");
        assert!(save(&root, Client::Code, mismatched, make()).is_err());
        assert!(!other.join(".mcp.json").exists());
        save(&root, Client::Code, destination(), make()).unwrap();
        let (server, location) = verification_entry(
            Client::Code,
            destination(),
            Verification::Saved {
                name: "added".into(),
            },
        )
        .unwrap();
        assert_eq!(server["type"], "http");
        assert_eq!(location.path, dunce::canonicalize(&config).unwrap());
        assert!(
            list(&root, Client::Code, select(&workspace))
                .unwrap()
                .can_undo
        );
        assert!(!list(&root, Client::Code, select(&other)).unwrap().can_undo);
        undo(&root, Client::Code, destination()).unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        assert_eq!(
            after,
            json!({"custom":{"keep":true},"mcpServers":{"existing":{"command":"keep"}}})
        );
    }

    #[test]
    fn edits_only_supported_native_fields_and_keeps_unedited_authentication() {
        let path = Path::new("config.toml");
        let old = json!({
            "url":"https://example.invalid/mcp", "startup_timeout_ms":12000, "enabled_tools":["old"],
            "http_headers_helper":"existing-helper", "oauth":{"client_id":"existing"},
        });
        let mut edit = input("fixture", Transport::Http, "https://example.invalid/mcp");
        edit.revision = Some(revision(path, &old));
        edit.advanced
            .insert("startup_timeout_sec".into(), Some(json!(12.5)));
        edit.advanced.insert("enabled_tools".into(), None);
        edit.advanced
            .insert("bearer_token_env_var".into(), Some(json!("MCP_TOKEN")));
        let saved = prepared_entry(path, Client::Codex, &edit, Some(&old)).unwrap();
        assert_eq!(saved["startup_timeout_sec"], 12.5);
        assert_eq!(saved["bearer_token_env_var"], "MCP_TOKEN");
        assert_eq!(saved["oauth"], old["oauth"]);
        assert_eq!(saved["http_headers_helper"], "existing-helper");
        assert!(saved.get("startup_timeout_ms").is_none());
        assert!(saved.get("enabled_tools").is_none());
        edit.advanced
            .insert("cwd".into(), Some(json!("/unexpected")));
        assert!(prepared_entry(path, Client::Codex, &edit, Some(&old)).is_err());
    }
}
