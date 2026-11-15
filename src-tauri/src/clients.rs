use crate::{
    codex, desktop, desktop_models, files,
    gateway::{self, Connection},
    models::{self, Applied, Pending, Store},
    transaction::{self, Client, Delta, Plan},
    vault, workbuddy,
};
use claudekeeper_claude::{profile_env, AuthMode, ProfileApply};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) const OWNED: &str = "69ae28b4-21a5-47c2-98d3-aeb0ecb21d5b";
const CODE_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_REASONING_MODEL",
    "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
    CODE_MODEL_OPTION,
    CODE_MODEL_OPTION_NAME,
];
pub(crate) const CODE_MODEL_OPTION: &str = "ANTHROPIC_CUSTOM_MODEL_OPTION";
pub(crate) const CODE_MODEL_OPTION_NAME: &str = "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME";

pub(crate) struct DesktopFiles {
    native: PathBuf,
    gateway: PathBuf,
    pub(crate) meta: PathBuf,
    pub(crate) owned: PathBuf,
}

impl DesktopFiles {
    pub(crate) fn new(locations: desktop::Locations) -> Self {
        let library = locations.gateway.join("configLibrary");
        Self {
            native: locations.native.join(desktop::CONFIG),
            gateway: locations.gateway.join(desktop::CONFIG),
            meta: library.join("_meta.json"),
            owned: library.join(format!("{OWNED}.json")),
        }
    }

    fn paths(&self) -> [PathBuf; 4] {
        [
            self.native.clone(),
            self.gateway.clone(),
            self.meta.clone(),
            self.owned.clone(),
        ]
    }
}

pub(crate) fn code_path() -> Result<PathBuf, String> {
    let dir = if let Some(dir) = env::var_os("CLAUDE_CONFIG_DIR") {
        PathBuf::from(dir)
    } else {
        dirs::home_dir()
            .ok_or("找不到当前用户的资料目录")?
            .join(".claude")
    };
    if !dir.is_absolute() {
        return Err("CLAUDE_CONFIG_DIR 必须是绝对路径".into());
    }
    Ok(dir.join("settings.json"))
}

