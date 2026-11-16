use crate::{
    clients::{ClientView, Outcome},
    codex_inference, desktop,
    gateway::{self, Connection},
    model_effort,
    model_transport as http,
    models::{self, Applied, Store},
    transaction::{self, Client, Delta, Field, Plan},
    vault,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

const PROVIDER: &str = "codeneo-lite";
const PROVIDERS: &str = "model_providers";
const CATALOG: &str = "codeneo-lite-models.json";
const CLI_CATALOG: &str = "codeneo-lite-cli-models.json";
const CATALOG_SETTING: &str = "model_catalog_json";
const DESKTOP_EFFORTS: &str = "enabled-reasoning-efforts";
pub const KEY_FLAG: &str = "--codex-key";

#[derive(Clone, Deserialize, Serialize)]
pub struct Original {
    path: PathBuf,
    model: Option<Value>,
    provider: Option<Value>,
    // A null value records that the key was absent before an explicit switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<Value>,
    #[serde(default)]
    official_override: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    desktop_effort: Option<DesktopEffort>,
}

#[derive(Clone, Deserialize, Serialize)]
struct DesktopEffort {
    original: Field,
    managed: Value,
}

pub(crate) enum Target<'a> {
    Profile(&'a str, Option<Connection>),
    Official,
    Restore,
}

/// Desktop writes `config.toml`. CLI writes `codeneo-lite.config.toml` and is launched with `--profile`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    Desktop,
    Cli,
}

fn catalog_models(path: &Path) -> Result<Vec<String>, String> {
    let catalog = Plan::open(&catalog_path(path))?;
    Ok(catalog
        .get(None, "models")?
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model["slug"].as_str())
        .map(str::to_owned)
        .collect())
}

fn cli_config_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.strip_suffix(".config.toml") == Some(PROVIDER))
}

pub(crate) fn catalog_path(config: &Path) -> PathBuf {
    config.with_file_name(if cli_config_file(config) {
        CLI_CATALOG
    } else {
        CATALOG
    })
}

fn catalog_value(config: &Path) -> Result<Value, String> {
    let path = catalog_path(config);
    Ok(json!(path
        .to_str()
        .ok_or("Codex 模型目录路径需要使用 UTF-8 编码")?))
}

pub(crate) fn config_home() -> Result<PathBuf, String> {
    let home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|dir| dir.join(".codex")))
        .ok_or("找不到 Codex 的配置目录")?;
    if !home.is_absolute() {
        return Err("CODEX_HOME 必须是绝对路径".into());
    }
    Ok(home)
}

pub fn path() -> Result<PathBuf, String> {
    Ok(config_home()?.join("config.toml"))
}

pub(crate) fn cli_config_path() -> Result<PathBuf, String> {
    Ok(config_home()?.join(format!("{PROVIDER}.config.toml")))
}

pub(crate) fn config_path(surface: Surface) -> Result<PathBuf, String> {
    match surface {
        Surface::Desktop => path(),
        Surface::Cli => cli_config_path(),
    }
}

/// Profile name for `codex --profile`, only when the CLI overlay selects Lite.
pub(crate) fn cli_launch_profile() -> Option<&'static str> {
    let path = cli_config_path().ok()?;
    if !path.is_file() {
        return None;
    }
    let plan = Plan::open(&path).ok()?;
    (plan
        .get(None, "model_provider")
        .ok()
        .flatten()
        .and_then(Value::as_str)
        == Some(PROVIDER))
    .then_some(PROVIDER)
}

fn applied<'a>(store: &'a Store, surface: Surface) -> Option<&'a models::Applied> {
    match surface {
        Surface::Desktop => store.codex.as_ref(),
        Surface::Cli => store.codex_cli.as_ref(),
    }
}

fn saved_original<'a>(store: &'a Store, surface: Surface) -> Option<&'a Original> {
    match surface {
        Surface::Desktop => store.codex_original.as_ref(),
        Surface::Cli => store.codex_cli_original.as_ref(),
    }
}

fn applied_field(surface: Surface) -> &'static str {
    match surface {
        Surface::Desktop => "codex",
        Surface::Cli => "codex_cli",
    }
}

fn original_field(surface: Surface) -> &'static str {
    match surface {
        Surface::Desktop => "codex_original",
        Surface::Cli => "codex_cli_original",
    }
}

pub fn read_key(root: &Path, reference: &str) -> Result<String, String> {
    if reference.len() > 8192 {
        return Err("Codex 凭据引用无效，请在 Lite 重新应用模型".into());
    }
    let sealed = URL_SAFE_NO_PAD
        .decode(reference)
        .map_err(|_| "Codex 凭据引用无效")?;
    String::from_utf8(vault::unprotect(root, &sealed)?)
        .map_err(|_| "无法读取 Codex 模型凭据".into())
}

fn official_base(base: Option<&str>) -> bool {
    base.filter(|url| !url.is_empty()).is_none_or(|url| {
        http::provider_base(url).is_ok_and(|url| url.as_str() == "https://api.openai.com/")
    })
}

