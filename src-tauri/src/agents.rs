use crate::{
    clients::{ClientView, Outcome},
    launch,
    mcp::Transport,
    models::{self, Protocol, Store},
    transaction::{self, Client, Field, Plan},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
};

use Client::*;
use Protocol::{Anthropic, ChatCompletions, Responses};
use Transport::{Http, Sse, Stdio};

#[derive(Default)]
pub struct Spec {
    pub cli: Option<&'static str>,
    pub protocols: &'static [Protocol],
    pub transports: &'static [Transport],
    pub toggle: bool,
    pub note: &'static str,
    pub mcp_note: &'static str,
    pub help: Option<&'static str>,
    pub install: Option<&'static str>,
    pub minimum_version: Option<[u32; 3]>,
}

pub fn spec(client: Client) -> Spec {
    const REMOTE_MCP: &[Transport] = &[Http, Stdio, Sse];
    const STREAMABLE_MCP: &[Transport] = &[Http, Stdio];
    const OPENAI: &[Protocol] = &[ChatCompletions, Responses];
    const MULTI_PROTOCOL: &[Protocol] = &[ChatCompletions, Responses, Anthropic];
    match client {
        Desktop => Spec { cli: None, protocols: &[Anthropic], transports: &[Stdio], ..Spec::default() },
        Code => Spec { cli: Some("claude"), protocols: &[Anthropic], transports: REMOTE_MCP, ..Spec::default() },
        Codex => Spec { cli: Some("codex"), protocols: &[Responses], transports: STREAMABLE_MCP, toggle: true, ..Spec::default() },
        Workbuddy => Spec { cli: None, protocols: &[ChatCompletions], transports: REMOTE_MCP, toggle: true, ..Spec::default() },
        Opencode => Spec {
            cli: Some("opencode"),
            protocols: MULTI_PROTOCOL,
            transports: STREAMABLE_MCP,
            toggle: true,
            note: "重新启动 OpenCode，在 /models 中选择已应用的模型。",
            mcp_note: "开启新会话，用 /mcp 查看服务。项目可覆盖同名配置。",
            help: Some("https://opencode.ai/docs/config"),
            install: Some("https://github.com/anomalyco/opencode/releases/tag/v1.19.5"),
            minimum_version: Some([1, 17, 18]),
        },
        Qwen => Spec {
            cli: Some("qwen"),
            protocols: OPENAI,
            transports: REMOTE_MCP,
            note: "重新启动 Qwen Code，在 /model 中切换。应用会选用 OpenAI 兼容认证，恢复原配置时会还原认证设置。",
            mcp_note: "从新会话的 /mcp 查看服务。项目设置可以覆盖用户设置。",
            help: Some("https://qwenlm.github.io/qwen-code-docs/en/users/configuration/model-providers/"),
            install: Some("https://qwenlm.github.io/qwen-code-docs/zh/users/quickstart/"),
            minimum_version: Some([0, 27, 3]),
            ..Spec::default()
        },
        Codebuddy => Spec {
            cli: Some("codebuddy"),
            protocols: OPENAI,
            transports: REMOTE_MCP,
            note: "重新启动 CodeBuddy Code，在 /model 中切换。项目配置可以覆盖用户设置。",
            mcp_note: "开启新会话后用 /mcp 查看服务。项目级服务在项目中生效。",
            help: Some("https://www.codebuddy.cn/docs/cli/models"),
            install: Some("https://www.codebuddy.cn/docs/cli/quickstart"),
            minimum_version: Some([2, 183, 0]),
            ..Spec::default()
        },
        Kimi => Spec {
            cli: Some("kimi"),
            protocols: MULTI_PROTOCOL,
            transports: REMOTE_MCP,
            toggle: true,
            note: "重新启动 Kimi Code CLI，在 /model 中切换。模型上下文上限可在应用时填写。",
            mcp_note: "使用 Kimi Code CLI 的 /mcp 查看连接状态。项目 MCP 可覆盖同名服务。",
            help: Some("https://www.kimi.com/code/docs/kimi-code-cli/configuration/config-files"),
            install: Some("https://www.kimi.com/code/docs/kimi-code-cli/guides/getting-started"),
            minimum_version: Some([2, 2, 0]),
        },
        Copilot => Spec {
            cli: Some("copilot"),
            protocols: MULTI_PROTOCOL,
            transports: REMOTE_MCP,
            note: "重新启动 Copilot CLI，在 /model 中选择已应用的模型。",
            mcp_note: "在 Copilot CLI 输入 /mcp，核对服务器和可用工具。",
            help: Some("https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-llm-models"),
            install: Some("https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli"),
            minimum_version: Some([1, 0, 110]),
            ..Spec::default()
        },
    }
}

pub fn is_added(client: Client) -> bool {
    spec(client).minimum_version.is_some()
}

pub fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "找不到当前用户的主目录".into())
}

pub fn location(variable: &str, fallback: PathBuf) -> Result<PathBuf, String> {
    let path = env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(fallback);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{variable} 必须使用绝对路径"))
    }
}