pub(crate) fn allowed(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![models::path(root)];
    paths.extend(code_path().ok());
    if let Ok(config) = codex::path() {
        paths.push(codex::catalog_path(&config));
        paths.push(config);
    }
    paths.extend(workbuddy::path().ok());
    if let Ok(locations) = desktop::locations() {
        paths.extend(DesktopFiles::new(locations).paths());
    }
    paths.extend(
        Client::ALL
            .into_iter()
            .filter(|c| crate::agents::is_added(*c))
            .filter_map(|client| crate::agents::model_paths(client).ok())
            .flatten(),
    );
    paths
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn desktop_managed() -> bool {
    #[cfg(target_os = "macos")]
    {
        let root = Path::new("/Library/Managed Preferences");
        let plist = "com.anthropic.claudefordesktop.plist";
        return root.join(plist).is_file()
            || dirs::home_dir()
                .and_then(|home| home.file_name().map(|user| root.join(user).join(plist)))
                .is_some_and(|p| p.is_file());
    }
    #[cfg(target_os = "linux")]
    Path::new("/etc/claude-desktop/managed-settings.json").is_file()
}

#[cfg(target_os = "windows")]
pub(crate) fn desktop_managed() -> bool {
    [
        r"HKLM\SOFTWARE\Policies\Claude",
        r"HKCU\SOFTWARE\Policies\Claude",
    ]
    .iter()
    .any(|key| {
        hidden_command("reg.exe")
            .args(["query", key])
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

#[cfg(target_os = "windows")]
fn hidden_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(program);
    command.creation_flags(0x08000000);
    command
}

#[cfg(unix)]
fn code_running() -> Result<bool, String> {
    let user = unsafe { libc::geteuid() }.to_string();
    let result = Command::new("/bin/ps")
        .args(["-ww", "-u", &user, "-o", "pid=,comm="])
        .output()
        .map_err(|_| "无法检查 Claude Code 会话状态")?;
    if !result.status.success() {
        return Err("无法检查 Claude Code 会话状态".into());
    }
    let mut engines = Vec::new();
    for (pid, executable) in
        claudekeeper_claude::parse_ps_commands(&String::from_utf8_lossy(&result.stdout))
    {
        if native_code(&executable) {
            return Ok(true);
        }
        if matches!(executable.rsplit('/').next(), Some("node" | "bun")) {
            engines.push(pid.to_string());
        }
    }
    for batch in engines.chunks(128) {
        let result = Command::new("/bin/ps")
            .args(["-ww", "-p", &batch.join(","), "-o", "pid=,args="])
            .output()
            .map_err(|_| "无法检查 Claude Code 会话状态")?;
        if result.status.code() == Some(1) && result.stdout.is_empty() {
            continue;
        }
        if !result.status.success() {
            return Err("无法检查 Claude Code 会话状态".into());
        }
        if claudekeeper_claude::parse_ps_commands(&String::from_utf8_lossy(&result.stdout))
            .iter()
            .any(|(_, args)| args.contains("@anthropic-ai/claude-code/") && args.contains("cli.js"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn native_code(executable: &str) -> bool {
    !executable.contains(".app/")
        && (matches!(
            executable.rsplit('/').next(),
            Some("claude" | "claude-code")
        ) || executable.contains("/.local/share/claude/versions/")
            || executable.contains("/node_modules/@anthropic-ai/claude-code/"))
}

#[cfg(target_os = "windows")]
fn code_running() -> Result<bool, String> {
    let script = r#"
        $ErrorActionPreference='Stop'
        $session = (Get-Process -Id $PID).SessionId
        $found = Get-CimInstance Win32_Process -Filter "Name='claude.exe' OR Name='node.exe' OR Name='bun.exe'" |
          Where-Object { $_.SessionId -eq $session } |
          Where-Object {
              if ($_.Name -ieq 'claude.exe') {
                  if (-not $_.ExecutablePath) { throw 'Unknown Claude executable' }
                  $_.ExecutablePath -notmatch 'WindowsApps|AnthropicClaude|Claude Desktop|claude-desktop|[\\/]Programs[\\/]Claude[\\/]|[\\/]Local[\\/]Claude[\\/]'
              } else {
                  $_.CommandLine -match '@anthropic-ai[\\/]claude-code[\\/].*cli\.js'
              }
          } | Select-Object -First 1
        if ($found) { 'running' } else { 'idle' }
    "#;
    let output = hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|_| "无法检查 Claude Code 会话状态")?;
    if !output.status.success() {
        return Err("无法检查 Claude Code 会话状态".into());
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "running" => Ok(true),
        "idle" => Ok(false),
        _ => Err("无法检查 Claude Code 会话状态".into()),
    }
}

#[derive(Serialize)]
pub struct ClientView {
    pub state: &'static str,
    pub account: String,
    pub model: String,
    pub selected: Option<String>,
    pub issue: Option<String>,
    pub can_undo: bool,
    pub can_restore: bool,
    pub pending: Option<PendingView>,
    pub models: Vec<String>,
    pub code_models: Vec<CodeModelView>,
    pub local_gateway: bool,
}

#[derive(Serialize)]
pub struct CodeModelView {
    role: &'static str,
    model: String,
}

#[derive(Serialize)]
pub struct PendingView {
    id: Option<String>,
    issue: Option<String>,
}

impl ClientView {
    pub(crate) fn official() -> Self {
        Self {
            state: "official",
            account: "Claude 官方".into(),
            model: String::new(),
            selected: None,
            issue: None,
            can_undo: false,
            can_restore: false,
            pending: None,
            models: Vec::new(),
            code_models: Vec::new(),
            local_gateway: false,
        }
    }
    fn unavailable(error: String) -> Self {
        Self {
            state: "unavailable",
            issue: Some(error),
            ..Self::official()
        }
    }
    pub(crate) fn third_party(url: Option<&str>, model: &str) -> Self {
        let host = url
            .and_then(|url| reqwest::Url::parse(url).ok())
            .and_then(|url| url.host_str().map(str::to_owned));
        Self {
            state: "third_party",
            account: host.unwrap_or_else(|| "第三方配置".into()),
            model: model
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect(),
            ..Self::official()
        }
    }
    pub(crate) fn applied(&mut self, client: Client, store: &Store, applied: Option<&Applied>) {
        let Some(applied) = applied else { return };
        if let Ok((model, account)) = store.model(&applied.id) {
            self.state = "lite";
            self.account = account.name.clone();
            self.selected = Some(model.id.clone());
            if applied.route != models::client_route(client, model, account) {
                self.issue = Some("档案已更新，重新应用后生效".into());
            }
        }
    }
}

fn value<'a>(plan: &'a Plan, section: Option<&str>, key: &str) -> Result<Option<&'a str>, String> {
    Ok(plan.get(section, key)?.and_then(Value::as_str))
}

fn valid_library_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub(crate) fn desktop_gateway_active(paths: &DesktopFiles) -> Result<bool, String> {
    if desktop_managed() {
        return Ok(false);
    }
    let gateway = Plan::open(&paths.gateway)?;
    let native = Plan::open(&paths.native)?;
    Ok(
        value(&gateway, None, "deploymentMode")?.or(value(&native, None, "deploymentMode")?)
            != Some("1p"),
    )
}

fn desktop_view(store: &Store, paths: &DesktopFiles) -> Result<ClientView, String> {
    if desktop_managed() {
        return Ok(ClientView {
            state: "managed",
            account: "组织管理".into(),
            issue: Some("Claude Desktop 的部署配置由组织管理".into()),
            ..ClientView::official()
        });
    }
    let native = Plan::open(&paths.native)?;
    let gateway = Plan::open(&paths.gateway)?;
    let meta = Plan::open(&paths.meta)?;
    let applied = value(&meta, None, "appliedId")?.filter(|id| valid_library_id(id));
    let hybrid = meta
        .get(None, "hybridPointer")?
        .is_some_and(|value| !value.is_null());
    let mode = value(&gateway, None, "deploymentMode")?.or(value(&native, None, "deploymentMode")?);
    if mode == Some("1p") || (mode != Some("3p") && applied.is_none() && !hybrid) {
        return Ok(ClientView::official());
    }
    let Some(id) = applied else {
        return Ok(ClientView::third_party(None, ""));
    };
    let path = paths.meta.with_file_name(format!("{id}.json"));
    let config = Plan::open(&path)?;
    let entries: Vec<String> = config
        .get(None, "inferenceModels")?
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| {
            v.get("labelOverride")
                .and_then(Value::as_str)
                .or_else(|| v.get("name").and_then(Value::as_str))
                .or_else(|| v.as_str())
        })
        .map(|id| id.chars().filter(|ch| !ch.is_control()).take(256).collect())
        .collect();
    let base = value(&config, None, "inferenceGatewayBaseUrl")?;
    let mut view = ClientView::third_party(base, entries.first().map(String::as_str).unwrap_or(""));
    view.models = entries;
    if id == OWNED {
        view.applied(Client::Desktop, store, store.desktop.as_ref());
        let port = store
            .desktop
            .as_ref()
            .and_then(|applied| applied.gateway.as_ref())
            .map_or(gateway::DEFAULT_PORT, desktop_models::Selection::port);
        view.local_gateway = base.and_then(gateway::desktop_port) == Some(port);
        if view.issue.is_none()
            && !view.local_gateway
            && store
                .desktop
                .as_ref()
                .and_then(|applied| store.model(&applied.id).ok())
                .is_some_and(|(model, _)| !desktop_models::supports_direct(model))
        {
            view.issue = Some("重新应用档案，Claude Desktop 将通过 Lite 网关加载模型列表。".into());
        }
    }
    Ok(view)
}

fn is_third_party(url: Option<&str>) -> bool {
    url.filter(|s| !s.is_empty()).is_some_and(|s| {
        reqwest::Url::parse(s).map_or(true, |url| url.host_str() != Some("api.anthropic.com"))
    })
}

fn code_view(root: &Path, store: &Store, path: &Path) -> Result<ClientView, String> {
    let plan = Plan::open(path)?;
    let base = value(&plan, Some("env"), "ANTHROPIC_BASE_URL")?;
    if !is_third_party(base) && store.code.is_none() {
        return Ok(ClientView::official());
    }
    let active = value(&plan, Some("env"), "ANTHROPIC_MODEL")?.unwrap_or("");
    let mut view = ClientView::third_party(base, active);
    for (role, name) in [
        ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
        ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
        ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        ("fable", "ANTHROPIC_DEFAULT_FABLE_MODEL"),
        ("fast", "ANTHROPIC_SMALL_FAST_MODEL"),
        ("subagent", "CLAUDE_CODE_SUBAGENT_MODEL"),
        ("reasoning", "ANTHROPIC_REASONING_MODEL"),
    ] {
        if let Some(model) =
            value(&plan, Some("env"), name)?.filter(|id| !id.is_empty() && *id != active)
        {
            view.code_models.push(CodeModelView {
                role,
                model: model
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(256)
                    .collect(),
            });
        }
    }
    if let Some(applied) = &store.code {
        if let Ok((model, account)) = store.model(&applied.id) {
            if base == Some(account.base_for(models::Protocol::Anthropic))
                && active == models::code_model(model, account)
            {
                let credential_matches =
                    if applied.route == models::client_route(Client::Code, model, account) {
                        let field =
                            if account.auth_for(models::Protocol::Anthropic) == AuthMode::ApiKey {
                                "ANTHROPIC_API_KEY"
                            } else {
                                "ANTHROPIC_AUTH_TOKEN"
                            };
                        value(&plan, Some("env"), field)?.is_some_and(|current| {
                            models::key(root, account).is_ok_and(|key| key == current)
                        })
                    } else {
                        true
                    };
                if credential_matches {
                    view.applied(Client::Code, store, Some(applied));
                    if view.issue.is_none()
                        && value(&plan, Some("env"), CODE_MODEL_OPTION)? != Some(active)
                    {
                        view.issue =
                            Some("重新应用档案即可在 Claude Code 的 /model 中显示该模型。".into());
                    }
                }
            }
        }
    }
    if view.state == "third_party" && !is_third_party(base) {
        Ok(ClientView::official())
    } else {
        Ok(view)
    }
}

#[derive(Serialize)]
struct AccountView<'a> {
    id: &'a str,
    preset: &'a str,
    provider: &'a str,
    name: &'a str,
    base_url: &'a str,
    auth: AuthMode,
    protocol: models::Protocol,
    has_key: bool,
    clients: Vec<Client>,
}

pub fn view(root: &Path) -> Result<Value, String> {
    let (_, store) = models::read(root)?;
    let clients: Vec<_> = Client::ALL
        .into_iter()
        .map(|client| {
            let mut status = match client {
                Client::Desktop => desktop::locations()
                    .and_then(|locations| desktop_view(&store, &DesktopFiles::new(locations))),
                Client::Code => code_path().and_then(|path| code_view(root, &store, &path)),
                Client::Codex => codex::path().and_then(|path| codex::view(&store, &path)),
                Client::Workbuddy => workbuddy::path().and_then(|path| workbuddy::view(&path)),
                _ => crate::agents::view(root, &store, client),
            }
            .unwrap_or_else(ClientView::unavailable);
            status.can_undo = transaction::can_undo(root, client);
            if client == Client::Code {
                status.pending = store.pending_code.as_ref().map(|pending| PendingView {
                    id: pending.id.clone(),
                    issue: pending.issue.clone(),
                });
            }
            let spec = crate::agents::spec(client);
            let configured = store.agents.get(&client).map(|selection| {
                selection.choices.iter().map(|choice| json!({"id":choice.id,"context":choice.context})).collect::<Vec<_>>()
            });
            json!({"id": client, "name": client.name(), "protocol": spec.protocols.first(),
            "protocols": spec.protocols, "agent": crate::agents::is_added(client),
            "models_config": configured, "note": spec.note, "mcp_note": spec.mcp_note, "help": spec.help,
            "install_help": spec.install.is_some(),
            "transports": spec.transports, "mcp_toggle": spec.toggle,
            "mcp_scopes": crate::mcp::scopes(client), "mcp_fields": crate::mcp::options::fields(client), "status": status})
        })
        .collect();
    let keyed_accounts: HashMap<_, _> = store
        .accounts
        .iter()
        .filter(|account| models::has_readable_key(root, account))
        .map(|account| (account.id.as_str(), account))
        .collect();
    let accounts: Vec<_> = store
        .accounts
        .iter()
        .map(|a| AccountView {
            id: &a.id,
            preset: &a.preset,
            provider: models::provider_id(&a.preset),
            name: &a.name,
            base_url: &a.base_url,
            auth: a.auth,
            protocol: a.protocol,
            has_key: keyed_accounts.contains_key(a.id.as_str()),
            clients: a.clients(),
        })
        .collect();
    let saved: Vec<_> = store
        .models
        .iter()
        .filter_map(|model| {
            let account = keyed_accounts.get(model.account.as_str())?;
            let clients: Vec<_> = Client::ALL
                .into_iter()
                .filter(|client| models::supports_model(*client, model, account))
                .collect();
            Some(json!({"id": model.id, "account": model.account, "name": model.name,
            "model": model.model, "checked": model.checked, "clients": clients,
            "desktop_models": model.desktop_models, "code_models": model.code_models,
            "desktop_direct": clients.contains(&Client::Desktop) && desktop_models::supports_direct(model)}))
        })
        .collect();
    Ok(
        json!({"clients": clients, "presets": models::presets(), "accounts": accounts,
        "models": saved, "in_use": store.in_use().collect::<Vec<_>>() }),
    )
}

fn selection(store: &Store, client: Client, id: Option<&str>) -> Result<Option<Applied>, String> {
    id.map(|id| {
        store.model(id).map(|(model, account)| Applied {
            id: id.into(),
            route: models::client_route(client, model, account),
            gateway: None,
            codex_gateway: None,
        })
    })
    .transpose()
}

fn desktop_connection(
    store: &Store,
    model: &models::Model,
    connection: Option<Connection>,
) -> Connection {
    connection.unwrap_or_else(|| {
        if !model.desktop_models.is_empty()
            || !desktop_models::supports_direct(model)
            || store
                .desktop
                .as_ref()
                .is_some_and(|applied| applied.gateway.is_some())
        {
            Connection::Gateway
        } else {
            Connection::Direct
        }
    })
}

fn plan_desktop(
    root: &Path,
    paths: &DesktopFiles,
    store: &Store,
    id: Option<&str>,
    port: Option<u16>,
    effort_catalog: Option<crate::desktop_effort::Catalog>,
) -> Result<Vec<Delta>, String> {
    let mut changes = Vec::new();
    let mut meta = Plan::open(&paths.meta)?;
    let mut applied = selection(store, Client::Desktop, id)?;
    if let Some(id) = id {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Desktop, model, account)?;
        let (config, gateway) = if let Some(port) = port {
            let (config, gateway) = desktop_models::configuration(
                root,
                paths,
                store.desktop.as_ref(),
                account,
                model,
                value(&meta, None, "appliedId")?,
                port,
                effort_catalog,
            )?;
            (config, Some(gateway))
        } else {
            let profile = models::desktop_profile(model, account);
            let key = models::key(root, account)?;
            (
                desktop_models::direct_configuration(&profile, &key)?,
                None,
            )
        };
        if let Some(applied) = &mut applied {
            applied.gateway = gateway;
        }
        let mut owned = Plan::open(&paths.owned)?;
        for (field, value) in config.as_object().ok_or("Desktop 模型配置无效")? {
            owned.set(None, field, Some(value.clone()))?;
        }
        owned.set(None, "deploymentDisplayName", Some(json!("CodeNeo Lite")))?;
        owned.set(None, "endUserAttribution", Some(json!(false)))?;
        changes.extend(owned.finish());
        let mut entries = match meta.get(None, "entries")? {
            Some(entries) => entries
                .as_array()
                .ok_or("Claude Desktop 配置库条目格式无效")?
                .clone(),
            None => Vec::new(),
        };
        match entries.iter_mut().find(|entry| entry["id"] == OWNED) {
            Some(entry)
                if matches!(
                    entry["name"].as_str(),
                    Some("ClaudeKeeper Lite" | "codeneo-lite")
                ) =>
            {
                entry["name"] = json!("CodeNeo Lite")
            }
            Some(_) => {}
            None => entries.push(json!({"id": OWNED, "name": "CodeNeo Lite"})),
        }
        meta.set(None, "entries", Some(Value::Array(entries)))?;
        meta.set(None, "appliedId", Some(json!(OWNED)))?;
        meta.set(None, "hybridPointer", Some(Value::Null))?;
    } else if paths.meta.is_file() {
        meta.set(None, "appliedId", Some(Value::Null))?;
        meta.set(None, "hybridPointer", Some(Value::Null))?;
    }
    changes.extend(meta.finish());
    let mode = if id.is_some() { "3p" } else { "1p" };
    let mut native = Plan::open(&paths.native)?;
    native.set(None, "deploymentMode", Some(json!(mode)))?;
    changes.extend(native.finish());
    if paths.gateway != paths.native {
        let mut gateway = Plan::open(&paths.gateway)?;
        gateway.set(None, "deploymentMode", Some(json!(mode)))?;
        changes.extend(gateway.finish());
    }
    let mut state = Plan::open(&models::path(root))?;
    state.set(None, "desktop", Some(json!(applied)))?;
    changes.extend(state.finish());
    Ok(changes)
}

fn code_fingerprint(path: &Path) -> Result<String, String> {
    let plan = Plan::open(path)?;
    let values: Result<Vec<_>, _> = CODE_KEYS
        .iter()
        .map(|key| plan.get(Some("env"), key).map(|value| value.cloned()))
        .collect();
    Ok(format!(
        "{:x}",
        Sha256::digest(transaction::json_bytes(&values?)?)
    ))
}

#[derive(Deserialize, Serialize)]
struct OfficialCode {
    path: PathBuf,
    fields: serde_json::Map<String, Value>,
}

fn code_official_path(root: &Path) -> PathBuf {
    root.join("official-code.bin")
}

fn capture_official(
    root: &Path,
    path: &Path,
    store: &Store,
    id: Option<&str>,
) -> Result<(), String> {
    if id.is_none() || store.code.is_some() {
        return Ok(());
    }
    let code = Plan::open(path)?;
    if is_third_party(value(&code, Some("env"), "ANTHROPIC_BASE_URL")?) {
        return Ok(());
    }
    let mut fields = serde_json::Map::new();
    for name in CODE_KEYS {
        if let Some(value) = code.get(Some("env"), name)? {
            fields.insert((*name).into(), value.clone());
        }
    }
    let snapshot = OfficialCode {
        path: path.to_path_buf(),
        fields,
    };
    vault::write(
        root,
        &code_official_path(root),
        &transaction::json_bytes(&snapshot)?,
    )
}

fn official_fields(root: &Path, path: &Path) -> Result<serde_json::Map<String, Value>, String> {
    let Some(bytes) = vault::read(root, &code_official_path(root))? else {
        return Ok(serde_json::Map::new());
    };
    let saved: OfficialCode =
        serde_json::from_slice(&bytes).map_err(|_| "Claude 官方配置的恢复记录无效")?;
    Ok(if saved.path == path {
        saved.fields
    } else {
        serde_json::Map::new()
    })
}

fn plan_code(
    root: &Path,
    path: &Path,
    store: &Store,
    id: Option<&str>,
) -> Result<Vec<Delta>, String> {
    let mut code = Plan::open(path)?;
    let mut desired = serde_json::Map::new();
    if let Some(id) = id {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Code, model, account)?;
        let key = models::key(root, account)?;
        let profile = models::code_profile(model, account);
        let apply = ProfileApply {
            secret: Some(key),
            proxy_http: None,
            confirmed: true,
        };
        desired = profile_env(&profile, &apply)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|(field, value)| (field, json!(value)))
            .collect();
        // Blank out the unused inherited credential, as Lite did for old profiles.
        let other = if profile.auth == AuthMode::ApiKey {
            "ANTHROPIC_AUTH_TOKEN"
        } else {
            "ANTHROPIC_API_KEY"
        };
        desired.insert(other.into(), json!(""));
        desired.insert(CODE_MODEL_OPTION.into(), json!(profile.model));
        desired.insert(CODE_MODEL_OPTION_NAME.into(), json!(model.name));
        if account.preset == "openrouter" {
            desired.insert(
                "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
                json!("1"),
            );
        }
    }
    if id.is_none() && store.code.is_some() {
        desired = official_fields(root, path)?;
    }
    if id.is_some()
        || store.code.is_some()
        || is_third_party(value(&code, Some("env"), "ANTHROPIC_BASE_URL")?)
    {
        for key in CODE_KEYS {
            if id.is_none()
                && store.code.is_none()
                && matches!(*key, CODE_MODEL_OPTION | CODE_MODEL_OPTION_NAME)
            {
                continue;
            }
            code.set(Some("env"), key, desired.remove(*key))?;
        }
    }
    let mut state = Plan::open(&models::path(root))?;
    state.set(
        None,
        "code",
        Some(json!(selection(store, Client::Code, id)?)),
    )?;
    Ok([code.finish(), state.finish()]
        .into_iter()
        .flatten()
        .collect())
}