fn can_restore(
    original: &Original,
    path: &Path,
    provider: &str,
    override_base: Option<&str>,
) -> bool {
    original.path == path
        && (provider == PROVIDER
            || (original.official_override && provider == "openai" && override_base.is_none()))
}

pub fn view(store: &Store, path: &Path) -> Result<ClientView, String> {
    view_on(store, path, Surface::Desktop)
}

pub(crate) fn view_on(store: &Store, path: &Path, surface: Surface) -> Result<ClientView, String> {
    if !path.is_file() {
        return Ok(ClientView {
            account: "OpenAI 官方".into(),
            ..ClientView::official()
        });
    }
    let plan = Plan::open(path)?;
    let provider = plan
        .get(None, "model_provider")?
        .and_then(Value::as_str)
        .unwrap_or("openai");
    let model = plan
        .get(None, "model")?
        .and_then(Value::as_str)
        .unwrap_or("");
    let override_base = plan.get(None, "openai_base_url")?.and_then(Value::as_str);
    let can_restore = saved_original(store, surface)
        .is_some_and(|original| can_restore(original, path, provider, override_base));
    if provider == "openai" && official_base(override_base) {
        return Ok(ClientView {
            account: "OpenAI 官方".into(),
            model: model.into(),
            can_restore,
            ..ClientView::official()
        });
    }
    let definition = plan.get(Some(PROVIDERS), provider)?;
    let base = if provider == "openai" {
        override_base
    } else {
        definition.and_then(|value| value["base_url"].as_str())
    };
    let mut view = ClientView::third_party(base, model);
    if provider == PROVIDER && plan.get(None, CATALOG_SETTING)? == Some(&catalog_value(path)?) {
        match catalog_models(path) {
            Ok(entries) if entries.iter().any(|slug| slug == model) => view.models = entries,
            _ => view.issue = Some("Codex 原生模型目录无法读取，请重新应用档案。".into()),
        }
    }
    let port = gateway_selection(store, surface)
        .map_or(gateway::DEFAULT_PORT, codex_inference::Selection::port);
    view.local_gateway = provider == PROVIDER && base.and_then(gateway::codex_port) == Some(port);
    view.can_restore = can_restore;
    if provider == PROVIDER
        && applied(store, surface).is_some_and(|applied| {
            store.model(&applied.id).is_ok_and(|(saved, account)| {
                if let Some(selected) = &applied.codex_gateway {
                    view.local_gateway
                        && definition
                            .is_some_and(|value| value["auth"]["args"][1] == selected.reference())
                } else {
                    http::provider_api_model(&saved.model).as_deref() == Ok(model)
                        && base
                            == Some(
                                models::openai_endpoint(account, models::Protocol::Responses, "")
                                    .as_str(),
                            )
                }
            })
        })
    {
        view.applied(Client::Codex, store, applied(store, surface));
        if view.issue.is_none() && view.models.is_empty() {
            view.issue = Some("重新应用档案即可在 Codex 的模型列表中查看第三方模型。".into());
        }
    }
    Ok(view)
}

fn gateway_selection(store: &Store, surface: Surface) -> Option<&codex_inference::Selection> {
    applied(store, surface)?.codex_gateway.as_ref()
}

fn applied_gateway(
    root: &Path,
    path: &Path,
    store: &Store,
    surface: Surface,
) -> Result<Option<(Arc<codex_inference::Route>, u16)>, String> {
    let config = Plan::open(path)?;
    if config.get(None, "model_provider")?.and_then(Value::as_str) != Some(PROVIDER) {
        return Ok(None);
    }
    let Some(provider) = config.get(Some(PROVIDERS), PROVIDER)? else {
        return Ok(None);
    };
    let Some(port) = provider["base_url"].as_str().and_then(gateway::codex_port) else {
        return Ok(None);
    };
    let Some((applied, selection)) = applied(store, surface)
        .and_then(|applied| applied.codex_gateway.as_ref().map(|s| (applied, s)))
    else {
        return if port == gateway::DEFAULT_PORT {
            Err("找不到 Codex 网关的已应用档案，请重新应用。".into())
        } else {
            Ok(None)
        };
    };
    let (model, account) = store.model(&applied.id)?;
    let route = selection.route(root, model, account)?;
    if port != selection.port()
        || config.get(None, "model")?.and_then(Value::as_str) != Some(&route.model)
        || provider["wire_api"] != "responses"
        || provider["auth"]["args"] != json!([KEY_FLAG, selection.reference()])
        || provider["auth"]["command"].as_str() != Some(desktop::launcher()?.as_str())
    {
        return Err("Codex 网关配置已改变，请重新应用档案。".into());
    }
    Ok(Some((route, port)))
}