pub fn first_present<const N: usize>(paths: [PathBuf; N]) -> PathBuf {
    paths
        .iter()
        .find(|path| path.symlink_metadata().is_ok())
        .unwrap_or(&paths[0])
        .clone()
}

fn opencode_global(home: &Path) -> Result<PathBuf, String> {
    let dir = location("XDG_CONFIG_HOME", home.join(".config"))?.join("opencode");
    if env::var_os("OPENCODE_CONFIG_CONTENT").is_some_and(|value| !value.is_empty()) {
        return Err("已通过 OPENCODE_CONFIG_CONTENT 覆盖配置，请在原客户端中管理该设置。".into());
    }
    if env::var_os("OPENCODE_CONFIG").is_some_and(|value| !value.is_empty()) {
        return location("OPENCODE_CONFIG", dir.join("opencode.jsonc"));
    }
    Ok(first_present([
        dir.join("opencode.jsonc"),
        dir.join("opencode.json"),
    ]))
}

pub fn settings(client: Client) -> Result<PathBuf, String> {
    let home = home()?;
    match client {
        Opencode => opencode_global(&home),
        Qwen => Ok(location("QWEN_HOME", home.join(".qwen"))?.join("settings.json")),
        Codebuddy => Ok(codebuddy_home()?.join("settings.json")),
        Kimi => Ok(location("KIMI_CODE_HOME", home.join(".kimi-code"))?.join("config.toml")),
        Copilot => location(
            "COPILOT_PROVIDERS_CONFIG",
            copilot_home()?.join("providers.json"),
        ),
        _ => Err("此客户端通过原生设置选择模型".into()),
    }
}

pub fn copilot_home() -> Result<PathBuf, String> {
    location("COPILOT_HOME", home()?.join(".copilot"))
}

pub fn codebuddy_home() -> Result<PathBuf, String> {
    location("CODEBUDDY_CONFIG_DIR", home()?.join(".codebuddy"))
}

pub fn model_paths(client: Client) -> Result<Vec<PathBuf>, String> {
    let path = settings(client)?;
    let extra = match client {
        Codebuddy => Some(path.with_file_name("models.json")),
        Copilot => Some(copilot_home()?.join("settings.json")),
        _ => None,
    };
    if extra.as_ref() == Some(&path) {
        return Err("Copilot 模型目录与用户设置必须使用不同的文件。".into());
    }
    Ok(std::iter::once(path).chain(extra).collect())
}

pub fn installed_compatible(client: Client, version: Option<&str>) -> bool {
    let Some(minimum) = spec(client).minimum_version else {
        return true;
    };
    let Some(version) = version.filter(|version| !version.contains('-')) else {
        return false;
    };
    let mut parts = version.split('.');
    let mut current = [0; 3];
    for part in &mut current {
        let Some(number) = parts.next().and_then(|part| part.parse().ok()) else {
            return false;
        };
        *part = number;
    }
    current[0] == minimum[0] && current >= minimum
}

pub fn version_issue(client: Client) -> &'static str {
    match client {
        Opencode => "Lite 支持 OpenCode 1.17.18 至 1.x。2.2.7 的用户级 MCP 在本机验证中未生效。",
        Qwen => "Lite 支持 Qwen Code 0.27.3 或更新的 0.x 版本。",
        Codebuddy => "Lite 支持 CodeBuddy Code 2.183.0 或更新的 2.x 版本。",
        Kimi => {
            "此版本的 Kimi CLI 尚未通过 Lite 兼容验证，请使用 Kimi Code CLI 2.2 或更新的 2.x 版本。"
        }
        Copilot => {
            "此版本的 GitHub Copilot CLI 尚未通过 Lite 兼容验证，请使用 1.0.110 或更新的 1.x 版本。"
        }
        _ => "当前版本尚未通过 Lite 兼容验证。",
    }
}

pub fn ensure_compatible(client: Client) -> Result<(), String> {
    if is_added(client) {
        let home = home()?;
        if launch::find_cli(client, &home).is_some() {
            let version = launch::cli_version(client, &home);
            match version.as_deref() {
                None => {
                    return Err(format!(
                        "无法读取 {} 的版本，请在终端确认后刷新。",
                        client.name()
                    ))
                }
                Some(version) if !installed_compatible(client, Some(version)) => {
                    return Err(version_issue(client).into());
                }
                Some(_) => {}
            }
        }
        check_configuration(client).map_err(ConfigurationIssue::message)?;
    }
    Ok(())
}

pub fn check_install(client: Client, version: Option<&str>) -> bool {
    installed_compatible(client, version) && check_configuration(client).is_ok()
}

pub(crate) enum ConfigurationIssue {
    Unsupported,
    Unreadable(String),
}

impl From<String> for ConfigurationIssue {
    fn from(error: String) -> Self {
        Self::Unreadable(error)
    }
}

impl ConfigurationIssue {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Unsupported => "检测到 OpenCode 2.x 配置，请在 OpenCode 中核对当前版本。".into(),
            Self::Unreadable(error) => error,
        }
    }
}