#[derive(Serialize)]
pub struct Outcome {
    pub message: std::borrow::Cow<'static, str>,
}

pub(crate) struct DesktopChange {
    deltas: Vec<Delta>,
    paths: Vec<PathBuf>,
    pub(crate) reload: bool,
}

fn desktop_requires_reload(
    deltas: &[Delta],
    state: &Path,
    owned: &Path,
    same_gateway: bool,
) -> bool {
    deltas.iter().any(|delta| {
        !(delta.at(state) || (same_gateway && delta.edits_only(owned, "inferenceModels")))
    })
}

impl DesktopChange {
    pub(crate) async fn prepare(
        root: &Path,
        id: Option<&str>,
        connection: Option<Connection>,
    ) -> Result<Self, String> {
        if desktop_managed() {
            return Err("Claude Desktop 的部署配置由组织管理".into());
        }
        let (_, store) = models::read(root)?;
        let locations = DesktopFiles::new(desktop::locations()?);
        let previous = store
            .desktop
            .as_ref()
            .and_then(|applied| applied.gateway.as_ref())
            .map(desktop_models::Selection::port);
        let port = if let Some(id) = id {
            let (model, account) = store.model(id)?;
            models::require_model(Client::Desktop, model, account)?;
            if connection == Some(Connection::Direct) && !desktop_models::supports_direct(model) {
                return Err(
                    "Claude Desktop 的直连模型列表需要 Claude 模型 ID。请选择 CodeNeo Lite网关。"
                        .into(),
                );
            }
            if desktop_connection(&store, model, connection) == Connection::Gateway {
                Some(gateway::prepare(&mut *gateway::SERVICE.lock().await, root, previous).await?)
            } else {
                None
            }
        } else {
            None
        };
        let effort_catalog = if let Some(id) = id.filter(|_| port.is_some()) {
            let (model, account) = store.model(id)?;
            desktop_models::discover_efforts(root, account, model).await?
        } else {
            None
        };
        let deltas = plan_desktop(root, &locations, &store, id, port, effort_catalog)?;
        let state = models::path(root);
        Ok(Self {
            reload: desktop_requires_reload(
                &deltas,
                &state,
                &locations.owned,
                port.is_some() && port == previous,
            ),
            deltas,
            paths: allowed(root),
        })
    }