pub(crate) async fn sync_gateway(root: &Path) -> Result<(), String> {
    let active = (|| {
        let (_, store) = models::read(root)?;
        if let Ok(path) = path() {
            if let Some(route) = applied_gateway(root, &path, &store, Surface::Desktop)? {
                return Ok(Some(route));
            }
        } else if store
            .codex
            .as_ref()
            .is_some_and(|applied| applied.codex_gateway.is_some())
        {
            return Err(path().unwrap_err());
        }
        match cli_config_path() {
            Ok(path) if path.is_file() => applied_gateway(root, &path, &store, Surface::Cli),
            Ok(_) => Ok(None),
            Err(error)
                if store
                    .codex_cli
                    .as_ref()
                    .is_some_and(|applied| applied.codex_gateway.is_some()) =>
            {
                Err(error)
            }
            Err(_) => Ok(None),
        }
    })();
    let mut service = gateway::SERVICE.lock().await;
    match active {
        Ok(Some((route, preferred))) => {
            if let Err(error) =
                gateway::prepare_existing(&mut service, root, Client::Codex, preferred).await
            {
                service.select_codex(None);
                return Err(error);
            }
            service.select_codex(Some(route));
            Ok(())
        }
        other => {
            service.select_codex(None);
            other.map(|_| ())
        }
    }
}

pub(crate) async fn gateway_status(root: &Path, surface: Surface) -> Result<(), String> {
    let active = applied_gateway(root, &config_path(surface)?, &models::read(root)?.1, surface)?;
    if let Some((active, preferred)) = active {
        let service = gateway::SERVICE.lock().await;
        if let Some(port) = service.port().filter(|port| *port != preferred) {
            return Err(gateway::port_changed(Client::Codex, port));
        }
        if service.is_serving_codex(&active) {
            return Ok(());
        }
    }
    Err("CodeNeo Lite网关尚未就绪。请从 Lite 打开 Codex，或重新应用档案。".into())
}

struct Change {
    deltas: Vec<Delta>,
    route: Option<Arc<codex_inference::Route>>,
    hot: bool,
}

fn model_catalog(
    store: &Store,
    primary: &models::Model,
    account: &models::Account,
    using_gateway: bool,
    catalogs: &model_effort::Accounts,
) -> Result<Value, String> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    let candidates = store.models.iter().filter(|model| {
        !using_gateway && model.account == primary.account && model.id != primary.id
    });
    for model in std::iter::once(primary).chain(candidates) {
        let slug = http::provider_api_model(&model.model)?;
        if !seen.insert(slug.clone()) {
            continue;
        }
        let effort = model_effort::options(account, models::Protocol::Responses, &slug, catalogs)
            .within(&http::EffortLevel::ALL);
        let levels: Vec<_> = effort.iter().flat_map(|effort| &effort.levels)
            .map(|level| json!({"effort":level, "description": match level {
                http::EffortLevel::None => "Off",
                http::EffortLevel::Minimal => "Minimal reasoning",
                http::EffortLevel::Low => "Light reasoning",
                http::EffortLevel::Medium => "Balanced reasoning",
                http::EffortLevel::High => "Deeper reasoning",
                http::EffortLevel::XHigh => "Extra high reasoning",
                http::EffortLevel::Max => "Maximum reasoning",
                http::EffortLevel::Ultra => "Ultra reasoning",
            }})).collect();
        entries.push(json!({
            "slug": slug,
            "display_name": model.name,
            "description": account.name,
            "visibility": "list",
            "supported_in_api": true,
            "priority": entries.len(),
            "default_reasoning_level": effort.as_ref().and_then(model_effort::Options::fallback),
            "supported_reasoning_levels": levels,
            "shell_type": "default",
            "base_instructions": "You are a coding assistant. Work with the user in the current workspace.",
            "support_verbosity": false,
            "truncation_policy": {"mode": "tokens", "limit": 4000},
            "context_window": if model.model.ends_with("[1m]") { 1_000_000 } else { 32_768 },
            "experimental_supported_tools": [],
            "input_modalities": ["text"],
            "supports_search_tool": false,
        }));
    }
    Ok(json!(entries))
}

fn restore_catalog(config: &mut Plan, path: &Path, original: &Original) -> Result<(), String> {
    if config.get(None, CATALOG_SETTING)? == Some(&catalog_value(path)?) {
        if let Some(value) = original.catalog.as_ref() {
            config.set(None, CATALOG_SETTING, value.value().cloned())?;
        }
    }
    if let Some(pref) = &original.desktop_effort {
        if config.get(Some("desktop"), DESKTOP_EFFORTS)? == Some(&pref.managed) {
            config.set(Some("desktop"), DESKTOP_EFFORTS, pref.original.value().cloned())?;
        }
    }
    Ok(())
}