pub(crate) fn check_configuration(client: Client) -> Result<(), ConfigurationIssue> {
    if !is_added(client) {
        return Ok(());
    }
    for path in model_paths(client)? {
        let plan = Plan::open_as(&path, matches!(client, Opencode | Qwen | Codebuddy))?;
        if client == Opencode {
            let v2_mcp = plan
                .get_at(&["mcp"], "servers")?
                .and_then(Value::as_object)
                .is_some_and(|entry| !entry.get("type").is_some_and(Value::is_string));
            if plan.get(None, "providers")?.is_some() || v2_mcp {
                return Err(ConfigurationIssue::Unsupported);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Choice {
    pub id: String,
    pub route: String,
    #[serde(default = "default_context")]
    pub context: u32,
}

fn default_context() -> u32 {
    32_768
}

#[derive(Default, Serialize, Deserialize)]
pub struct Selection {
    pub choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native: Option<String>,
    #[serde(default)]
    persistent: bool,
}

impl Selection {
    fn fields(&self, root: &Path) -> Result<Vec<OwnedField>, String> {
        let Some(native) = &self.native else {
            return Ok(Vec::new());
        };
        let bytes = STANDARD
            .decode(native)
            .map_err(|_| "Agent 原配置记录损坏")?;
        let data = crate::vault::unprotect(root, &bytes)?;
        serde_json::from_slice(&data).map_err(|_| "Agent 原配置记录损坏".into())
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct OwnedField {
    path: PathBuf,
    parent: Vec<String>,
    key: String,
    #[serde(with = "owned_value")]
    before: Option<Value>,
    #[serde(with = "owned_value")]
    after: Option<Value>,
}

mod owned_value {
    use super::{Field, Value};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Value>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Field::from(value.clone()).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Value>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Saved {
            Field(Field),
            Legacy(Option<Value>),
        }
        Ok(match Saved::deserialize(deserializer)? {
            Saved::Field(field) => field.value().cloned(),
            Saved::Legacy(value) => value,
        })
    }
}

impl OwnedField {
    fn is_default(&self, client: Client) -> bool {
        match client {
            Qwen => self.parent == ["model"] && self.key == "name",
            Kimi => self.parent.is_empty() && self.key == "default_model",
            Copilot => {
                self.parent.is_empty() && matches!(self.key.as_str(), "model" | "modelProviderId")
            }
            _ => self.parent.is_empty() && self.key == "model",
        }
    }

    fn changed_by_model_picker(
        &self,
        client: Client,
        value: Option<&Value>,
        models: &[String],
    ) -> bool {
        let Some(value) = value
            .and_then(Value::as_str)
            .filter(|_| self.is_default(client))
        else {
            return false;
        };
        models.iter().any(|model| {
            if model == value {
                return true;
            }
            let Some((provider, id)) = model.split_once('/').filter(|_| client == Copilot) else {
                return false;
            };
            if self.key == "modelProviderId" {
                value == provider || value.strip_prefix("byok:") == Some(provider)
            } else {
                value == id
            }
        })
    }
}

struct Config {
    plan: Plan,
    path: PathBuf,
    owned: Vec<OwnedField>,
}

impl Config {
    fn open(path: PathBuf, jsonc: bool) -> Result<Self, String> {
        Ok(Self {
            plan: Plan::open_as(&path, jsonc)?,
            path,
            owned: Vec::new(),
        })
    }

    fn rewind(
        &mut self,
        client: Client,
        fields: &[OwnedField],
        previous_models: &[String],
    ) -> Result<(), String> {
        for field in fields.iter().rev().filter(|field| field.path == self.path) {
            let parent: Vec<_> = field.parent.iter().map(String::as_str).collect();
            let actual = self.plan.get_at(&parent, &field.key)?;
            if actual == field.after.as_ref()
                || field.changed_by_model_picker(client, actual, previous_models)
            {
                self.plan
                    .set_at(&parent, &field.key, field.before.clone())?;
            } else if actual != field.before.as_ref() && !field.is_default(client) {
                return Err("Agent 配置在外部发生变更，请核对当前配置后再应用或撤销".into());
            }
        }
        Ok(())
    }

    fn put(&mut self, parent: &[&str], key: &str, value: Value) -> Result<(), String> {
        self.edit(parent, key, Some(value))
    }

    fn edit(&mut self, parent: &[&str], key: &str, after: Option<Value>) -> Result<(), String> {
        let before = self.plan.get_at(parent, key)?.cloned();
        self.owned.push(OwnedField {
            path: self.path.clone(),
            parent: parent.iter().map(|part| (*part).into()).collect(),
            key: key.to_owned(),
            before,
            after: after.clone(),
        });
        self.plan.set_at(parent, key, after)
    }

    fn insert(&mut self, parent: &[&str], key: &str, value: Value) -> Result<(), String> {
        if self.plan.get_at(parent, key)?.is_some() {
            return Err("此 Agent 已有同名服务商或模型，请在原客户端核对现有条目".into());
        }
        self.put(parent, key, value)
    }
}

#[derive(Deserialize)]
pub struct RequestedModel {
    pub id: String,
    #[serde(default = "default_context")]
    pub context: u32,
}

struct Route<'a> {
    saved: &'a models::Model,
    account: &'a models::Account,
    model: String,
    protocol: Protocol,
    context: u32,
}

impl Route<'_> {
    fn provider(&self) -> String {
        format!("codeneo-lite-{}", self.account.id)
    }
    fn var(&self) -> String {
        format!("CODENEO_LITE_{}", self.account.id.to_ascii_uppercase())
    }
    fn alias(&self) -> String {
        format!("codeneo-{}", self.saved.id)
    }
    fn endpoint(&self) -> String {
        if self.protocol == Anthropic {
            self.account.base_for(Anthropic).to_owned()
        } else {
            models::openai_endpoint(self.account, self.protocol, "")
        }
    }
    fn selection_id(&self, client: Client) -> String {
        match client {
            Opencode => format!("{}/{}", self.provider(), self.model),
            Kimi => self.alias(),
            Copilot => format!("{}/{}", self.provider(), self.alias()),
            _ => self.model.clone(),
        }
    }
}

fn routes<'a>(
    client: Client,
    store: &'a Store,
    choices: &[Choice],
) -> Result<Vec<Route<'a>>, String> {
    choices
        .iter()
        .map(|choice| {
            let (saved, account) = store.model(&choice.id)?;
            models::require_model(client, saved, account)?;
            if models::route(saved, account) != choice.route {
                return Err("模型或凭据已更新，请在 Lite 重新应用后启动".into());
            }
            route(client, saved, account, choice.context)
        })
        .collect()
}

fn route<'a>(
    client: Client,
    saved: &'a models::Model,
    account: &'a models::Account,
    context: u32,
) -> Result<Route<'a>, String> {
    Ok(Route {
        saved,
        account,
        model: crate::model_transport::provider_api_model(&saved.model)?,
        protocol: account.protocol_for(client),
        context,
    })
}