    pub(crate) fn commit(self, root: &Path) -> Result<(), String> {
        if desktop_managed() {
            return Err("Claude Desktop 的部署配置由组织管理".into());
        }
        transaction::commit(root, Client::Desktop, self.deltas, &self.paths)
    }
}

pub async fn switch(client: Client, id: Option<String>) -> Result<Outcome, String> {
    if client == Client::Desktop {
        let message = crate::launch::apply_desktop(id.as_deref(), None, None).await?;
        return Ok(Outcome {
            message: message.into(),
        });
    }
    if client == Client::Codex {
        let root = crate::secrets::root()?;
        let target = id.as_deref().map_or(codex::Target::Restore, |id| {
            codex::Target::Profile(id, None)
        });
        return codex::switch(&root, target).await;
    }
    tauri::async_runtime::spawn_blocking(move || {
        switch_client(&crate::secrets::root()?, client, id.as_deref())
    })
    .await
    .map_err(|_| "Lite 无法完成本机配置操作")?
}

fn switch_client(root: &Path, client: Client, id: Option<&str>) -> Result<Outcome, String> {
    let _lock = files::lock(root)?;
    let paths = allowed(root);
    transaction::recover(root, &paths)?;
    let (before, mut store) = models::read(root)?;
    match client {
        Client::Codex => Err("请使用 Codex 的模型切换入口".into()),
        Client::Workbuddy => workbuddy::add(
            root,
            &workbuddy::path()?,
            &store,
            id.ok_or("请在 WorkBuddy 对话的模型菜单里切回内置模型")?,
            &paths,
        ),
        Client::Desktop => Err("请使用 Desktop 的模型应用入口".into()),
        Client::Code => {
            let path = code_path()?;
            let deltas = plan_code(root, &path, &store, id)?;
            if code_running()? {
                store.pending_code = Some(Pending {
                    id: id.map(str::to_owned),
                    route: selection(&store, Client::Code, id)?.map(|applied| applied.route),
                    config: code_fingerprint(&path)?,
                    issue: None,
                });
                models::write(root, before.as_deref(), &store)?;
                return Ok(Outcome {
                    message: "Claude Code 正在运行，已等待会话退出后应用。".into(),
                });
            }
            capture_official(root, &path, &store, id)?;
            transaction::commit(root, client, deltas, &paths)?;
            clear_pending(root)?;
            Ok(Outcome {
                message: "Code 配置已写入。请在新会话用 /status 查看。".into(),
            })
        }
        _ => {
            let mut choices = id
                .map(|id| crate::agents::RequestedModel {
                    id: id.to_owned(),
                    context: 32_768,
                })
                .into_iter()
                .collect::<Vec<_>>();
            if let (Some(id), Some(active)) = (id, store.agents.get(&client)) {
                if let Some(previous) = active.choices.iter().find(|choice| choice.id == id) {
                    choices[0].context = previous.context;
                }
                choices.extend(active.choices.iter().filter(|choice| choice.id != id).map(
                    |choice| crate::agents::RequestedModel {
                        id: choice.id.clone(),
                        context: choice.context,
                    },
                ));
            }
            crate::agents::apply(root, client, &store, choices)
        }
    }
}