fn enable_desktop_efforts(config: &mut Plan, original: &mut Original, entries: &Value) -> Result<(), String> {
    let current = config.get(Some("desktop"), DESKTOP_EFFORTS)?.cloned();
    let levels: BTreeSet<_> = entries.as_array().into_iter().flatten()
        .filter_map(|model| model["supported_reasoning_levels"].as_array()).flatten()
        .filter_map(|entry| entry["effort"].as_str().and_then(http::EffortLevel::parse))
        .collect();
    if levels.is_empty() {
        if let Some(pref) = original.desktop_effort.take() {
            if current.as_ref() == Some(&pref.managed) {
                config.set(Some("desktop"), DESKTOP_EFFORTS, pref.original.value().cloned())?;
            }
        }
    } else {
        let previous = original.desktop_effort.take()
            .filter(|pref| current.as_ref() == Some(&pref.managed))
            .map_or_else(|| Field::from(current), |pref| pref.original);
        let managed = json!(levels);
        config.set(Some("desktop"), DESKTOP_EFFORTS, Some(managed.clone()))?;
        original.desktop_effort = Some(DesktopEffort { original: previous, managed });
    }
    Ok(())
}

fn plan_switch(
    root: &Path,
    path: &Path,
    store: &Store,
    target: Target<'_>,
    port: Option<u16>,
    catalogs: &model_effort::Accounts,
    surface: Surface,
) -> Result<Change, String> {
    let mut config = Plan::open(path)?;
    let mut state = Plan::open(&models::path(root))?;
    let current_applied = applied(store, surface);
    let current_original = saved_original(store, surface);
    let applied_key = applied_field(surface);
    let original_key = original_field(surface);
    if current_original.is_some_and(|original| original.path != path) {
        return Err("Codex 资料目录已变化，请回到原目录恢复配置后再切换".into());
    }
    let current_provider = config.get(None, "model_provider")?.cloned();
    let provider = current_provider
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or("openai");
    let existing = config.get(Some(PROVIDERS), PROVIDER)?.cloned();
    let original = || -> Result<Original, String> {
        Ok(Original {
            path: path.to_path_buf(),
            model: config.get(None, "model")?.cloned(),
            provider: current_provider.clone(),
            base_url: None,
            official_override: false,
            catalog: None,
            desktop_effort: None,
        })
    };
    let mut route = None;
    let mut hot = false;
    let mut deltas = Vec::new();
    let mut retired_catalog = None;
    if let Target::Profile(id, _) = target {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Codex, model, account)?;
        let native_catalog = catalog_value(path)?;
        let current_catalog = config.get(None, CATALOG_SETTING)?.cloned();
        if current_original.is_none() && existing.is_some() {
            return Err("Codex 已有名为 codeneo-lite 的服务配置，请先在 Codex 中核对".into());
        }
        let mut previous = current_original.cloned().map_or_else(original, Ok)?;
        if current_catalog.as_ref() != Some(&native_catalog) {
            previous.catalog = Some(current_catalog.into());
        } else if previous.catalog.is_none() {
            return Err("Codex 已在使用同名模型目录，请在 Codex 中核对配置。".into());
        }
        let fingerprint = models::route(model, account);
        let previous_gateway = current_applied.and_then(|value| value.codex_gateway.as_ref());
        let (base_url, reference, codex_gateway) = if let Some(port) = port {
            let mut selected =
                codex_inference::Selection::new(root, id, &fingerprint, previous_gateway)?;
            selected.set_port(port);
            let slug = http::provider_api_model(&model.model)?;
            selected.set_effort(model_effort::declared(account, models::Protocol::Responses, &slug, catalogs));
            route = Some(selected.route(root, model, account)?);
            (
                gateway::codex_url(port),
                selected.reference().to_owned(),
                Some(selected),
            )
        } else {
            let key = models::key(root, account)?;
            let previous = existing
                .as_ref()
                .filter(|_| previous_gateway.is_none())
                .and_then(|value| value["auth"]["args"][1].as_str());
            let reference = match previous
                .filter(|reference| read_key(root, reference).as_deref() == Ok(key.as_str()))
            {
                Some(reference) => reference.to_owned(),
                None => URL_SAFE_NO_PAD.encode(vault::protect(root, key.as_bytes())?),
            };
            (
                models::openai_endpoint(account, models::Protocol::Responses, ""),
                reference,
                None,
            )
        };
        let mut definition = json!({
            "name": account.name,
            "base_url": base_url,
            "wire_api": "responses",
            "supports_websockets": false,
            "auth": {"command": desktop::launcher()?, "args": [KEY_FLAG, reference]},
        });
        if route.is_some() {
            definition["name"] = json!("CodeNeo Lite网关");
            definition["capabilities"] = json!({"remote_compaction": "unsupported"});
            hot = provider == PROVIDER && existing.as_ref() == Some(&definition);
        }
        let mut catalog = Plan::open(&catalog_path(path))?;
        let owner = catalog.get(None, "managed_by")?;
        if owner != Some(&json!(PROVIDER))
            && (owner.is_some() || catalog.get(None, "models")?.is_some())
        {
            return Err("Codex 已存在同名模型目录文件，请先核对该文件。".into());
        }
        catalog.set(None, "managed_by", Some(json!(PROVIDER)))?;
        let entries = model_catalog(store, model, account, route.is_some(), catalogs)?;
        if surface == Surface::Desktop {
            enable_desktop_efforts(&mut config, &mut previous, &entries)?;
        }
        state.set(None, original_key, Some(json!(previous)))?;
        catalog.set(None, "models", Some(entries))?;
        deltas.extend(catalog.finish());
        config.set(None, CATALOG_SETTING, Some(native_catalog))?;
        config.set(Some(PROVIDERS), PROVIDER, Some(definition))?;
        config.set(None, "model_provider", Some(json!(PROVIDER)))?;
        config.set(
            None,
            "model",
            Some(json!(http::provider_api_model(&model.model)?)),
        )?;
        state.set(
            None,
            applied_key,
            Some(json!(Applied {
                id: id.into(),
                route: fingerprint,
                gateway: None,
                codex_gateway,
            })),
        )?;
    } else if matches!(target, Target::Official) {
        let base = config.get(None, "openai_base_url")?.cloned();
        if provider != "openai" || !official_base(base.as_ref().and_then(Value::as_str)) {
            if provider == PROVIDER && current_original.is_none() {
                return Err("Codex 当前档案不受 Lite 管理，请在 Codex 中核对。".into());
            }
            let mut previous = match current_original {
                Some(value) => value.clone(),
                None => original()?,
            };
            if provider == PROVIDER {
                restore_catalog(&mut config, path, &previous)?;
            }
            retired_catalog =
                (provider == PROVIDER && previous.catalog.is_some()).then(|| catalog_path(path));
            let previous_base = previous
                .base_url
                .as_ref()
                .or(base.as_ref())
                .and_then(Value::as_str);
            if previous
                .provider
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or("openai")
                == "openai"
                && official_base(previous_base)
            {
                config.set(None, "model", previous.model)?;
                config.set(None, "model_provider", previous.provider)?;
                if let Some(value) = previous.base_url {
                    config.set(None, "openai_base_url", (!value.is_null()).then_some(value))?;
                }
                state.set(None, original_key, None)?;
            } else {
                if previous.base_url.is_none() {
                    previous.base_url = Some(base.unwrap_or(Value::Null));
                }
                previous.official_override = true;
                config.set(None, "model", None)?;
                config.set(None, "model_provider", Some(json!("openai")))?;
                config.set(None, "openai_base_url", None)?;
                state.set(None, original_key, Some(json!(previous)))?;
            }
            if provider == PROVIDER {
                config.set(Some(PROVIDERS), PROVIDER, None)?;
            }
            state.set(None, applied_key, None)?;
        }
    } else if let Some(original) = current_original {
        if !can_restore(
            original,
            path,
            provider,
            config.get(None, "openai_base_url")?.and_then(Value::as_str),
        ) {
            return Err("Codex 已在外部切换服务商，请核对配置后再操作".into());
        }
        config.set(None, "model", original.model.clone())?;
        config.set(None, "model_provider", original.provider.clone())?;
        if provider == PROVIDER {
            restore_catalog(&mut config, path, original)?;
        }
        retired_catalog =
            (provider == PROVIDER && original.catalog.is_some()).then(|| catalog_path(path));
        if let Some(value) = original.base_url.as_ref() {
            config.set(
                None,
                "openai_base_url",
                (!value.is_null()).then(|| value.clone()),
            )?;
        }
        if provider == PROVIDER {
            config.set(Some(PROVIDERS), PROVIDER, None)?;
        }
        state.set(None, applied_key, None)?;
        state.set(None, original_key, None)?;
    }
    deltas.extend([config.finish(), state.finish()].into_iter().flatten());
    if let Some(path) = retired_catalog {
        let mut catalog = Plan::open(&path)?;
        if catalog.get(None, "managed_by")? == Some(&json!(PROVIDER)) {
            catalog.set(None, "models", Some(json!([])))?;
            deltas.extend(catalog.finish());
        }
    }
    Ok(Change { deltas, route, hot })
}