fn opencode(root: &Path, config: &mut Config, routes: &[Route<'_>]) -> Result<(), String> {
    let mut providers: BTreeMap<String, Value> = BTreeMap::new();
    for route in routes {
        let provider = match providers.entry(route.provider()) {
            std::collections::btree_map::Entry::Occupied(provider) => provider.into_mut(),
            std::collections::btree_map::Entry::Vacant(provider) => {
                let package = match route.protocol {
                    ChatCompletions => "@ai-sdk/openai-compatible",
                    Responses => "@ai-sdk/openai",
                    Anthropic => "@ai-sdk/anthropic",
                };
                provider.insert(json!({
                    "npm": package, "name": route.account.name, "models": {},
                    "options": {"baseURL": route.endpoint(), "apiKey": models::key(root, route.account)?}
                }))
            }
        };
        if provider["models"].get(&route.model).is_some() {
            return Err("同一服务商中包含重复模型 ID".into());
        }
        provider["models"][&route.model] = json!({"name":route.saved.name, "tool_call":true});
    }
    for (name, provider) in providers {
        config.insert(&["provider"], &name, provider)?;
    }
    config.put(&[], "model", json!(routes[0].selection_id(Opencode)))
}

pub(crate) fn array(plan: &Plan, parent: &[&str], key: &str) -> Result<Vec<Value>, String> {
    plan.get_at(parent, key)?
        .filter(|value| !value.is_null())
        .map(|entries| {
            entries
                .as_array()
                .cloned()
                .ok_or_else(|| format!("{key} 配置必须为数组"))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn insert_model(entries: &mut Vec<Value>, model: Value) -> Result<(), String> {
    if entries.iter().any(|entry| entry["id"] == model["id"]) {
        return Err("已存在同名模型 ID。请保留其中一个来源，或在原客户端核对该条目".into());
    }
    entries.push(model);
    Ok(())
}

fn qwen(root: &Path, config: &mut Config, routes: &[Route<'_>]) -> Result<(), String> {
    let mut registered = array(&config.plan, &["modelProviders"], "openai")?;
    let mut selected = BTreeSet::new();
    let mut providers = BTreeSet::new();
    for route in routes {
        if !selected.insert(&route.model) {
            return Err("Qwen Code 的模型列表中包含重复 ID。".into());
        }
        let key_var = route.var();
        if providers.insert(key_var.clone()) {
            config.insert(&["env"], &key_var, json!(models::key(root, route.account)?))?;
        }
        let mut entry = json!({"id":route.model, "name":route.saved.name,
            "baseUrl":route.endpoint(), "envKey":key_var});
        if route.protocol == Responses {
            entry["wireApi"] = json!("responses");
        }
        if let Some(index) = registered
            .iter()
            .position(|model| model["id"] == route.model)
        {
            let existing = &mut registered[index];
            if existing["baseUrl"] != route.endpoint() {
                return Err("Qwen Code 已有同名模型，请在模型库选择其他模型 ID。".into());
            }
            let existing = existing
                .as_object_mut()
                .ok_or("Qwen Code 模型条目格式无效")?;
            existing.remove("wireApi");
            existing.extend(entry.as_object().expect("model fields").clone());
        } else {
            insert_model(&mut registered, entry)?;
        }
    }
    config.put(&["modelProviders"], "openai", json!(registered))?;
    config.put(&["security", "auth"], "selectedType", json!("openai"))?;
    config.put(&["model"], "name", json!(routes[0].model))
}

fn codebuddy(root: &Path, config: &mut Config, routes: &[Route<'_>]) -> Result<(), String> {
    let mut registered = array(&config.plan, &[], "models")?;
    let mut visible = array(&config.plan, &[], "availableModels")?;
    let limited = !visible.is_empty();
    for route in routes {
        let responses = route.protocol == Responses;
        let url = if responses {
            route.endpoint()
        } else {
            models::openai_endpoint(route.account, ChatCompletions, "/chat/completions")
        };
        let mut entry = crate::workbuddy::model_fields(
            &route.model,
            &route.saved.name,
            &url,
            &models::key(root, route.account)?,
        );
        entry["supportsToolCall"] = json!(true);
        if responses {
            entry["api"] = json!("openai-responses");
        }
        insert_model(&mut registered, entry)?;
        if limited && !visible.iter().any(|id| id == &route.model) {
            visible.push(json!(route.model));
        }
    }
    config.put(&[], "models", json!(registered))?;
    if limited {
        config.put(&[], "availableModels", json!(visible))?;
    }
    Ok(())
}

fn kimi(root: &Path, config: &mut Config, routes: &[Route<'_>]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for route in routes {
        let provider = route.provider();
        if seen.insert(provider.clone()) {
            let kind = match route.protocol {
                ChatCompletions => "openai",
                Responses => "openai_responses",
                Anthropic => "anthropic",
            };
            config.insert(
                &["providers"],
                &provider,
                json!({
                    "type":kind, "base_url":route.endpoint(), "api_key":models::key(root, route.account)?
                }),
            )?;
        }
        config.insert(
            &["models"],
            &route.alias(),
            json!({
                "provider":provider, "model":route.model, "max_context_size":route.context,
                "display_name":route.saved.name, "capabilities":["tool_use"]
            }),
        )?;
    }
    config.put(&[], "default_model", json!(routes[0].alias()))
}

fn copilot(root: &Path, config: &mut Config, routes: &[Route<'_>]) -> Result<(), String> {
    let mut providers = array(&config.plan, &[], "providers")?;
    let mut registered = array(&config.plan, &[], "models")?;
    let mut seen = BTreeSet::new();
    for route in routes {
        let name = route.provider();
        if seen.insert(name.clone()) {
            if providers.iter().any(|provider| provider["name"] == name) {
                return Err("Copilot 已存在同名服务商".into());
            }
            let kind = if route.protocol == Anthropic {
                "anthropic"
            } else {
                "openai"
            };
            let mut provider = json!({"name":name, "type":kind, "baseUrl":route.endpoint(),
                "apiKey":models::key(root, route.account)?});
            if route.protocol == Responses {
                provider["wireApi"] = json!("responses");
            }
            providers.push(provider);
        }
        let id = route.alias();
        if registered
            .iter()
            .any(|model| model["provider"] == name && model["id"] == id)
        {
            return Err("Copilot 已有同名模型，请核对用户配置".into());
        }
        registered.push(json!({
            "id":id, "name":route.saved.name, "provider":name, "wireModel":route.model, "modelId":route.model
        }));
    }
    config.put(&[], "providers", json!(providers))?;
    config.put(&[], "models", json!(registered))
}

fn prepare_configs(
    root: &Path,
    client: Client,
    routes: &[Route<'_>],
    previous: &[OwnedField],
    previous_models: &[String],
    paths: Vec<PathBuf>,
) -> Result<Vec<Config>, String> {
    let mut configs: Vec<_> = paths
        .into_iter()
        .map(|path| Config::open(path, matches!(client, Opencode | Qwen | Codebuddy)))
        .collect::<Result<_, _>>()?;
    if previous
        .iter()
        .any(|field| !configs.iter().any(|config| config.path == field.path))
    {
        return Err("Agent 配置路径已改变，请切回原档案后再应用或恢复".into());
    }
    if configs.is_empty() {
        return Err("找不到 Agent 的用户配置路径".into());
    }
    for config in &mut configs {
        config.rewind(client, previous, previous_models)?;
    }
    if !routes.is_empty() {
        match client {
            Opencode => opencode(root, &mut configs[0], routes)?,
            Qwen => qwen(root, &mut configs[0], routes)?,
            Codebuddy => {
                configs[0].put(&[], "model", json!(routes[0].model))?;
                let catalog = configs.get_mut(1).ok_or("找不到 CodeBuddy 模型配置路径")?;
                codebuddy(root, catalog, routes)?;
            }
            Kimi => kimi(root, &mut configs[0], routes)?,
            Copilot => {
                copilot(root, &mut configs[0], routes)?;
                let preferences = configs.get_mut(1).ok_or("找不到 Copilot 用户设置路径")?;
                preferences.put(&[], "model", json!(routes[0].selection_id(Copilot)))?;
                preferences.edit(&[], "modelProviderId", None)?;
            }
            _ => return Err("请在 Agent 的原生界面配置模型".into()),
        }
    }
    Ok(configs)
}

pub fn apply(
    root: &Path,
    client: Client,
    store: &Store,
    requested: Vec<RequestedModel>,
) -> Result<Outcome, String> {
    if !requested.is_empty() {
        ensure_compatible(client)?;
    }
    let paths = model_paths(client)?;
    apply_at(
        root,
        client,
        store,
        requested,
        paths,
        &crate::clients::allowed(root),
    )
}

fn apply_at(
    root: &Path,
    client: Client,
    store: &Store,
    requested: Vec<RequestedModel>,
    paths: Vec<PathBuf>,
    allowed: &[PathBuf],
) -> Result<Outcome, String> {
    if !is_added(client) || spec(client).protocols.is_empty() {
        return Err("请在 Agent 的原生界面配置模型".into());
    }
    if requested.len() > 24 {
        return Err("一个客户端最多配置 24 个 Lite 模型".into());
    }
    let mut seen = BTreeSet::new();
    let choices: Vec<_> = requested
        .into_iter()
        .map(|model| {
            if !seen.insert(model.id.clone()) || !(1..=10_000_000).contains(&model.context) {
                return Err("请核对重复模型及上下文上限".into());
            }
            let (saved, account) = store.model(&model.id)?;
            Ok(Choice {
                route: models::route(saved, account),
                id: model.id,
                context: model.context,
            })
        })
        .collect::<Result<_, String>>()?;
    let existing = store.agents.get(&client);
    let current = routes(client, store, &choices)?;
    let mut deltas = Vec::new();
    let mut owned = Vec::new();
    let previous = existing
        .map(|saved| saved.fields(root))
        .transpose()?
        .unwrap_or_default();
    let previous_models: Vec<_> = existing
        .into_iter()
        .flat_map(|selection| &selection.choices)
        .filter_map(|choice| {
            let (saved, account) = store.model(&choice.id).ok()?;
            route(client, saved, account, choice.context)
                .ok()
                .map(|route| route.selection_id(client))
        })
        .collect();
    for config in prepare_configs(root, client, &current, &previous, &previous_models, paths)? {
        owned.extend(config.owned);
        deltas.extend(config.plan.finish());
    }
    let native = if owned.is_empty() {
        None
    } else if owned == previous {
        existing.and_then(|saved| saved.native.clone())
    } else {
        Some(STANDARD.encode(crate::vault::protect(
            root,
            &transaction::json_bytes(&owned)?,
        )?))
    };
    let mut state = Plan::open(&models::path(root))?;
    let saved = (!choices.is_empty()).then(|| {
        json!(Selection {
            choices: choices.clone(),
            native,
            persistent: true,
        })
    });
    state.set(Some("agents"), client.id(), saved)?;
    deltas.extend(state.finish());
    transaction::commit(root, client, deltas, allowed)?;
    Ok(Outcome {
        message: if choices.is_empty() {
            "已恢复原模型配置。新会话将读取恢复后的设置。"
        } else {
            "已写入客户端的模型列表和默认模型。重新启动客户端后生效。"
        }
        .into(),
    })
}

pub fn view(root: &Path, store: &Store, client: Client) -> Result<ClientView, String> {
    let active = store.agents.get(&client);
    let mut view = ClientView {
        state: "ready",
        account: "由客户端管理".into(),
        ..ClientView::official()
    };
    {
        let path = if client == Copilot {
            copilot_home()?.join("settings.json")
        } else {
            settings(client)?
        };
        if path.is_file() {
            let config = Plan::open_as(&path, matches!(client, Opencode | Qwen | Codebuddy))?;
            let current = match client {
                Qwen => config.get(Some("model"), "name")?,
                Kimi => config.get(None, "default_model")?,
                _ => config.get(None, "model")?,
            };
            if let Some(current) = current.and_then(Value::as_str) {
                let mut current = current.to_owned();
                if client == Copilot && !current.contains('/') {
                    if let Some(provider) =
                        config.get(None, "modelProviderId")?.and_then(Value::as_str)
                    {
                        if let Some(provider) = provider
                            .strip_prefix("byok:")
                            .or_else(|| provider.starts_with("codeneo-lite-").then_some(provider))
                        {
                            current = format!("{provider}/{current}");
                        }
                    }
                }
                view.model = current
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(300)
                    .collect();
            }
        } else if active.is_none()
            && !path.parent().is_some_and(Path::is_dir)
            && !spec(client).protocols.is_empty()
        {
            view.state = "uninitialized";
            view.account = "尚无用户配置".into();
        }
    }
    if let Some(active) = active {
        view.state = "lite";
        let native_matches = verify_native(root, client, active).is_ok();
        let current = routes(client, store, &active.choices).ok();
        let selected = current.as_ref().and_then(|routes| {
            routes
                .iter()
                .find(|route| route.selection_id(client) == view.model)
        });
        if let Some(route) = selected {
            view.selected = Some(route.saved.id.clone());
            view.account = route.account.name.clone();
            view.model = route.model.clone();
        }
        view.models = active
            .choices
            .iter()
            .filter_map(|choice| {
                store.model(&choice.id).ok().and_then(|(saved, _)| {
                    crate::model_transport::provider_api_model(&saved.model).ok()
                })
            })
            .collect();
        if !active.persistent {
            view.issue = Some("请重新应用模型列表，使原客户端能够独立读取凭据。".into());
        } else if current.is_none() {
            view.issue = Some("模型档案或凭据已更新，请重新应用。".into());
        } else if !native_matches {
            view.issue = Some("Agent 配置在外部发生变更，请核对已生效的模型。".into());
        }
    }
    if view.issue.is_none()
        && client == Opencode
        && ["opencode.json", "opencode.jsonc"]
            .iter()
            .any(|file| home().is_ok_and(|home| home.join(file).is_file()))
    {
        view.issue =
            Some("启动目录中发现项目配置。若同名项目设置生效，请在 Agent 中核对默认模型。".into());
    }
    Ok(view)
}

fn verify_native(root: &Path, client: Client, selection: &Selection) -> Result<(), String> {
    let mut configs = BTreeMap::new();
    for field in selection.fields(root)? {
        if field.is_default(client) {
            continue;
        }
        if !configs.contains_key(&field.path) {
            configs.insert(field.path.clone(), Plan::open_as(&field.path, true)?);
        }
        let parent: Vec<_> = field.parent.iter().map(String::as_str).collect();
        if configs[&field.path].get_at(&parent, &field.key)? != field.after.as_ref() {
            return Err("模型配置已在原客户端修改，请返回 Lite 核对配置".into());
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn config_env(client: Client) -> &'static [&'static str] {
    match client {
        Opencode => &["XDG_CONFIG_HOME", "XDG_DATA_HOME", "OPENCODE_CONFIG"],
        Qwen => &["QWEN_HOME", "QWEN_RUNTIME_DIR"],
        Codebuddy => &["CODEBUDDY_CONFIG_DIR"],
        Kimi => &["KIMI_CODE_HOME"],
        Copilot => &["COPILOT_HOME", "COPILOT_PROVIDERS_CONFIG"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn incompatible_and_unrecognized_versions_do_not_enter_the_registry() {
        assert!(check_install(Codex, None));
        for version in [None, Some("2.2.7"), Some("1.17.17"), Some("1.19.5-beta")] {
            assert!(!installed_compatible(Opencode, version));
        }
        assert!(installed_compatible(Opencode, Some("1.19.5")));
        assert!(installed_compatible(Qwen, Some("0.27.3")));
        assert!(!installed_compatible(Qwen, Some("0.17.1")));
        assert!(installed_compatible(Codebuddy, Some("2.183.0")));
        assert!(!installed_compatible(Copilot, Some("1.0.109")));
        assert!(installed_compatible(Copilot, Some("1.0.110")));
    }

    #[test]
    fn reapplies_multiple_models_without_losing_native_mcp_and_undo_restores_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config = root.join("settings.json");
        let source = r#"{
            // Qwen user preferences
            "modelProviders":{"openai":[{"id":"existing"}]},
            "model":{"name":"existing"},
            "security":{"auth":{"selectedType":"qwen-oauth"}},
            "mcpServers":{"original":{"command":"server"}},
        }"#;
        fs::write(&config, source).unwrap();
        let ids: Vec<_> = ["fixture-a", "fixture-b"]
            .into_iter()
            .map(|model| {
                models::save(root, serde_json::from_value(json!({
                "preset":"custom","account_name":"Fixture","base_url":"https://example.test/v1",
                "auth":"auth_token","key":"fixture-secret","name":"","model":model,
                "protocol":"chat_completions"
            })).unwrap(), Some(Qwen)).unwrap()
            })
            .collect();
        let paths = vec![models::path(root), config.clone()];
        let apply = |order: &[usize]| {
            let store = models::read(root).unwrap().1;
            let requested = order
                .iter()
                .map(|&index| RequestedModel {
                    id: ids[index].clone(),
                    context: default_context(),
                })
                .collect();
            apply_at(root, Qwen, &store, requested, vec![config.clone()], &paths).unwrap();
        };
        let mut legacy = Config::open(config.clone(), true).unwrap();
        let mut registered = array(&legacy.plan, &["modelProviders"], "openai").unwrap();
        registered.extend(["fixture-a", "fixture-b"].map(|id| {
            json!({
                "id": id, "baseUrl": "https://example.test/v1", "envKey": "LEGACY_LITE_KEY"
            })
        }));
        legacy
            .put(&["modelProviders"], "openai", json!(registered))
            .unwrap();
        legacy.put(&["model"], "name", json!("fixture-a")).unwrap();
        legacy
            .put(&["security", "auth"], "selectedType", json!("openai"))
            .unwrap();
        let (_, saved) = models::read(root).unwrap();
        let choices: Vec<_> = ids
            .iter()
            .map(|id| {
                let (model, account) = saved.model(id).unwrap();
                Choice {
                    id: id.clone(),
                    route: models::route(model, account),
                    context: default_context(),
                }
            })
            .collect();
        let mut state = Plan::open(&models::path(root)).unwrap();
        state
            .set(
                Some("agents"),
                Qwen.id(),
                Some(json!({
                    "choices": choices,
                    "native": STANDARD.encode(crate::vault::protect(
                        root, &transaction::json_bytes(&legacy.owned).unwrap()
                    ).unwrap())
                })),
            )
            .unwrap();
        transaction::write_single(legacy.plan).unwrap();
        transaction::write_single(state).unwrap();
        apply(&[0, 1]);
        let config_plan = Plan::open_as(&config, true).unwrap();
        let registered = array(&config_plan, &["modelProviders"], "openai").unwrap();
        let key_var = registered[1]["envKey"].as_str().unwrap();
        assert_eq!(
            config_plan.get(Some("env"), key_var).unwrap(),
            Some(&json!("fixture-secret"))
        );
        let mut mcp = Plan::open_as(&config, true).unwrap();
        mcp.set(
            Some("mcpServers"),
            "added",
            Some(json!({"httpUrl":"http://localhost:9999/mcp"})),
        )
        .unwrap();
        transaction::commit_mcp(root, Qwen, None, mcp).unwrap();
        apply(&[1, 0]);
        let read = || Plan::open_as(&config, true).unwrap();
        assert_eq!(
            read().get(Some("model"), "name").unwrap(),
            Some(&json!("fixture-b"))
        );
        transaction::undo(root, Qwen, &paths).unwrap();
        assert_eq!(
            read().get(Some("model"), "name").unwrap(),
            Some(&json!("fixture-a"))
        );
        assert!(read().get(Some("mcpServers"), "added").unwrap().is_some());
        transaction::undo_mcp(root, Qwen, None, &config).unwrap();
        apply(&[]);
        let plan = read();
        assert_eq!(
            plan.get(Some("model"), "name").unwrap(),
            Some(&json!("existing"))
        );
        assert_eq!(
            plan.get_at(&["security", "auth"], "selectedType").unwrap(),
            Some(&json!("qwen-oauth"))
        );
        assert!(plan.get(Some("mcpServers"), "original").unwrap().is_some());
        assert!(plan.get(Some("mcpServers"), "added").unwrap().is_none());
        let text = fs::read_to_string(config).unwrap();
        assert!(text.contains("// Qwen user preferences"));
        assert!(!text.contains("fixture-secret"));
        assert!(!fs::read_to_string(models::path(root))
            .unwrap()
            .contains("fixture-secret"));
    }

    #[test]
    fn native_model_shapes_register_all_choices_and_leave_existing_tables() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ids: Vec<_> = ["fixture-a", "fixture-b"]
            .into_iter()
            .map(|model| {
                models::save(root, serde_json::from_value(json!({
                "preset":"custom","account_name":"Fixture","base_url":"https://example.test/v1",
                "auth":"auth_token","key":"fixture-secret","name":"","model":model,
                "protocol":"chat_completions"
            })).unwrap(), None).unwrap()
            })
            .collect();
        let (_, store) = models::read(root).unwrap();
        let choices: Vec<_> = ids
            .iter()
            .map(|id| {
                let (model, account) = store.model(id).unwrap();
                Choice {
                    id: id.clone(),
                    route: models::route(model, account),
                    context: 32768,
                }
            })
            .collect();
        let routes = routes(Opencode, &store, &choices).unwrap();
        let path = dir.path().join("opencode.jsonc");
        let mut config = Config::open(path.clone(), true).unwrap();
        opencode(root, &mut config, &routes).unwrap();
        transaction::write_single(config.plan).unwrap();
        let config = Plan::open(&path).unwrap();
        let provider = config
            .get(Some("provider"), &routes[0].provider())
            .unwrap()
            .unwrap();
        assert_eq!(provider["models"].as_object().unwrap().len(), 2);
        assert_eq!(provider["options"]["apiKey"], "fixture-secret");
        assert_eq!(
            config.get(None, "model").unwrap(),
            Some(&json!(routes[0].selection_id(Opencode)))
        );
        let path = dir.path().join("config.toml");
        fs::write(&path, "# Kimi settings\n[other]\nname = 'original'\n").unwrap();
        let mut config = Config::open(path.clone(), false).unwrap();
        kimi(root, &mut config, &routes).unwrap();
        transaction::write_single(config.plan).unwrap();
        let saved = Plan::open(&path).unwrap();
        assert_eq!(
            saved.get(Some("other"), "name").unwrap(),
            Some(&json!("original"))
        );
        assert_eq!(
            saved
                .get(Some("models"), &routes[1].alias())
                .unwrap()
                .unwrap()["model"],
            "fixture-b"
        );
        assert_eq!(
            saved.get(None, "default_model").unwrap(),
            Some(&json!(routes[0].alias()))
        );
        assert_eq!(
            saved
                .get(Some("providers"), &routes[0].provider())
                .unwrap()
                .unwrap()["api_key"],
            "fixture-secret"
        );
    }
}