pub fn apply_workbuddy_models(root: &Path, ids: Vec<String>) -> Result<Outcome, String> {
    if ids.is_empty() || ids.len() > 300 {
        return Err("请选择有效的 WorkBuddy 模型列表".into());
    }
    let _lock = files::lock(root)?;
    let paths = allowed(root);
    transaction::recover(root, &paths)?;
    let (_, store) = models::read(root)?;
    let ids: Vec<_> = ids.iter().map(String::as_str).collect();
    workbuddy::add_many(root, &workbuddy::path()?, &store, &ids, &paths)
}

pub fn apply_agent_models(
    root: &Path,
    client: Client,
    models: Vec<crate::agents::RequestedModel>,
) -> Result<Outcome, String> {
    let _lock = files::lock(root)?;
    transaction::recover(root, &allowed(root))?;
    crate::agents::apply(root, client, &models::read(root)?.1, models)
}

fn clear_pending(root: &Path) -> Result<(), String> {
    let (before, mut store) = models::read(root)?;
    if store.pending_code.take().is_some() {
        models::write(root, before.as_deref(), &store)?;
    }
    Ok(())
}

pub fn cancel_pending(root: &Path) -> Result<(), String> {
    let _lock = files::lock(root)?;
    clear_pending(root)
}

fn check_launch(root: &Path, client: Client) -> Result<(), String> {
    if client == Client::Code {
        if let Some(pending) = models::read(root)?.1.pending_code {
            return Err(pending.issue.unwrap_or_else(|| {
                "Claude Code 有待应用的配置。请退出旧会话，等待切换完成后再打开终端。".into()
            }));
        }
    }
    Ok(())
}

pub(crate) fn lock_for_launch(root: &Path, client: Client) -> Result<std::fs::File, String> {
    let lock = files::lock(root)?;
    transaction::recover(root, &allowed(root))?;
    if client == Client::Desktop {
        crate::mcp::policy::recover(root)?;
    }
    check_launch(root, client)?;
    Ok(lock)
}

pub fn process_pending(root: &Path) -> Result<Option<Result<String, String>>, String> {
    let (_, store) = models::read(root)?;
    if !store
        .pending_code
        .as_ref()
        .is_some_and(|pending| pending.issue.is_none())
    {
        return Ok(None);
    }
    let running = code_running();
    if running == Ok(true) {
        return Ok(None);
    }
    let _lock = files::lock(root)?;
    let paths = allowed(root);
    transaction::recover(root, &paths)?;
    let (before, mut store) = models::read(root)?;
    let Some(pending) = store.pending_code.as_ref().filter(|p| p.issue.is_none()) else {
        return Ok(None);
    };
    // A session can start between the optimistic check and taking the lock.
    let running = code_running();
    if running == Ok(true) {
        return Ok(None);
    }
    let path = code_path()?;
    let result = (|| {
        running?;
        let latest_route =
            selection(&store, Client::Code, pending.id.as_deref())?.map(|applied| applied.route);
        if latest_route != pending.route || code_fingerprint(&path)? != pending.config {
            return Err("等待期间模型档案或 Code 配置已改变，请重新应用".into());
        }
        let deltas = plan_code(root, &path, &store, pending.id.as_deref())?;
        capture_official(root, &path, &store, pending.id.as_deref())?;
        transaction::commit(root, Client::Code, deltas, &paths)
    })();
    match result {
        Ok(()) => {
            clear_pending(root)?;
            Ok(Some(Ok("Code 已应用等待中的配置，新会话可使用。".into())))
        }
        Err(error) => {
            store
                .pending_code
                .as_mut()
                .expect("pending checked above")
                .issue = Some(error.clone());
            models::write(root, before.as_deref(), &store)?;
            Ok(Some(Err(error)))
        }
    }
}

