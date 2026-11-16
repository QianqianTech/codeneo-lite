use crate::{
    files, model_transport as http,
    transaction::{self, Client},
    vault,
};
use claudekeeper_claude::{
    normalize_desktop_models, preset_by_id, provider_presets, AuthMode, Profile,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    Anthropic,
    Responses,
    ChatCompletions,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic Messages",
            Self::Responses => "OpenAI Responses",
            Self::ChatCompletions => "Chat Completions",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Account {
    pub id: String,
    pub preset: String,
    pub name: String,
    pub base_url: String,
    pub auth: AuthMode,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    credential: String,
}

impl Account {
    fn protocols(&self) -> &[Protocol] {
        if provider_supports_same_key(&self.preset) {
            ALL_PROTOCOLS
        } else {
            preset_protocols(&self.preset).unwrap_or_else(|| std::slice::from_ref(&self.protocol))
        }
    }

    pub fn supports(&self, client: Client) -> bool {
        crate::agents::spec(client)
            .protocols
            .iter()
            .any(|protocol| self.protocols().contains(protocol))
    }

    pub fn protocol_for(&self, client: Client) -> Protocol {
        let protocols = crate::agents::spec(client).protocols;
        if self.protocol == Protocol::Anthropic
            && self.preset != "openrouter"
            && protocols.contains(&self.protocol)
        {
            return self.protocol;
        }
        protocols
            .iter()
            .copied()
            .find(|protocol| self.protocols().contains(protocol))
            .unwrap_or(self.protocol)
    }

    pub fn base_for(&self, protocol: Protocol) -> &str {
        let base = match (self.preset.as_str(), protocol) {
            ("deepseek-openai", Protocol::Anthropic) => {
                preset_by_id("deepseek").map(|preset| preset.base_url)
            }
            ("deepseek", Protocol::ChatCompletions | Protocol::Responses) => {
                Some(DEEPSEEK_OPENAI_BASE)
            }
            ("kimi" | "kimi-pro", Protocol::ChatCompletions | Protocol::Responses) => {
                Some(KIMI_OPENAI_BASE)
            }
            ("qwen-openai", Protocol::Anthropic) => {
                preset_by_id("qwen").map(|preset| preset.base_url)
            }
            ("qwen", Protocol::ChatCompletions | Protocol::Responses) => Some(QWEN_OPENAI_BASE),
            _ => None,
        };
        base.unwrap_or(&self.base_url)
    }

    pub fn auth_for(&self, protocol: Protocol) -> AuthMode {
        if protocol == Protocol::Anthropic {
            self.auth
        } else {
            AuthMode::AuthToken
        }
    }

    pub fn clients(&self) -> Vec<Client> {
        Client::ALL
            .into_iter()
            .filter(|client| self.supports(*client))
            .collect()
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    pub account: String,
    pub name: String,
    pub model: String,
    pub checked: Option<Check>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktop_models: Vec<String>,
    #[serde(default, skip_serializing_if = "CodeModels::is_empty")]
    pub code_models: CodeModels,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CodeModels {
    pub opus: String,
    pub sonnet: String,
    pub haiku: String,
    pub fable: String,
    pub fast: String,
    pub subagent: String,
    pub reasoning: String,
}

impl CodeModels {
    pub(crate) fn values(&self) -> [&String; 7] {
        [
            &self.opus,
            &self.sonnet,
            &self.haiku,
            &self.fable,
            &self.fast,
            &self.subagent,
            &self.reasoning,
        ]
    }

    fn is_empty(&self) -> bool {
        self.values().into_iter().all(|id| id.is_empty())
    }

    fn normalize(mut self, secret: &str) -> Result<Self, String> {
        for id in [
            &mut self.opus,
            &mut self.sonnet,
            &mut self.haiku,
            &mut self.fable,
            &mut self.fast,
            &mut self.subagent,
            &mut self.reasoning,
        ] {
            *id = if id.trim().is_empty() {
                String::new()
            } else {
                model_id(id)?
            };
            reject_key_in_model(id, secret)?;
        }
        Ok(self)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Check {
    pub healthy: bool,
    pub message: String,
    pub latency_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Applied {
    pub id: String,
    pub route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<crate::desktop_models::Selection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_gateway: Option<crate::codex_inference::Selection>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Pending {
    pub id: Option<String>,
    pub route: Option<String>,
    pub config: String,
    pub issue: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Store {
    pub accounts: Vec<Account>,
    pub models: Vec<Model>,
    pub desktop: Option<Applied>,
    pub code: Option<Applied>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_capabilities: Option<crate::clients::CodeCapabilities>,
    pub codex: Option<Applied>,
    pub codex_original: Option<crate::codex::Original>,
    pub codex_cli: Option<Applied>,
    pub codex_cli_original: Option<crate::codex::Original>,
    pub pending_code: Option<Pending>,
    pub agents: std::collections::BTreeMap<Client, crate::agents::Selection>,
}

pub fn path(root: &Path) -> PathBuf {
    root.join("models.json")
}

pub fn read(root: &Path) -> Result<(Option<Vec<u8>>, Store), String> {
    let bytes = files::read(&path(root))?;
    let store = match &bytes {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| "Lite 的模型档案格式无效")?,
        None => Store::default(),
    };
    Ok((bytes, store))
}

pub fn write(root: &Path, old: Option<&[u8]>, store: &Store) -> Result<(), String> {
    files::replace(&path(root), old, &transaction::json_bytes(store)?)
}

impl Store {
    pub fn model(&self, id: &str) -> Result<(&Model, &Account), String> {
        let model = self
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or("找不到模型档案")?;
        let account = self
            .accounts
            .iter()
            .find(|account| account.id == model.account)
            .ok_or("找不到模型的接入账号")?;
        Ok((model, account))
    }

    pub fn in_use(&self) -> impl Iterator<Item = &str> {
        [&self.desktop, &self.code, &self.codex, &self.codex_cli]
            .into_iter()
            .flatten()
            .map(|applied| applied.id.as_str())
            .chain(
                self.pending_code
                    .as_ref()
                    .and_then(|pending| pending.id.as_deref()),
            )
            .chain(
                self.agents
                    .values()
                    .flat_map(|agent| agent.choices.iter().map(|choice| choice.id.as_str())),
            )
    }
}

fn credential_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("模型的 Key 引用无效，请重新填写 Key".into());
    }
    Ok(root.join("model-credentials").join(format!("{id}.bin")))
}

pub fn key(root: &Path, account: &Account) -> Result<String, String> {
    let key = read_key(root, account)?;
    validate_key_for_provider(account, &key)?;
    Ok(key)
}

pub(crate) fn has_readable_key(root: &Path, account: &Account) -> bool {
    read_key(root, account).is_ok()
}

fn read_key(root: &Path, account: &Account) -> Result<String, String> {
    let path = credential_path(root, &account.credential)?;
    let bytes = vault::read(root, &path)?.ok_or("此接入账号的 Key 已丢失，请重新填写")?;
    let key = String::from_utf8(bytes).map_err(|_| "此接入账号的 Key 无法读取")?;
    validate_key(&key)?;
    Ok(key)
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 4096
        || reqwest::header::HeaderValue::from_str(key).is_err()
        || key.contains(char::is_whitespace)
    {
        return Err("请填写有效的模型 API Key".into());
    }
    Ok(())
}

pub(crate) fn is_aliyun_token_plan(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.host_str() == Some("token-plan.cn-beijing.maas.aliyuncs.com")
}

fn validate_key_for_provider(account: &Account, key: &str) -> Result<(), String> {
    // Also check accounts configured manually for this endpoint.
    let token_plan =
        reqwest::Url::parse(&account.base_url).is_ok_and(|url| is_aliyun_token_plan(&url));
    if token_plan && !key.strip_prefix("sk-sp-").is_some_and(|tail| !tail.is_empty()) {
        return Err("千问 Token Plan 地址需要套餐专属 API Key（以 sk-sp- 开头）。请在模型库更新该接入账号的 Key，再重新应用档案。".into());
    }
    Ok(())
}

pub fn route(model: &Model, account: &Account) -> String {
    let fields = serde_json::json!([
        account.id,
        account.base_url,
        account.auth,
        account.credential,
        model.model
    ]);
    format!("{:x}", Sha256::digest(fields.to_string().as_bytes()))
}

pub fn client_route(client: Client, model: &Model, account: &Account) -> String {
    let base = route(model, account);
    let extra = match client {
        Client::Desktop if !model.desktop_models.is_empty() => {
            serde_json::json!(model.desktop_models)
        }
        Client::Code if !model.code_models.is_empty() => serde_json::json!(model.code_models),
        _ => return base,
    };
    format!(
        "{:x}",
        Sha256::digest(serde_json::json!([base, extra]).to_string().as_bytes())
    )
}

pub fn supports_model(client: Client, model: &Model, account: &Account) -> bool {
    account.supports(client)
        && (client != Client::Desktop
            || !model.desktop_models.is_empty()
            || normalize_desktop_models(std::slice::from_ref(&model.model)).is_ok())
}

pub fn require_model(client: Client, model: &Model, account: &Account) -> Result<(), String> {
    if !account.supports(client) {
        return Err(format!(
            "{} 需要 {} 兼容接口",
            client.name(),
            crate::agents::spec(client)
                .protocols
                .iter()
                .map(|p| p.label())
                .collect::<Vec<_>>()
                .join(" 或 ")
        ));
    }
    if !supports_model(client, model, account) {
        return Err(format!(
            "{} 需要具体的模型 ID，请在模型库中选择一个模型",
            client.name()
        ));
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Preset {
    id: &'static str,
    provider: &'static str,
    name: &'static str,
    base_url: &'static str,
    auth: AuthMode,
    protocols: &'static [Protocol],
    models: &'static [&'static str],
    clients: Vec<Client>,
}

pub fn presets() -> Vec<Preset> {
    provider_presets()
        .iter()
        .map(|preset| preset.id)
        .filter(|id| *id != "kimi-pro")
        .chain(["openai", "custom"])
        .filter_map(preset)
        .collect()
}

const ALL_PROTOCOLS: &[Protocol] = &[
    Protocol::Anthropic,
    Protocol::Responses,
    Protocol::ChatCompletions,
];
const OPENAI_PROTOCOLS: &[Protocol] = &[Protocol::Responses, Protocol::ChatCompletions];
const DEEPSEEK_OPENAI_BASE: &str = "https://api.deepseek.com";
const KIMI_OPENAI_BASE: &str = "https://api.kimi.com/coding/v1";
const QWEN_OPENAI_BASE: &str = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";

fn preset_protocols(id: &str) -> Option<&'static [Protocol]> {
    match id {
        "openai" | "deepseek-openai" | "qwen-openai" => Some(OPENAI_PROTOCOLS),
        _ => None,
    }
}

fn provider_supports_same_key(id: &str) -> bool {
    matches!(provider_id(id), "deepseek" | "kimi" | "qwen" | "openrouter")
}

pub fn provider_id(preset: &str) -> &str {
    match preset {
        "deepseek-openai" => "deepseek",
        "kimi-pro" => "kimi",
        "qwen-openai" => "qwen",
        id => id,
    }
}

fn preset(id: &str) -> Option<Preset> {
    let (id, name, base_url, auth) = match id {
        "openai" => (
            "openai",
            "OpenAI API",
            "https://api.openai.com/v1",
            AuthMode::AuthToken,
        ),
        "deepseek-openai" => (
            "deepseek-openai",
            "DeepSeek（OpenAI 兼容）",
            DEEPSEEK_OPENAI_BASE,
            AuthMode::AuthToken,
        ),
        "qwen-openai" => (
            "qwen-openai",
            "千问 Token Plan（OpenAI 兼容）",
            QWEN_OPENAI_BASE,
            AuthMode::AuthToken,
        ),
        "custom" => ("custom", "自定义兼容接口", "", AuthMode::AuthToken),
        id => {
            let p = preset_by_id(id)?;
            (p.id, p.kind, p.base_url, p.auth)
        }
    };
    let provider = provider_id(id);
    let models: &[&str] = match id {
        "glm" => &["glm-5.3", "glm-5.3-flash"],
        _ => match provider {
            "deepseek" => &["deepseek-flash", "deepseek-v4-pro"],
            "kimi" => &[
                "k3.1-pro",
                "k3.1-pro-highspeed",
                "k3",
                "k3-256k",
                "kimi-for-coding",
                "kimi-for-coding-highspeed",
            ],
            // Text models documented for Token Plan. Availability varies by subscription.
            // https://help.aliyun.com/zh/model-studio/token-plan-team-overview
            "qwen" => &[
                "qwen4-max-preview",
                "qwen3.8-max",
                "qwen3.8-flash",
                "qwen3.7-max",
                "qwen3.7-plus",
                "qwen3.6-plus",
                "qwen3.6-flash",
                "auto",
                "deepseek-v4.1-flash",
                "deepseek-v4.1-flash-1017",
                "deepseek-v4-pro",
                "deepseek-v4-pro-0813",
                "deepseek-v4-flash-0731",
                "kimi-k2.7-code",
                "kimi-k2.6",
                "kimi-k2.5",
                "glm-5.3",
                "glm-5.2",
                "glm-5.1",
                "glm-5",
                "MiniMax-M2.5",
            ],
            "openrouter" => &[
                "anthropic/claude-sonnet-5.5",
                "anthropic/claude-opus-5.5",
                "deepseek/deepseek-v4.1-flash",
                "z-ai/glm-5.3",
            ],
            "openai" => &[
                "gpt-6-astra",
                "gpt-6.1-sol",
                "gpt-6.1-terra",
                "gpt-6-luna",
                "gpt-5.6-sol",
                "gpt-5.4-mini",
            ],
            _ => &[],
        },
    };
    let protocols = if id == "custom" || provider_supports_same_key(id) {
        ALL_PROTOCOLS
    } else {
        preset_protocols(id).unwrap_or(&[Protocol::Anthropic])
    };
    Some(Preset {
        id,
        provider,
        name,
        base_url,
        auth,
        protocols,
        models,
        clients: compatible_clients(protocols),
    })
}

fn compatible_clients(protocols: &[Protocol]) -> Vec<Client> {
    Client::ALL
        .into_iter()
        .filter(|client| {
            crate::agents::spec(*client)
                .protocols
                .iter()
                .any(|p| protocols.contains(p))
        })
        .collect()
}

#[derive(Deserialize)]
pub struct Input {
    pub id: Option<String>,
    account_id: Option<String>,
    preset: String,
    account_name: String,
    base_url: String,
    auth: AuthMode,
    key: String,
    name: String,
    model: String,
    #[serde(default)]
    desktop_models: Option<Vec<String>>,
    #[serde(default)]
    code_models: Option<CodeModels>,
    #[serde(default)]
    protocol: Protocol,
}

#[derive(Deserialize)]
pub struct SelectedModel {
    model: String,
    #[serde(default)]
    name: String,
}

pub(crate) fn saved_input(
    root: &Path,
    client: Client,
    id: &str,
    gateway: bool,
) -> Result<Input, String> {
    let (_, store) = read(root)?;
    let (model, account) = store.model(id)?;
    require_model(client, model, account)?;
    let name = if client == Client::Desktop {
        if gateway || !model.desktop_models.is_empty() {
            crate::desktop_models::model_names(model)?[0].clone()
        } else {
            code_model(model, account)
        }
    } else {
        model.model.clone()
    };
    Ok(Input {
        id: Some(id.into()),
        account_id: Some(account.id.clone()),
        preset: account.preset.clone(),
        account_name: account.name.clone(),
        base_url: account.base_url.clone(),
        auth: account.auth,
        key: String::new(),
        name: model.name.clone(),
        model: name,
        desktop_models: None,
        code_models: None,
        protocol: account.protocol,
    })
}

fn clean_name(value: &str, fallback: &str) -> Result<String, String> {
    let value = value.trim();
    let value = if value.is_empty() { fallback } else { value };
    if value.chars().count() > 96 || value.chars().any(char::is_control) {
        return Err("名称过长或包含无效字符".into());
    }
    Ok(value.to_string())
}

fn model_id(value: &str) -> Result<String, String> {
    let model = value.trim();
    if model.chars().any(char::is_whitespace) {
        return Err("模型 ID 不能包含空格".into());
    }
    if http::provider_api_model(model)?.is_empty() {
        return Err("请填写有效的模型 ID".into());
    }
    Ok(model.to_string())
}

fn reject_key_in_model(model: &str, secret: &str) -> Result<(), String> {
    if !secret.is_empty() && (model == secret || (secret.len() >= 8 && model.contains(secret))) {
        return Err("请填写模型 ID。API Key 填在接入账号的密钥栏。".into());
    }
    Ok(())
}

fn draft(root: &Path, store: &Store, input: &Input) -> Result<(Account, String), String> {
    let account = if let Some(id) = input.account_id.as_deref() {
        store
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or("接入账号已删除，请重新选择")?
    } else {
        let (label, url, auth, protocol) = if input.preset == "custom" {
            (
                "自定义接入",
                input.base_url.as_str(),
                input.auth,
                input.protocol,
            )
        } else {
            let preset = preset(&input.preset).ok_or("请选择模型服务商")?;
            let protocol = preset_protocols(preset.id).map_or(Protocol::Anthropic, |p| p[0]);
            (preset.name, preset.base_url, preset.auth, protocol)
        };
        if auth == AuthMode::Official {
            return Err("请选择 Bearer 或 x-api-key 认证方式".into());
        }
        if protocol != Protocol::Anthropic && auth != AuthMode::AuthToken {
            return Err("OpenAI 兼容接口使用 Bearer 认证".into());
        }
        let base = http::provider_base(url)?;
        let base_url = if protocol == Protocol::Anthropic {
            base.as_str().trim_end_matches('/').to_owned()
        } else {
            reqwest::Url::parse(url.trim())
                .map_err(|_| "模型接口地址无效")?
                .as_str()
                .trim_end_matches('/')
                .to_owned()
        };
        Account {
            id: String::new(),
            preset: input.preset.clone(),
            name: clean_name(&input.account_name, label)?,
            base_url,
            auth,
            protocol,
            credential: String::new(),
        }
    };
    let entered_key = input.key.trim();
    let key = if entered_key.is_empty() && !account.id.is_empty() {
        key(root, &account)?
    } else {
        validate_key(entered_key)?;
        validate_key_for_provider(&account, entered_key)?;
        entered_key.to_owned()
    };
    Ok((account, key))
}

pub fn save(root: &Path, input: Input, target: Option<Client>) -> Result<String, String> {
    save_models(root, input, target, Vec::new()).map(|mut ids| ids.remove(0))
}

pub fn save_models(
    root: &Path,
    input: Input,
    target: Option<Client>,
    selected: Vec<SelectedModel>,
) -> Result<Vec<String>, String> {
    const LIMIT: usize = 300;
    if selected.len() >= LIMIT || (!selected.is_empty() && input.id.is_some()) {
        return Err("模型档案数量已达上限或无法批量编辑已有档案".into());
    }
    let batch = !selected.is_empty();
    let _lock = files::lock(root)?;
    let (before, mut store) = read(root)?;
    let (mut account, clear) = draft(root, &store, &input)?;
    let existing_account = account
        .id
        .is_empty()
        .then(|| {
            store.accounts.iter().find(|saved| {
                provider_id(&saved.preset) == provider_id(&account.preset)
                    && (provider_supports_same_key(&account.preset)
                        || (saved.base_url == account.base_url
                            && saved.protocol == account.protocol
                            && saved.auth == account.auth))
                    && key(root, saved).as_deref() == Ok(clear.as_str())
            })
        })
        .flatten();
    let reuse = existing_account.is_some();
    if let Some(existing) = existing_account {
        account = existing.clone();
    }
    if let Some(client) = target.filter(|client| !account.supports(*client)) {
        return Err(format!("此接入账号不适用于 {}", client.name()));
    }
    let index = input
        .id
        .as_ref()
        .map(|id| {
            store
                .models
                .iter()
                .position(|m| m.id == *id)
                .ok_or("模型档案已删除")
        })
        .transpose()?;
    let previous = index.map(|i| &store.models[i]);
    let raw_desktop = input
        .desktop_models
        .or_else(|| previous.map(|old| old.desktop_models.clone()))
        .unwrap_or_default();
    let desktop_models =
        normalize_desktop_models(&raw_desktop).map_err(|error| error.to_string())?;
    for id in &desktop_models {
        reject_key_in_model(id, &clear)?;
    }
    let code_models = input
        .code_models
        .or_else(|| previous.map(|old| old.code_models.clone()))
        .unwrap_or_default()
        .normalize(&clear)?;
    if (!desktop_models.is_empty() || !code_models.is_empty()) && !account.supports(Client::Code) {
        return Err("Claude 的多模型档案需要 Anthropic Messages 兼容接口。".into());
    }
    let default = if input.model.trim().is_empty() {
        desktop_models
            .first()
            .map(String::as_str)
            .unwrap_or(&input.model)
    } else {
        input.model.as_str()
    };
    let model = model_id(default)?;
    reject_key_in_model(&model, &clear)?;
    if target == Some(Client::Desktop) && desktop_models.is_empty() {
        normalize_desktop_models(std::slice::from_ref(&model))
            .map_err(|error| error.to_string())?;
    }
    let label = clean_name(&input.name, &model)?;
    if account.id.is_empty() {
        account.id = vault::id()?;
    }
    let account_index = store.accounts.iter().position(|a| a.id == account.id);
    let previous_key = account.credential.clone();
    let group_desktop = batch && target == Some(Client::Desktop);
    let mut items = vec![Model {
        id: String::new(),
        account: account.id.clone(),
        name: label,
        model,
        checked: None,
        desktop_models,
        code_models,
    }];
    let mut seen = BTreeSet::from([http::provider_api_model(&items[0].model)?]);
    for selection in selected {
        let model = model_id(&selection.model)?;
        reject_key_in_model(&model, &clear)?;
        if !seen.insert(http::provider_api_model(&model)?) {
            continue;
        }
        if target == Some(Client::Desktop) {
            normalize_desktop_models(std::slice::from_ref(&model))
                .map_err(|error| error.to_string())?;
        }
        if group_desktop {
            let desktop = &mut items[0];
            if desktop.desktop_models.is_empty() {
                desktop
                    .desktop_models
                    .push(api_id(&desktop.model).to_owned());
            }
            let model = api_id(&model);
            if !desktop.desktop_models.iter().any(|id| id == model) {
                desktop.desktop_models.push(model.to_owned());
            }
            continue;
        }
        items.push(Model {
            id: String::new(),
            account: account.id.clone(),
            name: clean_name(&selection.name, &model)?,
            model,
            checked: None,
            desktop_models: Vec::new(),
            code_models: CodeModels::default(),
        });
    }
    let mut ids = Vec::with_capacity(items.len());
    for (position, mut item) in items.into_iter().enumerate() {
        let duplicate = index
            .is_none()
            .then(|| {
                store.models.iter().position(|saved| {
                    saved.account == item.account && api_id(&saved.model) == api_id(&item.model)
                })
            })
            .flatten();
        if let Some(i) = duplicate.filter(|_| {
            position != 0 || item.desktop_models.is_empty() && item.code_models.is_empty()
        }) {
            ids.push(store.models[i].id.clone());
            continue;
        }
        if let Some(i) = index.or(duplicate) {
            let old = &store.models[i];
            item.id = old.id.clone();
            if duplicate.is_some() {
                item.name = old.name.clone();
                item.model = old.model.clone();
                if item.desktop_models.is_empty() {
                    item.desktop_models = old.desktop_models.clone();
                } else if group_desktop {
                    let mut names = old.desktop_models.clone();
                    if names.is_empty() {
                        names.push(api_id(&old.model).to_owned());
                    }
                    names.append(&mut item.desktop_models);
                    item.desktop_models =
                        normalize_desktop_models(&names).map_err(|error| error.to_string())?;
                }
                if item.code_models.is_empty() {
                    item.code_models = old.code_models.clone();
                }
            }
            if old.model == item.model && old.account == item.account {
                item.checked = old.checked.clone();
            }
            ids.push(item.id.clone());
            store.models[i] = item;
        } else {
            if store.models.len() >= LIMIT {
                return Err("模型档案数量已达上限".into());
            }
            item.id = vault::id()?;
            ids.push(item.id.clone());
            store.models.push(item);
        }
    }
    let new_path = if previous_key.is_empty() || (!reuse && !input.key.trim().is_empty()) {
        account.credential = vault::id()?;
        let path = credential_path(root, &account.credential)?;
        files::private_dir(path.parent().ok_or("凭据目录无效")?)?;
        vault::write(root, &path, clear.as_bytes())?;
        for model in &mut store.models {
            if model.account == account.id {
                model.checked = None;
            }
        }
        Some(path)
    } else {
        None
    };
    if let Some(i) = account_index {
        store.accounts[i] = account;
    } else {
        store.accounts.push(account);
    }
    let stale = prune_accounts(&mut store);
    if let Err(error) = write(root, before.as_deref(), &store) {
        if let Some(path) = new_path {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    if new_path.is_some() {
        delete_credential(root, &previous_key);
    }
    for id in stale {
        delete_credential(root, &id);
    }
    Ok(ids)
}

fn api_id(model: &str) -> &str {
    model.strip_suffix("[1m]").unwrap_or(model)
}

fn prune_accounts(store: &mut Store) -> Vec<String> {
    let mut removed = Vec::new();
    store.accounts.retain(|account| {
        let keep = store.models.iter().any(|model| model.account == account.id);
        if !keep {
            removed.push(account.credential.clone());
        }
        keep
    });
    removed
}

fn delete_credential(root: &Path, id: &str) {
    if let Ok(path) = credential_path(root, id) {
        let _ = fs::remove_file(path);
    }
}

pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    let _lock = files::lock(root)?;
    let (before, mut store) = read(root)?;
    if store.in_use().any(|used| used == id) {
        return Err("此档案正在使用或等待应用，请先切换客户端或取消等待".into());
    }
    store.model(id)?;
    store.models.retain(|model| model.id != id);
    let stale = prune_accounts(&mut store);
    write(root, before.as_deref(), &store)?;
    for id in stale {
        delete_credential(root, &id);
    }
    Ok(())
}

fn header(auth: AuthMode, key: &str) -> (&'static str, String) {
    match auth {
        AuthMode::ApiKey => ("x-api-key", key.to_string()),
        _ => ("authorization", format!("Bearer {key}")),
    }
}

pub async fn catalog(
    root: &Path,
    input: Input,
    client: Option<Client>,
) -> Result<http::ProviderCatalog, String> {
    let (_, store) = read(root)?;
    let (account, key) = draft(root, &store, &input)?;
    if let Some(client) = client.filter(|client| !account.supports(*client)) {
        return Err(format!("此接入账号不适用于 {}", client.name()));
    }
    let transport = http::ReqwestProviderModels::new(None)?;
    fetch_catalog(&transport, &account, &key, client).await
}

async fn fetch_catalog(
    transport: &(impl http::ProviderTransport + Sync),
    account: &Account,
    key: &str,
    client: Option<Client>,
) -> Result<http::ProviderCatalog, String> {
    let protocol = client.map_or(account.protocol, |client| account.protocol_for(client));
    let catalog_protocol = if provider_id(&account.preset) == "kimi" {
        Protocol::ChatCompletions
    } else {
        protocol
    };
    let base = http::provider_base(account.base_for(catalog_protocol))?;
    let (name, value) = header(account.auth_for(catalog_protocol), key);
    if account.preset == "openrouter" {
        let mut url = base;
        url.set_path("/api/v1/models/user");
        let label = match client {
            Some(Client::Desktop) => Some("claude_desktop"),
            Some(Client::Code) => Some("claude_code"),
            _ => None,
        };
        if let Some(label) = label {
            url.query_pairs_mut().append_pair("client", label);
        }
        let urls = [
            url.to_string(),
            openai_endpoint(account, catalog_protocol, "/models"),
        ];
        return http::fetch_provider_catalog_from_urls(transport, &urls, (name, &value)).await;
    }
    if catalog_protocol != Protocol::Anthropic {
        let url = openai_endpoint(account, catalog_protocol, "/models");
        return http::fetch_provider_catalog_from_urls(transport, &[url], (name, &value)).await;
    }
    http::fetch_provider_catalog(transport, &base, (name, &value)).await
}

pub async fn test(root: &Path, input: Input, client: Option<Client>) -> Result<Check, String> {
    let (_, store) = read(root)?;
    let (account, key) = draft(root, &store, &input)?;
    let model = model_id(&input.model)?;
    if let Some(client) = client.filter(|client| !account.supports(*client)) {
        return Err(format!("此接入账号不适用于 {}", client.name()));
    }
    let protocol = client.map_or(account.protocol, |client| account.protocol_for(client));
    let base = http::provider_base(account.base_for(protocol))?;
    let (name, value) = header(account.auth_for(protocol), &key);
    let transport = http::ReqwestProviderModels::new(None)?;
    let checked = match protocol {
        Protocol::Anthropic => {
            let result =
                http::verify_provider_model(&transport, &base, (name, &value), &model).await?;
            Check {
                healthy: result.healthy,
                message: result.message,
                latency_ms: result.latency_ms,
            }
        }
        _ => test_openai(&transport, &account, (name, &value), &model, protocol).await,
    };
    if let Some(id) = input.id.as_deref().filter(|_| input.key.trim().is_empty()) {
        let _lock = files::lock(root)?;
        let (before, mut store) = read(root)?;
        let current = store.model(id).ok();
        if current.is_some_and(|(saved, channel)| {
            saved.model == model
                && channel.id == account.id
                && channel.credential == account.credential
        }) {
            if let Some(saved) = store.models.iter_mut().find(|model| model.id == id) {
                saved.checked = Some(checked.clone());
            }
            write(root, before.as_deref(), &store)?;
        }
    }
    Ok(checked)
}

pub fn openai_endpoint(account: &Account, protocol: Protocol, route: &str) -> String {
    let base = account.base_for(protocol).trim_end_matches('/');
    let version = if account.preset == "openrouter" {
        "/v1"
    } else {
        ""
    };
    format!("{base}{version}{route}")
}

async fn test_openai(
    transport: &impl http::ProviderTransport,
    account: &Account,
    header: (&str, &str),
    model: &str,
    protocol: Protocol,
) -> Check {
    use serde_json::json;
    let model = api_id(model);
    let responses = protocol == Protocol::Responses;
    let prompt = "Reply with OK only.";
    let (route, mut body) = if responses {
        (
            "/responses",
            json!({"model": model, "input": prompt, "max_output_tokens": 32, "store": false}),
        )
    } else {
        (
            "/chat/completions",
            json!({"model": model, "messages": [{"role": "user", "content": prompt}]}),
        )
    };
    if !responses {
        let limit = if account.preset == "openai" {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[limit] = json!(32);
    }
    let start = Instant::now();
    let result = transport
        .request(
            &openai_endpoint(account, protocol, route),
            header,
            Some(body),
        )
        .await;
    let status = match result {
        Ok(reply) if (200..300).contains(&reply.status) => {
            let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            let content = if responses {
                body["output"].as_array()
            } else {
                body["choices"].as_array()
            };
            if body.get("error").is_none_or(serde_json::Value::is_null)
                && content.is_some_and(|items| !items.is_empty())
            {
                Ok("模型调用通过".to_owned())
            } else {
                Err("接口有响应，但没有返回可用的生成结果".into())
            }
        }
        Ok(reply) => Err(match reply.status {
            401 | 403 => "API Key 无效，或没有该模型的调用权限".into(),
            404 | 405 => format!(
                "请检查接口是否支持 {}，以及模型 ID 是否正确",
                protocol.label()
            ),
            429 => "调用受到频率或额度限制".into(),
            status => format!("模型接口返回 HTTP {status}，请检查服务商配置"),
        }),
        Err(error) => Err(error),
    };
    Check {
        healthy: status.is_ok(),
        message: status.unwrap_or_else(|error| error),
        latency_ms: start.elapsed().as_millis() as u64,
    }
}

pub fn desktop_profile(model: &Model, account: &Account) -> Profile {
    let mut profile = claude_profile(model, account);
    profile.desktop_models = model.desktop_models.clone();
    profile
}

fn claude_profile(model: &Model, account: &Account) -> Profile {
    let mut profile = Profile::official();
    profile.id = model.id.clone();
    profile.name = model.name.clone();
    profile.base_url = account.base_for(Protocol::Anthropic).to_owned();
    profile.auth = account.auth_for(Protocol::Anthropic);
    profile.model = code_model(model, account);
    profile
}

pub fn code_profile(model: &Model, account: &Account) -> Profile {
    let mut profile = claude_profile(model, account);
    let slot = |id: &str| {
        if id.is_empty() {
            profile.model.clone()
        } else {
            resolved_code_model(id, account)
        }
    };
    profile.opus_model = slot(&model.code_models.opus);
    profile.sonnet_model = slot(&model.code_models.sonnet);
    profile.haiku_model = slot(&model.code_models.haiku);
    profile.fable_model = slot(&model.code_models.fable);
    profile.small_fast_model = slot(&model.code_models.fast);
    profile.subagent_model = slot(&model.code_models.subagent);
    profile.reasoning_model = resolved_code_model(&model.code_models.reasoning, account);
    profile
}

pub fn code_model(model: &Model, account: &Account) -> String {
    resolved_code_model(&model.model, account)
}

fn resolved_code_model(model: &str, account: &Account) -> String {
    if model.ends_with("[1m]") {
        return model.to_owned();
    }
    if let Some(preset) = preset_by_id(provider_id(&account.preset)) {
        for id in [
            preset.model,
            preset.sonnet_model,
            preset.opus_model,
            preset.haiku_model,
        ] {
            if id.strip_suffix("[1m]") == Some(model) {
                return id.to_string();
            }
        }
    }
    model.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(preset: &str, account: Option<&str>, model: &str, key: &str) -> Input {
        serde_json::from_value(json!({
            "account_id": account, "preset": preset, "key": key, "model": model, "name": "",
            "account_name": "", "base_url": "", "auth": "auth_token"
        }))
        .unwrap()
    }

    fn selection(model: &str) -> SelectedModel {
        SelectedModel {
            model: model.into(),
            name: String::new(),
        }
    }

    #[test]
    fn qwen_token_plan_rejects_incompatible_keys_at_save_and_when_reading_legacy_accounts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for preset in ["qwen", "qwen-openai"] {
            let error = save(root, input(preset, None, "qwen3.8-max", "sk-wrong"), None)
                .unwrap_err();
            assert!(error.contains("sk-sp-"));
            assert!(!error.contains("sk-wrong"));
        }
        assert!(!path(root).exists());

        let id = save(
            root,
            input("qwen", None, "qwen3.8-max", "sk-sp-test"),
            Some(Client::Desktop),
        )
        .unwrap();
        let (_, store) = read(root).unwrap();
        let (_, account) = store.model(&id).unwrap();
        let old = credential_path(root, &account.credential).unwrap();
        vault::write(root, &old, b"sk-legacy-key").unwrap();
        assert!(has_readable_key(root, account));
        assert!(key(root, account).unwrap_err().contains("sk-sp-"));

        let mut edit = saved_input(root, Client::Desktop, &id, true).unwrap();
        edit.key = "sk-sp-replacement".into();
        assert_eq!(save(root, edit, Some(Client::Desktop)).unwrap(), id);
        let (_, store) = read(root).unwrap();
        assert_eq!(
            key(root, store.model(&id).unwrap().1).unwrap(),
            "sk-sp-replacement"
        );

        let mut other = input("custom", None, "different-model", "sk-generic-key");
        other.base_url = "https://custom.example/apps/anthropic".into();
        assert!(save(root, other, None).is_ok());
    }

    #[test]
    fn batch_save_reuses_one_key_and_rejects_the_entire_invalid_selection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let ids = save_models(
            root,
            input("deepseek", None, "deepseek-flash", "fixture-key"),
            None,
            vec![
                selection("deepseek-v4-pro"),
                selection("deepseek-flash[1m]"),
            ],
        )
        .unwrap();
        assert_eq!(ids.len(), 2);
        let (before, store) = read(root).unwrap();
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.models.len(), 2);
        let (first, account) = store.model(&ids[0]).unwrap();
        assert_eq!(first.model, "deepseek-flash");
        assert_eq!(store.model(&ids[1]).unwrap().1.id, account.id);
        assert_eq!(key(root, account).unwrap(), "fixture-key");
        assert_eq!(
            fs::read_dir(root.join("model-credentials"))
                .unwrap()
                .count(),
            1
        );

        assert!(save_models(
            root,
            input(
                "deepseek",
                Some(&account.id),
                "new-model",
                "fixture-new-key"
            ),
            None,
            vec![selection("invalid model")],
        )
        .is_err());
        assert_eq!(read(root).unwrap().0, before);
        assert_eq!(key(root, account).unwrap(), "fixture-key");

        let retried = save_models(
            root,
            input("deepseek-openai", None, "deepseek-flash", "fixture-key"),
            Some(Client::Codex),
            vec![selection("deepseek-v4-pro")],
        )
        .unwrap();
        assert_eq!(retried, ids);
        let (_, store) = read(root).unwrap();
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.models.len(), 2);
        assert_eq!(store.accounts[0].credential, account.credential);
    }

    #[test]
    fn desktop_batch_extends_the_existing_profile_and_keeps_its_default() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut existing = input("kimi", None, "k3.1-pro", "fixture-key");
        existing.name = "Existing profile".into();
        existing.desktop_models = Some(vec!["k3".into(), "k3.1-pro".into()]);
        let id = save(root, existing, Some(Client::Desktop)).unwrap();
        let (_, before) = read(root).unwrap();
        let account = &before.accounts[0];
        let ids = save_models(
            root,
            input("kimi", Some(&account.id), "k3.1-pro", ""),
            Some(Client::Desktop),
            vec![selection("kimi-for-coding")],
        )
        .unwrap();
        assert_eq!(ids, [id.clone()]);
        let (_, store) = read(root).unwrap();
        let (profile, account) = store.model(&id).unwrap();
        assert_eq!(store.models.len(), 1);
        assert_eq!(profile.name, "Existing profile");
        assert_eq!(
            profile.desktop_models,
            ["k3", "k3.1-pro", "kimi-for-coding"]
        );
        assert_eq!(account.credential, before.accounts[0].credential);
        assert!(account.supports(Client::Codex));
        assert_eq!(account.auth_for(Protocol::Anthropic), AuthMode::ApiKey);
        assert_eq!(account.auth_for(Protocol::Responses), AuthMode::AuthToken);
        assert_eq!(
            openai_endpoint(account, account.protocol_for(Client::Codex), "/responses"),
            "https://api.kimi.com/coding/v1/responses"
        );
    }

    #[tokio::test]
    async fn catalog_uses_the_vendor_endpoint_and_auth_for_the_selected_client() {
        struct CatalogStub(&'static str, &'static str);
        impl http::ProviderTransport for CatalogStub {
            async fn request(
                &self,
                url: &str,
                header: (&str, &str),
                body: Option<serde_json::Value>,
            ) -> Result<http::ProviderReply, String> {
                assert_eq!(url, self.0);
                assert_eq!(header.0, "authorization");
                assert_eq!(header.1, format!("Bearer {}", self.1));
                assert!(body.is_none());
                Ok(http::ProviderReply {
                    status: 200,
                    body: br#"{"data":[{"id":"first"},{"id":"second"}]}"#.to_vec(),
                })
            }
        }
        let temp = tempfile::tempdir().unwrap();
        for (preset, client, url) in [
            (
                "kimi",
                Some(Client::Desktop),
                "https://api.kimi.com/coding/v1/models",
            ),
            (
                "kimi",
                Some(Client::Codex),
                "https://api.kimi.com/coding/v1/models",
            ),
            (
                "deepseek-openai",
                Some(Client::Desktop),
                "https://api.deepseek.com/models",
            ),
            (
                "qwen",
                Some(Client::Codex),
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/models",
            ),
            (
                "openrouter",
                Some(Client::Codex),
                "https://openrouter.ai/api/v1/models/user",
            ),
        ] {
            let credential = if preset == "qwen" {
                "sk-sp-fixture-key"
            } else {
                "fixture-key"
            };
            let (account, key) = draft(
                temp.path(),
                &Store::default(),
                &input(preset, None, "", credential),
            )
            .unwrap();
            let result = fetch_catalog(&CatalogStub(url, credential), &account, &key, client)
                .await
                .unwrap();
            assert_eq!(result.models.len(), 2);
        }
    }
}
