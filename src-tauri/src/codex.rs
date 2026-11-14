use crate::{
    clients::{ClientView, Outcome},
    codex_inference, desktop,
    gateway::{self, Connection},
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
const CATALOG_SETTING: &str = "model_catalog_json";
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
}

pub(crate) enum Target<'a> {
    Profile(&'a str, Option<Connection>),
    Official,
    Restore,
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

pub(crate) fn catalog_path(config: &Path) -> PathBuf {
    config.with_file_name(CATALOG)
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
    let can_restore = store
        .codex_original
        .as_ref()
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
    let port =
        gateway_selection(store).map_or(gateway::DEFAULT_PORT, codex_inference::Selection::port);
    view.local_gateway = provider == PROVIDER && base.and_then(gateway::codex_port) == Some(port);
    view.can_restore = can_restore;
    if provider == PROVIDER
        && store.codex.as_ref().is_some_and(|applied| {
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
        view.applied(Client::Codex, store, store.codex.as_ref());
        if view.issue.is_none() && view.models.is_empty() {
            view.issue = Some("重新应用档案即可在 Codex 的模型列表中查看第三方模型。".into());
        }
    }
    Ok(view)
}

fn gateway_selection(store: &Store) -> Option<&codex_inference::Selection> {
    store.codex.as_ref()?.codex_gateway.as_ref()
}

fn applied_gateway(
    root: &Path,
    path: &Path,
    store: &Store,
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
    let Some((applied, selection)) = store
        .codex
        .as_ref()
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
        match path() {
            Ok(path) => applied_gateway(root, &path, &store),
            Err(error)
                if store
                    .codex
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

pub(crate) async fn gateway_status(root: &Path) -> Result<(), String> {
    let active = applied_gateway(root, &path()?, &models::read(root)?.1)?;
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
}

fn model_catalog(
    store: &Store,
    primary: &models::Model,
    account: &models::Account,
    using_gateway: bool,
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
        // Use a text-only catalog entry for endpoints without capability metadata.
        entries.push(json!({
            "slug": slug,
            "display_name": model.name,
            "description": account.name,
            "visibility": "list",
            "supported_in_api": true,
            "priority": entries.len(),
            "default_reasoning_level": "none",
            "supported_reasoning_levels": [{"effort": "none", "description": "Off"}],
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
    Ok(())
}

fn plan_switch(
    root: &Path,
    path: &Path,
    store: &Store,
    target: Target<'_>,
    port: Option<u16>,
) -> Result<Change, String> {
    let mut config = Plan::open(path)?;
    let mut state = Plan::open(&models::path(root))?;
    if store
        .codex_original
        .as_ref()
        .is_some_and(|original| original.path != path)
    {
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
        })
    };
    let mut route = None;
    let mut deltas = Vec::new();
    let mut retired_catalog = None;
    if let Target::Profile(id, _) = target {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Codex, model, account)?;
        let native_catalog = catalog_value(path)?;
        let current_catalog = config.get(None, CATALOG_SETTING)?.cloned();
        if store.codex_original.is_none() && existing.is_some() {
            return Err("Codex 已有名为 codeneo-lite 的服务配置，请先在 Codex 中核对".into());
        }
        let mut previous = store.codex_original.clone().map_or_else(original, Ok)?;
        if current_catalog.as_ref() != Some(&native_catalog) {
            previous.catalog = Some(current_catalog.into());
        } else if previous.catalog.is_none() {
            return Err("Codex 已在使用同名模型目录，请在 Codex 中核对配置。".into());
        }
        state.set(None, "codex_original", Some(json!(previous)))?;
        let fingerprint = models::route(model, account);
        let previous_gateway = store
            .codex
            .as_ref()
            .and_then(|value| value.codex_gateway.as_ref());
        let (base_url, reference, codex_gateway) = if let Some(port) = port {
            let mut selected =
                codex_inference::Selection::new(root, id, &fingerprint, previous_gateway)?;
            selected.set_port(port);
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
        }
        let mut catalog = Plan::open(&catalog_path(path))?;
        let owner = catalog.get(None, "managed_by")?;
        if owner != Some(&json!(PROVIDER))
            && (owner.is_some() || catalog.get(None, "models")?.is_some())
        {
            return Err("Codex 已存在同名模型目录文件，请先核对该文件。".into());
        }
        catalog.set(None, "managed_by", Some(json!(PROVIDER)))?;
        catalog.set(
            None,
            "models",
            Some(model_catalog(store, model, account, route.is_some())?),
        )?;
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
            "codex",
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
            if provider == PROVIDER && store.codex_original.is_none() {
                return Err("Codex 当前档案不受 Lite 管理，请在 Codex 中核对。".into());
            }
            let mut previous = match &store.codex_original {
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
                state.set(None, "codex_original", None)?;
            } else {
                if previous.base_url.is_none() {
                    previous.base_url = Some(base.unwrap_or(Value::Null));
                }
                previous.official_override = true;
                config.set(None, "model", None)?;
                config.set(None, "model_provider", Some(json!("openai")))?;
                config.set(None, "openai_base_url", None)?;
                state.set(None, "codex_original", Some(json!(previous)))?;
            }
            if provider == PROVIDER {
                config.set(Some(PROVIDERS), PROVIDER, None)?;
            }
            state.set(None, "codex", None)?;
        }
    } else if let Some(original) = &store.codex_original {
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
        state.set(None, "codex", None)?;
        state.set(None, "codex_original", None)?;
    }
    deltas.extend([config.finish(), state.finish()].into_iter().flatten());
    if let Some(path) = retired_catalog {
        let mut catalog = Plan::open(&path)?;
        if catalog.get(None, "managed_by")? == Some(&json!(PROVIDER)) {
            catalog.set(None, "models", Some(json!([])))?;
            deltas.extend(catalog.finish());
        }
    }
    Ok(Change { deltas, route })
}

pub(crate) async fn switch(root: &Path, target: Target<'_>) -> Result<Outcome, String> {
    let _lock = crate::clients::lock_for_launch(root, Client::Codex)?;
    let path = path()?;
    let (_, store) = models::read(root)?;
    let mut service = gateway::SERVICE.lock().await;
    let port = if let Target::Profile(id, connection) = target {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Codex, model, account)?;
        let previous = gateway_selection(&store);
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
    let Change { deltas, route } = plan_switch(root, &path, &store, target, port)?;
    transaction::commit(root, Client::Codex, deltas, &crate::clients::allowed(root))?;
    service.select_codex(route);
    Ok(Outcome {
        message: "已保存 Codex 用户配置。重新打开 Codex 或启动新 CLI 进程后使用。".into(),
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
        let apply = |target, port| {
            let (_, store) = models::read(root).unwrap();
            let Change { deltas, route } = plan_switch(root, &path, &store, target, port).unwrap();
            transaction::commit(root, Client::Codex, deltas, &allowed).unwrap();
            route
        };
        let a = apply(
            Target::Profile(&first, Some(Connection::Gateway)),
            Some(gateway::DEFAULT_PORT),
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
        let alternate = 49321;
        let b = apply(
            Target::Profile(&second, Some(Connection::Gateway)),
            Some(alternate),
        )
        .unwrap();
        assert_ne!(a.key, b.key);
        let (_, store) = models::read(root).unwrap();
        assert_eq!(
            view(&store, &path).unwrap().selected.as_deref(),
            Some(second.as_str())
        );
        assert!(applied_gateway(root, &path, &store)
            .unwrap()
            .is_some_and(|(active, port)| active.key == b.key && port == alternate));

        assert!(apply(Target::Official, None).is_none());
        let (_, store) = models::read(root).unwrap();
        let official = view(&store, &path).unwrap();
        assert_eq!(official.state, "official");
        assert!(official.can_restore && store.codex.is_none());
        assert!(applied_gateway(root, &path, &store).unwrap().is_none());
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
        assert!(applied_gateway(root, &path, &store)
            .unwrap()
            .is_some_and(|(active, port)| active.key == b.key && port == alternate));
        assert!(apply(Target::Official, None).is_none());
        assert!(apply(Target::Restore, None).is_none());
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
        let change = plan_switch(root, &path, &store, Target::Restore, None).unwrap();
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
        fs::write(&config, "model = 'gpt-5.2'\nmodel_catalog_json = 'user-catalog.json'\nmodel_reasoning_effort = 'high'\n").unwrap();
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
            let change = plan_switch(root, &config, &store, target, None).unwrap();
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
            assert_eq!(catalog["models"][0]["slug"], "provider/fixture-b");
            assert_eq!(catalog["models"][1]["slug"], "fixture-a");
        }
    }
}