pub async fn undo(client: Client) -> Result<Outcome, String> {
    if matches!(client, Client::Desktop | Client::Codex) {
        let root = crate::secrets::root()?;
        let _lock = lock_for_launch(&root, client)?;
        if client == Client::Desktop && desktop_managed() {
            return Err("Claude Desktop 的部署配置由组织管理".into());
        }
        let paths = allowed(&root);
        let plan = transaction::prepare_undo(&root, client, &paths)?;
        if let Some(preferred) = plan.iter().find_map(|delta| {
            transaction::restored_gateway_port(delta, &models::path(&root), client)
        }) {
            if client == Client::Desktop {
                gateway::desktop_key(&root, false)?;
            }
            let port =
                gateway::prepare(&mut *gateway::SERVICE.lock().await, &root, Some(preferred))
                    .await?;
            if port != preferred {
                return Err(format!("撤销记录中的网关使用端口 {preferred}，当前网关使用端口 {port}。请从模型库应用需要恢复的档案。"));
            }
        }
        if client == Client::Desktop && desktop_managed() {
            return Err("Claude Desktop 的部署配置由组织管理".into());
        }
        transaction::commit(&root, client, plan, &paths)?;
        let message = if client == Client::Desktop {
            desktop_models::sync(&root).await?;
            "已撤销上次 Desktop 切换。重新打开 Claude Desktop 后生效。"
        } else {
            codex::sync_gateway(&root).await?;
            "已撤销上次 Codex 切换。重新打开 Codex 后生效。"
        };
        return Ok(Outcome {
            message: message.into(),
        });
    }
    tauri::async_runtime::spawn_blocking(move || undo_client(&crate::secrets::root()?, client))
        .await
        .map_err(|_| "Lite 无法完成本机配置操作")?
}