pub(crate) async fn switch(
    root: &Path,
    target: Target<'_>,
    surface: Surface,
) -> Result<Outcome, String> {
    let _lock = crate::clients::lock_for_launch(root, Client::Codex)?;
    let path = config_path(surface)?;
    let (_, store) = models::read(root)?;
    let catalogs = if let Target::Profile(id, connection) = target {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Codex, model, account)?;
        let using_gateway = connection.map_or(gateway_selection(&store, surface).is_some(), |conn| {
            conn == Connection::Gateway
        });
        model_effort::discover(
            root, Client::Codex, &store,
            std::iter::once(model).chain(store.models.iter().filter(|candidate| {
                !using_gateway && candidate.account == model.account && candidate.id != id
            })),
        ).await
    } else {
        Default::default()
    };
    let mut service = gateway::SERVICE.lock().await;
    let port = if let Target::Profile(id, connection) = target {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Codex, model, account)?;
        let previous = gateway_selection(&store, surface);
        let use_gateway = connection.map_or(previous.is_some(), |conn| conn == Connection::Gateway);
        if use_gateway {
            Some(
                gateway::prepare(
                    &mut service,
                    root,
                    previous.map(codex_inference::Selection::port),
                )
                .await?,
            )
        } else {
            None
        }
    } else {
        None
    };
    let Change { deltas, route, hot } =
        plan_switch(root, &path, &store, target, port, &catalogs, surface)?;
    transaction::commit(root, Client::Codex, deltas, &crate::clients::allowed(root))?;
    service.select_codex(route);
    let message = match (surface, hot) {
        (Surface::Cli, true) => "Codex CLI 网关已切换模型。原会话的下一条请求使用新模型。",
        (Surface::Cli, false) => "已写入 Codex CLI 的独立配置，没有修改桌面应用。",
        (Surface::Desktop, true) => "Codex 网关已切换模型。原会话的下一条请求使用新模型。",
        (Surface::Desktop, false) => "已保存 Codex 桌面配置。完全退出并重新打开桌面应用后生效。",
    };
    Ok(Outcome {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn gateway_and_official_mode_preserve_the_external_provider_and_never_store_clear_keys() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let path = root.join("config.toml");
        fs::write(&path, "model = 'external-model'\nmodel_provider = 'external'\nopenai_base_url = 'https://other.invalid/v1'\n[model_providers.external]\nbase_url = 'https://original.invalid/v1'\n[mcp_servers.fixture]\ncommand = 'keep'\n").unwrap();
        let save = |model: &str| {
            models::save(root, serde_json::from_value(json!({
            "preset": "custom", "account_name": "Fixture", "base_url": "https://provider.invalid/custom/v1",
            "auth": "auth_token", "protocol": "responses", "name": model, "model": model, "key": "fixture-upstream-key"
        })).unwrap(), Some(Client::Codex)).unwrap()
        };
        let first = save("fixture-one");
        let second = save("fixture-two");
        let allowed = [path.clone(), models::path(root), catalog_path(&path)];
        let apply = |target, port, expected_hot| {
            let (_, store) = models::read(root).unwrap();
            let Change { deltas, route, hot } =
                plan_switch(root, &path, &store, target, port, &Default::default(), Surface::Desktop).unwrap();
            assert_eq!(hot, expected_hot);
            transaction::commit(root, Client::Codex, deltas, &allowed).unwrap();
            route
        };
        let a = apply(
            Target::Profile(&first, Some(Connection::Gateway)),
            Some(gateway::DEFAULT_PORT),
            false,
        )
        .unwrap();
        assert_eq!(
            a.endpoint.as_str(),
            "https://provider.invalid/custom/v1/responses"
        );
        assert_eq!(a.upstream_key, "fixture-upstream-key");
        assert_ne!(a.key, a.upstream_key);
        let bytes = fs::read_to_string(&path).unwrap();
        assert!(!bytes.contains(&a.key) && !bytes.contains(&a.upstream_key));
        let b = apply(
            Target::Profile(&second, Some(Connection::Gateway)),
            Some(gateway::DEFAULT_PORT),
            true,
        )
        .unwrap();
        assert_eq!(a.key, b.key);
        assert_eq!(b.model, "fixture-two");
        let alternate = 49321;
        let moved = apply(
            Target::Profile(&second, Some(Connection::Gateway)),
            Some(alternate),
            false,
        )
        .unwrap();
        assert_eq!(b.key, moved.key);
        let (_, store) = models::read(root).unwrap();
        assert_eq!(
            view(&store, &path).unwrap().selected.as_deref(),
            Some(second.as_str())
        );
        assert!(applied_gateway(root, &path, &store, Surface::Desktop)
            .unwrap()
            .is_some_and(|(active, port)| active.key == b.key && port == alternate));

        assert!(apply(Target::Official, None, false).is_none());
        let (_, store) = models::read(root).unwrap();
        let official = view(&store, &path).unwrap();
        assert_eq!(official.state, "official");
        assert!(official.can_restore && store.codex.is_none());
        assert!(applied_gateway(root, &path, &store, Surface::Desktop).unwrap().is_none());
        let config = Plan::open(&path).unwrap();
        assert!(config.get(None, "model").unwrap().is_none());
        assert!(config.get(None, "openai_base_url").unwrap().is_none());
        assert!(config.get(Some(PROVIDERS), "external").unwrap().is_some());
        assert!(config
            .get(Some("mcp_servers"), "fixture")
            .unwrap()
            .is_some());
        transaction::undo(root, Client::Codex, &allowed).unwrap();
        let (_, store) = models::read(root).unwrap();
        assert!(applied_gateway(root, &path, &store, Surface::Desktop)
            .unwrap()
            .is_some_and(|(active, port)| active.key == b.key && port == alternate));
        assert!(apply(Target::Official, None, false).is_none());
        assert!(apply(Target::Restore, None, false).is_none());
        let config = Plan::open(&path).unwrap();
        assert_eq!(
            config.get(None, "model").unwrap(),
            Some(&json!("external-model"))
        );
        assert_eq!(
            config.get(None, "openai_base_url").unwrap(),
            Some(&json!("https://other.invalid/v1"))
        );
        assert_eq!(
            config.get(None, "model_provider").unwrap(),
            Some(&json!("external"))
        );
        assert!(config.get(Some(PROVIDERS), PROVIDER).unwrap().is_none());
    }

    #[test]
    fn switch_preserves_toml_and_recovers_original_after_mcp_edit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("config.toml");
        fs::write(&path, "# existing preferences\nmodel = 'gpt-5.2' # comment\n[features]\nweb_search = false\n[mcp_servers.original]\nurl = 'https://example.invalid/mcp'\n").unwrap();
        let input = serde_json::from_value(json!({
            "id": null, "account_id": null, "preset": "custom", "account_name": "Fixture", "name": "",
            "base_url": "https://example.invalid/llm/v4", "auth": "auth_token", "protocol": "responses",
            "model": "gpt-6.1-sol", "key": "fixture-provider-key"
        })).unwrap();
        let id = models::save(root, input, None).unwrap();
        let paths = [path.clone(), models::path(root), catalog_path(&path)];
        let (_, store) = models::read(root).unwrap();
        let change = plan_switch(
            root,
            &path,
            &store,
            Target::Profile(&id, Some(Connection::Direct)),
            None,
            &Default::default(),
            Surface::Desktop,
        )
        .unwrap();
        transaction::commit(root, Client::Codex, change.deltas, &paths).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# existing preferences"));
        assert!(text.contains("# comment"));
        assert!(!text.contains("fixture-provider-key"));
        let mut config = Plan::open(&path).unwrap();
        let provider = config.get(Some(PROVIDERS), PROVIDER).unwrap().unwrap();
        assert_eq!(provider["base_url"], "https://example.invalid/llm/v4");
        assert_eq!(provider["wire_api"], "responses");
        let reference = provider["auth"]["args"][1].as_str().unwrap();
        assert_eq!(read_key(root, reference).unwrap(), "fixture-provider-key");
        config
            .set(
                Some("mcp_servers"),
                "extra",
                Some(json!({"command": "fixture"})),
            )
            .unwrap();
        transaction::write_single(config).unwrap();
        let (_, store) = models::read(root).unwrap();
        assert_eq!(
            view(&store, &path).unwrap().selected.as_deref(),
            Some(id.as_str())
        );
        let change = plan_switch(root, &path, &store, Target::Restore, None, &Default::default(), Surface::Desktop).unwrap();
        transaction::commit(root, Client::Codex, change.deltas, &paths).unwrap();
        let config = Plan::open(&path).unwrap();
        assert_eq!(config.get(None, "model").unwrap(), Some(&json!("gpt-5.2")));
        assert!(config.get(Some(PROVIDERS), PROVIDER).unwrap().is_none());
        assert!(config.get(Some("mcp_servers"), "extra").unwrap().is_some());
        assert!(config
            .get(Some("mcp_servers"), "original")
            .unwrap()
            .is_some());
    }

    #[test]
    fn direct_catalog_shows_same_account_models_and_restores_external_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config = root.join("config.toml");
        fs::write(&config, "model = 'gpt-5.2'\nmodel_catalog_json = 'user-catalog.json'\nmodel_reasoning_effort = 'high'\n[desktop]\nenabled-reasoning-efforts = ['low']\n").unwrap();
        let save = |model, key| {
            models::save(
                root,
                serde_json::from_value(json!({
                    "preset": "custom", "account_name": "Fixture", "base_url": "https://fixture.test/v1",
                    "auth": "auth_token", "protocol": "responses", "name": "", "model": model, "key": key,
                })).unwrap(),
                Some(Client::Codex),
            ).unwrap()
        };
        save("fixture-a", "fixture-token");
        let primary = save("provider/fixture-b", "fixture-token");
        save("different-account", "another-token");
        let catalog = catalog_path(&config);
        let allowed = [config.clone(), catalog.clone(), models::path(root)];
        let apply = |target| {
            let (_, store) = models::read(root).unwrap();
            let (_, account) = store.model(&primary).unwrap();
            let catalogs = [(account.id.clone(), [("provider/fixture-b".into(), model_effort::Options {
                levels: vec![http::EffortLevel::High, http::EffortLevel::Max],
                default: Some(http::EffortLevel::High),
            })].into())].into();
            let change = plan_switch(root, &config, &store, target, None, &catalogs, Surface::Desktop).unwrap();
            transaction::commit(root, Client::Codex, change.deltas, &allowed).unwrap();
        };
        apply(Target::Profile(&primary, Some(Connection::Direct)));
        let (_, store) = models::read(root).unwrap();
        assert_eq!(
            view(&store, &config).unwrap().models,
            ["provider/fixture-b", "fixture-a"]
        );
        let plan = Plan::open(&config).unwrap();
        assert_eq!(
            plan.get(None, "model_reasoning_effort").unwrap(),
            Some(&json!("high"))
        );
        assert_eq!(plan.get(Some("desktop"), DESKTOP_EFFORTS).unwrap(), Some(&json!(["high", "max"])));
        let check_catalog = |catalog: &Value| {
            assert_eq!(catalog["models"][0]["slug"], "provider/fixture-b");
            assert_eq!(catalog["models"][0]["default_reasoning_level"], "high");
            let levels = catalog["models"][0]["supported_reasoning_levels"].as_array().unwrap();
            assert_eq!(levels.iter().map(|level| level["effort"].as_str().unwrap()).collect::<Vec<_>>(), ["high", "max"]);
            assert_eq!(catalog["models"][1]["slug"], "fixture-a");
            assert_eq!(catalog["models"][1]["supported_reasoning_levels"], json!([]));
        };
        check_catalog(&serde_json::from_slice(&fs::read(&catalog).unwrap()).unwrap());
        apply(Target::Official);
        let plan = Plan::open(&config).unwrap();
        assert_eq!(
            plan.get(None, CATALOG_SETTING).unwrap(),
            Some(&json!("user-catalog.json"))
        );
        assert_eq!(
            plan.get(None, "model_reasoning_effort").unwrap(),
            Some(&json!("high"))
        );
        assert_eq!(plan.get(Some("desktop"), DESKTOP_EFFORTS).unwrap(), Some(&json!(["low"])));
        assert_eq!(
            Plan::open(&catalog).unwrap().get(None, "models").unwrap(),
            Some(&json!([]))
        );
        transaction::undo(root, Client::Codex, &allowed).unwrap();
        let (_, store) = models::read(root).unwrap();
        assert_eq!(
            view(&store, &config).unwrap().models,
            ["provider/fixture-b", "fixture-a"]
        );
        if let Some(binary) = env::var_os("CODENEO_TEST_CODEX") {
            let output = std::process::Command::new(binary)
                .args(["debug", "models"])
                .current_dir(root)
                .env_clear()
                .env("HOME", root)
                .env("CODEX_HOME", root)
                .env("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
            check_catalog(&catalog);
        }
    }

    #[test]
    fn cli_profile_does_not_change_desktop_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let desktop = root.join("config.toml");
        fs::write(
            &desktop,
            "model = 'gpt-6-astra'\nmodel_reasoning_effort = 'max'\n[desktop]\nenabled-reasoning-efforts = ['low', 'medium', 'high']\n[mcp_servers.keep]\ncommand = 'keep'\n",
        )
        .unwrap();
        let overlay = root.join("codeneo-lite.config.toml");
        let id = models::save(
            root,
            serde_json::from_value(json!({
                "preset": "custom", "account_name": "Fixture", "base_url": "https://fixture.test/v1",
                "auth": "auth_token", "protocol": "responses", "name": "Fixture", "model": "gpt-6.1-sol",
                "key": "fixture-provider-key"
            }))
            .unwrap(),
            Some(Client::Codex),
        )
        .unwrap();
        let before = fs::read_to_string(&desktop).unwrap();
        let allowed = [
            desktop.clone(),
            overlay.clone(),
            catalog_path(&overlay),
            models::path(root),
        ];
        let (_, store) = models::read(root).unwrap();
        let change = plan_switch(
            root,
            &overlay,
            &store,
            Target::Profile(&id, Some(Connection::Direct)),
            None,
            &Default::default(),
            Surface::Cli,
        )
        .unwrap();
        transaction::commit(root, Client::Codex, change.deltas, &allowed).unwrap();
        assert_eq!(fs::read_to_string(&desktop).unwrap(), before);
        let overlay_text = fs::read_to_string(&overlay).unwrap();
        assert!(overlay_text.contains("model_provider = \"codeneo-lite\"") || overlay_text.contains("model_provider = 'codeneo-lite'"));
        assert!(!overlay_text.contains("fixture-provider-key"));
        assert!(overlay_text.contains("gpt-6.1-sol"));
        let (_, store) = models::read(root).unwrap();
        assert!(store.codex.is_none());
        assert_eq!(store.codex_cli.as_ref().map(|applied| applied.id.as_str()), Some(id.as_str()));
        assert_eq!(view_on(&store, &overlay, Surface::Cli).unwrap().selected.as_deref(), Some(id.as_str()));
        let change = plan_switch(
            root,
            &overlay,
            &store,
            Target::Restore,
            None,
            &Default::default(),
            Surface::Cli,
        )
        .unwrap();
        transaction::commit(root, Client::Codex, change.deltas, &allowed).unwrap();
        assert_eq!(fs::read_to_string(&desktop).unwrap(), before);
        let restored = Plan::open(&overlay).unwrap();
        assert!(restored.get(None, "model_provider").unwrap().is_none());
        assert!(restored.get(None, "model").unwrap().is_none());
    }
}