fn undo_client(root: &Path, client: Client) -> Result<Outcome, String> {
    let _lock = files::lock(root)?;
    if client == Client::Code && code_running()? {
        return Err("请先结束正在运行的 Claude Code 会话，再撤销切换".into());
    }
    let paths = allowed(root);
    transaction::recover(root, &paths)?;
    transaction::undo(root, client, &paths)?;
    if client == Client::Code {
        clear_pending(root)?;
    }
    Ok(Outcome {
        message: match client {
            Client::Desktop | Client::Codex => return Err("请使用客户端的模型切换入口撤销".into()),
            Client::Code => "已撤销上次 Code 切换。请在新会话用 /status 查看。",
            Client::Workbuddy => "已撤销上次 WorkBuddy 模型配置。请在对话的模型菜单中查看。",
            _ => "已撤销上次模型配置。请在 Agent 的新会话中查看。",
        }
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn pending_code_configuration_blocks_a_new_code_terminal() {
        let root = tempfile::tempdir().unwrap();
        let _lock = files::lock(root.path()).unwrap();
        let store = Store {
            pending_code: Some(Pending {
                id: None,
                route: None,
                config: "fixture".into(),
                issue: None,
            }),
            ..Store::default()
        };
        models::write(root.path(), None, &store).unwrap();
        assert!(check_launch(root.path(), Client::Code)
            .unwrap_err()
            .contains("待应用"));
        assert!(check_launch(root.path(), Client::Codex).is_ok());
        clear_pending(root.path()).unwrap();
        assert!(check_launch(root.path(), Client::Code).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn code_process_detection_recognizes_resolved_native_paths() {
        assert!(native_code(
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe"
        ));
        assert!(native_code(
            "/Users/my name/.local/share/claude/versions/2.3"
        ));
        assert!(!native_code(
            "/Applications/Claude.app/Contents/MacOS/Claude"
        ));
        assert!(!native_code("/usr/bin/claude-desktop"));
    }

    fn save_model(root: &Path, model: &str, account: Option<&str>) -> String {
        let input = serde_json::from_value(json!({
            "id": null, "account_id": account, "preset": "deepseek", "account_name": "测试接入",
            "base_url": "", "auth": "auth_token", "name": "", "model": model,
            "key": if account.is_none() { "fixture-supplier-key" } else { "" }
        }))
        .unwrap();
        models::save(root, input, None).unwrap()
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn overview_hides_profiles_until_their_account_has_a_readable_key() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let first = save_model(root, "deepseek-flash", None);
        let mut stored = read(&models::path(root));
        let account = stored["accounts"][0]["id"].as_str().unwrap().to_owned();
        let second = save_model(root, "deepseek-v4-pro", Some(&account));
        let configured = view(root).unwrap();
        assert_eq!(configured["models"].as_array().unwrap().len(), 2);
        assert_eq!(configured["accounts"][0]["has_key"], true);
        assert!(configured["accounts"][0].get("credential").is_none());

        let credential = stored["accounts"][0]["credential"].as_str().unwrap();
        let path = root
            .join("model-credentials")
            .join(format!("{credential}.bin"));
        fs::remove_file(path).unwrap();
        let missing = view(root).unwrap();
        assert_eq!(missing["models"], json!([]));
        assert_eq!(missing["accounts"][0]["has_key"], false);

        stored = read(&models::path(root));
        stored["accounts"][0]
            .as_object_mut()
            .unwrap()
            .remove("credential");
        fs::write(models::path(root), serde_json::to_vec(&stored).unwrap()).unwrap();
        assert_eq!(view(root).unwrap()["models"], json!([]));

        let repaired = models::save(
            root,
            serde_json::from_value(json!({
                "account_id": account, "preset": "deepseek", "account_name": "",
                "base_url": "", "auth": "auth_token", "name": "", "model": "deepseek-flash",
                "key": "replacement-fixture-key"
            }))
            .unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(repaired, first);
        let repaired = view(root).unwrap();
        assert_eq!(repaired["accounts"][0]["has_key"], true);
        assert_eq!(repaired["models"][1]["id"], second);
        assert_eq!(repaired["models"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn code_profiles_share_a_key_and_restore_original_official_fields() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lite");
        let code = temp.path().join("settings.json");
        let original = json!({"env":{"ANTHROPIC_API_KEY":"fixture-official-key","ANTHROPIC_MODEL":"claude-sonnet", "HTTPS_PROXY":"fixture-proxy", (CODE_MODEL_OPTION): "existing-model"}, "hooks":{"keep":[]}});
        fs::write(&code, transaction::json_bytes(&original).unwrap()).unwrap();
        let first = save_model(&root, "deepseek-flash", None);
        let account = models::read(&root).unwrap().1.accounts[0].id.clone();
        let second = save_model(&root, "deepseek-v4-pro", Some(&account));
        let paths = vec![code.clone(), models::path(&root)];
        for id in [&first, &second] {
            let (_, store) = models::read(&root).unwrap();
            assert_eq!(store.accounts.len(), 1);
            capture_official(&root, &code, &store, Some(id)).unwrap();
            let deltas = plan_code(&root, &code, &store, Some(id)).unwrap();
            transaction::commit(&root, Client::Code, deltas, &paths).unwrap();
            let applied = read(&code);
            assert_eq!(
                applied["env"]["ANTHROPIC_AUTH_TOKEN"],
                "fixture-supplier-key"
            );
            assert_eq!(applied["env"]["ANTHROPIC_API_KEY"], "");
            assert_eq!(
                applied["env"][CODE_MODEL_OPTION],
                applied["env"]["ANTHROPIC_MODEL"]
            );
            assert_eq!(
                applied["env"]["HTTPS_PROXY"],
                original["env"]["HTTPS_PROXY"]
            );
            assert_eq!(applied["hooks"], original["hooks"]);
        }
        let (_, store) = models::read(&root).unwrap();
        let deltas = plan_code(&root, &code, &store, None).unwrap();
        transaction::commit(&root, Client::Code, deltas, &paths).unwrap();
        assert_eq!(read(&code), original);
        assert!(models::read(&root).unwrap().1.code.is_none());
        assert!(!fs::read_to_string(models::path(&root))
            .unwrap()
            .contains("fixture-supplier-key"));
    }

    #[test]
    fn desktop_switch_and_undo_preserve_mcp_and_the_external_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lite");
        let id = save_model(&root, "deepseek-flash", None);
        let desktop = DesktopFiles::new(desktop::Locations {
            native: temp.path().join("Claude"),
            gateway: temp.path().join("Claude-3p"),
        });
        fs::create_dir_all(desktop.meta.parent().unwrap()).unwrap();
        fs::create_dir_all(desktop.native.parent().unwrap()).unwrap();
        fs::write(
            &desktop.native,
            br#"{"mcpServers":{"native-helper":{"command":"keep"}}}"#,
        )
        .unwrap();
        fs::write(
            &desktop.gateway,
            br#"{"deploymentMode":"3p","mcpServers":{"frameleap":{"command":"existing"}}}"#,
        )
        .unwrap();
        let external = "68332ab0-c0d7-4212-b30c-bdd813d6ca01";
        let metadata = json!({"appliedId":external,"entries":[{"id":external,"name":"已有档案"}],"hybridPointer":null});
        fs::write(&desktop.meta, transaction::json_bytes(&metadata).unwrap()).unwrap();
        let external_path = desktop.meta.with_file_name(format!("{external}.json"));
        fs::write(&external_path, b"{}").unwrap();
        let (_, store) = models::read(&root).unwrap();
        let mut paths = desktop.paths().to_vec();
        paths.push(models::path(&root));
        let deltas = plan_desktop(&root, &desktop, &store, Some(&id), None, None).unwrap();
        transaction::commit(&root, Client::Desktop, deltas, &paths).unwrap();
        assert_eq!(read(&desktop.meta)["appliedId"], OWNED);
        assert_eq!(
            read(&desktop.owned)["deploymentDisplayName"],
            "CodeNeo Lite"
        );
        assert_eq!(read(&desktop.owned)["endUserAttribution"], false);
        assert_eq!(
            read(&desktop.owned)["inferenceModels"][0]["name"],
            "deepseek-flash"
        );
        assert_eq!(
            read(&desktop.gateway)["mcpServers"]["frameleap"]["command"],
            "existing"
        );
        let mut new_mcp = read(&desktop.gateway);
        new_mcp["mcpServers"]["frameleap"]["command"] = json!("updated-after-switch");
        fs::write(&desktop.gateway, transaction::json_bytes(&new_mcp).unwrap()).unwrap();
        transaction::undo(&root, Client::Desktop, &paths).unwrap();
        assert_eq!(read(&desktop.meta), metadata);
        assert_eq!(read(&desktop.gateway), new_mcp);
        assert_eq!(fs::read(&external_path).unwrap(), b"{}");
        assert!(read(&desktop.native).get("deploymentMode").is_none());
    }

    #[test]
    fn desktop_gateway_can_switch_model_lists_without_reloading() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("lite");
        let paths = DesktopFiles::new(desktop::Locations {
            native: fixture.path().join("Claude"),
            gateway: fixture.path().join("Claude-3p"),
        });
        let first = save_model(&root, "fixture-model", None);
        let second = save_model(&root, "fixture-model", None);
        let third = save_model(&root, "other-model", None);
        let state = models::path(&root);
        let allowed = [paths.paths().to_vec(), vec![state.clone()]].concat();
        let apply = |id: Option<&str>, connection, port| {
            let (_, store) = models::read(&root).unwrap();
            let previous = store
                .desktop
                .as_ref()
                .and_then(|applied| applied.gateway.as_ref())
                .map(desktop_models::Selection::port);
            let port = id.and_then(|id| {
                (desktop_connection(&store, store.model(id).unwrap().0, connection)
                    == Connection::Gateway)
                    .then_some(port)
            });
            let changes = plan_desktop(&root, &paths, &store, id, port, None).unwrap();
            let reload = desktop_requires_reload(
                &changes,
                &state,
                &paths.owned,
                port.is_some() && port == previous,
            );
            transaction::commit(&root, Client::Desktop, changes, &allowed).unwrap();
            reload
        };
        let default = gateway::DEFAULT_PORT;
        assert!(apply(
            Some(first.as_str()),
            Some(Connection::Gateway),
            default
        ));
        let mut config = read(&paths.owned);
        assert_eq!(
            config["inferenceGatewayBaseUrl"],
            gateway::desktop_url(default)
        );
        assert_eq!(
            config["inferenceModels"][0]["labelOverride"],
            "fixture-model"
        );
        config["inferenceGatewayApiKey"] = json!("__sealed__:v1:fixture-token");
        fs::write(&paths.owned, transaction::json_bytes(&config).unwrap()).unwrap();
        assert!(!apply(
            Some(second.as_str()),
            Some(Connection::Gateway),
            default
        ));
        assert_eq!(read(&paths.owned), config);
        assert_eq!(models::read(&root).unwrap().1.desktop.unwrap().id, second);
        assert!(!apply(Some(&third), Some(Connection::Gateway), default));
        let updated = read(&paths.owned);
        assert_eq!(
            updated["inferenceModels"][0]["labelOverride"],
            "other-model"
        );
        assert_eq!(
            updated["inferenceGatewayApiKey"],
            config["inferenceGatewayApiKey"]
        );
        let alternate = 49321;
        assert!(apply(
            Some(second.as_str()),
            Some(Connection::Gateway),
            alternate,
        ));
        let (_, store) = models::read(&root).unwrap();
        assert!(desktop_view(&store, &paths).unwrap().local_gateway);
        assert_eq!(
            desktop_models::labels_for_test(&root, &paths, &store).unwrap(),
            ["fixture-model"]
        );
        let shifted = read(&paths.owned);
        assert_eq!(
            shifted["inferenceGatewayBaseUrl"],
            gateway::desktop_url(alternate)
        );
        assert_eq!(
            shifted["inferenceGatewayApiKey"],
            config["inferenceGatewayApiKey"]
        );
        assert!(apply(
            Some(first.as_str()),
            Some(Connection::Direct),
            default
        ));
        assert_ne!(
            read(&paths.owned)["inferenceGatewayBaseUrl"],
            gateway::desktop_url(alternate)
        );
        assert_eq!(
            read(&paths.owned)["inferenceModels"][0]["name"],
            "fixture-model"
        );
        let undo = transaction::prepare_undo(&root, Client::Desktop, &allowed).unwrap();
        assert_eq!(
            undo.iter()
                .find_map(|delta| transaction::restored_gateway_port(
                    delta,
                    &state,
                    Client::Desktop
                )),
            Some(alternate)
        );
        transaction::commit(&root, Client::Desktop, undo, &allowed).unwrap();
        assert_eq!(read(&paths.owned), shifted);
        assert!(apply(None, None, default));
        assert_eq!(read(&paths.native)["deploymentMode"], "1p");
        assert!(models::read(&root).unwrap().1.desktop.is_none());
    }

    #[test]
    fn desktop_multi_model_selection_survives_restart_edits_and_undo() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lite");
        let paths = DesktopFiles::new(desktop::Locations {
            native: temp.path().join("Claude"),
            gateway: temp.path().join("Claude-3p"),
        });
        let save = |input| {
            models::save(
                &root,
                serde_json::from_value(input).unwrap(),
                Some(Client::Desktop),
            )
        };
        let mut input = json!({
            "id": null, "account_id": null, "preset": "deepseek", "account_name": "Fixture",
            "base_url": "", "auth": "auth_token", "name": "", "model": "",
            "desktop_models": ["deepseek-v4-pro", "deepseek-flash[1m]", "deepseek-flash"], "key": "fixture-supplier-key"
        });
        let id = save(input.clone()).unwrap();
        let (_, store) = models::read(&root).unwrap();
        let files = [paths.paths().to_vec(), vec![models::path(&root)]].concat();
        transaction::commit(
            &root,
            Client::Desktop,
            plan_desktop(
                &root,
                &paths,
                &store,
                Some(&id),
                Some(gateway::DEFAULT_PORT),
                None,
            )
            .unwrap(),
            &files,
        )
        .unwrap();
        let original = read(&paths.owned);
        assert_eq!(
            original["inferenceModels"][0]["labelOverride"],
            "deepseek-v4-pro"
        );
        assert_eq!(
            original["inferenceModels"][1]["labelOverride"],
            "deepseek-flash"
        );
        assert_ne!(
            original["inferenceModels"][0]["name"],
            original["inferenceModels"][1]["name"]
        );
        assert_eq!(original["inferenceModels"].as_array().unwrap().len(), 2);
        assert_ne!(original["inferenceGatewayApiKey"], "fixture-supplier-key");
        let mut sealed = original.clone();
        sealed["inferenceGatewayApiKey"] = json!("__sealed__:v1:fixture-sealed-key");
        fs::write(&paths.owned, transaction::json_bytes(&sealed).unwrap()).unwrap();
        let (_, store) = models::read(&root).unwrap();
        let before = desktop_models::labels_for_test(&root, &paths, &store).unwrap();
        assert_eq!(before, ["deepseek-v4-pro", "deepseek-flash"]);
        input["id"] = json!(id);
        input["account_id"] = json!(store.accounts[0].id);
        input["key"] = json!("");
        input["desktop_models"] = json!(["AUTO"]);
        assert!(save(input.clone()).is_err());
        input["desktop_models"] = json!(["deepseek-flash", "deepseek-v4-pro", "deepseek-reasoner"]);
        save(input).unwrap();
        let (_, store) = models::read(&root).unwrap();
        assert_eq!(
            desktop_models::labels_for_test(&root, &paths, &store).unwrap(),
            before
        );
        assert!(desktop_view(&store, &paths).unwrap().issue.is_some());
        transaction::commit(
            &root,
            Client::Desktop,
            plan_desktop(
                &root,
                &paths,
                &store,
                Some(&id),
                Some(gateway::DEFAULT_PORT),
                None,
            )
            .unwrap(),
            &files,
        )
        .unwrap();
        let (_, store) = models::read(&root).unwrap();
        assert_eq!(
            desktop_models::labels_for_test(&root, &paths, &store).unwrap(),
            ["deepseek-flash", "deepseek-v4-pro", "deepseek-reasoner"]
        );
        transaction::undo(&root, Client::Desktop, &files).unwrap();
        let (_, store) = models::read(&root).unwrap();
        assert_eq!(
            desktop_models::labels_for_test(&root, &paths, &store).unwrap(),
            before
        );
    }

    #[test]
    fn code_model_mappings_reuse_profile_env_and_keep_old_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lite");
        let code = temp.path().join("settings.json");
        let input = serde_json::from_value(json!({
            "id": null, "account_id": null, "preset": "deepseek", "account_name": "Fixture",
            "base_url": "", "auth": "auth_token", "name": "", "model": "deepseek-v4-pro",
            "code_models": {"sonnet": "deepseek-flash", "subagent": "deepseek-flash", "reasoning": "deepseek-reasoner"},
            "key": "fixture-supplier-key"
        })).unwrap();
        let id = models::save(&root, input, Some(Client::Code)).unwrap();
        let (_, store) = models::read(&root).unwrap();
        let deltas = plan_code(&root, &code, &store, Some(&id)).unwrap();
        transaction::commit(
            &root,
            Client::Code,
            deltas,
            &[code.clone(), models::path(&root)],
        )
        .unwrap();
        let env = &read(&code)["env"];
        assert_eq!(env["ANTHROPIC_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "deepseek-v4-pro");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "deepseek-flash[1m]");
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "deepseek-flash[1m]");
        assert_eq!(env["ANTHROPIC_REASONING_MODEL"], "deepseek-reasoner");
    }
}
