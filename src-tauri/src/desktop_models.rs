use crate::{
    clients::{self, DesktopFiles, OWNED},
    desktop, desktop_effort, desktop_inference, files,
    gateway::{self, SERVICE},
    model_transport, models,
    transaction::{self, Client, Plan},
};
use claudekeeper_claude::normalize_desktop_models;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

#[derive(Clone, Serialize, Deserialize)]
pub struct Selection {
    models: Vec<String>,
    supplier: String,
    config_hash: String,
    allow_history: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    scoped_model_ids: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    picker_metadata: bool,
    #[serde(default, skip_serializing_if = "desktop_effort::Catalog::is_empty")]
    effort_catalog: desktop_effort::Catalog,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
}

impl Selection {
    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or(gateway::DEFAULT_PORT)
    }

    fn inference_route(
        &self,
        id: &str,
        account: &models::Account,
        key: String,
    ) -> Result<Arc<desktop_inference::Route>, String> {
        route(
            account,
            key,
            &self.models,
            self.scoped_model_ids.then_some(id),
            self.picker_metadata,
            self.effort_catalog.clone(),
        )
    }

    pub(crate) fn replace_supplier_key(
        &mut self,
        id: &str,
        account: &models::Account,
        old_key: String,
        new_key: String,
    ) -> Result<bool, String> {
        if self.inference_route(id, account, old_key)?.supplier_fingerprint() != self.supplier {
            return Ok(false);
        }
        self.supplier = self.inference_route(id, account, new_key)?.supplier_fingerprint();
        self.allow_history = false;
        Ok(true)
    }
}

pub(crate) fn model_names(model: &models::Model) -> Result<Vec<String>, String> {
    let names = if model.desktop_models.is_empty() {
        std::slice::from_ref(&model.model)
    } else {
        model.desktop_models.as_slice()
    };
    normalize_desktop_models(names).map_err(|error| error.to_string())
}

pub(crate) fn supports_direct(model: &models::Model) -> bool {
    model_names(model).is_ok_and(|names| {
        names.iter().all(|name| {
            let id = name.to_ascii_lowercase();
            id.starts_with("claude-") || id.starts_with("anthropic/claude-")
        })
    })
}

fn route(
    account: &models::Account,
    secret: String,
    names: &[String],
    profile: Option<&str>,
    picker_metadata: bool,
    effort_catalog: desktop_effort::Catalog,
) -> Result<Arc<desktop_inference::Route>, String> {
    let mut names = names.iter();
    let model = names
        .next()
        .ok_or("请填写至少一个 Desktop 模型 ID。")?
        .clone();
    let scope = profile.map(|profile| (profile, model.as_str()));
    let additional_models = names
        .cloned()
        .map(|name| desktop_inference::GatewayModel::new(name, scope, picker_metadata))
        .collect();
    Ok(Arc::new(desktop_inference::Route {
        profile_id: account.id.clone(),
        base: model_transport::provider_base(account.base_for(models::Protocol::Anthropic))?,
        api_key: account.auth_for(models::Protocol::Anthropic)
            == claudekeeper_claude::AuthMode::ApiKey,
        secret,
        model,
        additional_models,
        picker_metadata,
        effort_catalog,
    }))
}

fn config_hash(config: &Value, expected: &Value) -> Result<String, String> {
    // Desktop can seal the loopback token after reading it. Accept that form
    // only for our listener. Requests still need the original token.
    let mut fields = expected.clone();
    for (key, value) in fields.as_object_mut().ok_or("Lite 网关配置无效")? {
        let found = &config[key.as_str()];
        if key == "inferenceGatewayApiKey"
            && config["inferenceGatewayBaseUrl"] == expected["inferenceGatewayBaseUrl"]
            && config["inferenceGatewayAuthScheme"] == "x-api-key"
            && found.as_str().is_some_and(is_sealed_key)
        {
            continue;
        }
        *value = found.clone();
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(transaction::json_bytes(&fields)?)
    ))
}

pub fn configuration(
    root: &Path,
    paths: &DesktopFiles,
    previous: Option<&models::Applied>,
    account: &models::Account,
    model: &models::Model,
    applied_id: Option<&str>,
    port: u16,
    discovered: Option<desktop_effort::Catalog>,
) -> Result<(Value, Selection), String> {
    let key = gateway::desktop_key(root, true)?;
    let names = model_names(model)?;
    let effort_catalog = discovered.unwrap_or_else(|| {
        previous
            .filter(|applied| {
                applied.id == model.id
                    && applied.route == models::client_route(Client::Desktop, model, account)
            })
            .and_then(|applied| applied.gateway.as_ref())
            .map_or_else(desktop_effort::Catalog::new, |old| old.effort_catalog.clone())
    });
    // Keep alias digests when upgrading the currently applied model list.
    let scoped_model_ids = !(applied_id == Some(OWNED)
        && previous
            .filter(|applied| applied.id == model.id)
            .and_then(|applied| applied.gateway.as_ref())
            .is_some_and(|old| !old.scoped_model_ids && old.models == names));
    let target = route(
        account,
        models::key(root, account)?,
        &names,
        scoped_model_ids.then_some(model.id.as_str()),
        true,
        effort_catalog.clone(),
    )?;
    let mut config = target.desktop_config(&gateway::desktop_url(port), &key);
    let supplier = target.supplier_fingerprint();
    let mut allow_history = false;
    let hash = config_hash(&config, &config)?;
    if applied_id == Some(OWNED) {
        if let Some(old) = previous.and_then(|row| row.gateway.as_ref()) {
            let existing = json!(transaction::decode(files::read(&paths.owned)?.as_deref())?);
            let expected = target.desktop_config(&gateway::desktop_url(old.port()), &key);
            if config_hash(&existing, &expected)? == old.config_hash {
                allow_history = old.supplier == supplier && old.allow_history;
                if existing["inferenceGatewayApiKey"]
                    .as_str()
                    .is_some_and(is_sealed_key)
                {
                    config["inferenceGatewayApiKey"] = existing["inferenceGatewayApiKey"].clone();
                }
            }
        }
    }
    let selection = Selection {
        models: names,
        supplier,
        config_hash: hash,
        allow_history,
        scoped_model_ids,
        picker_metadata: true,
        effort_catalog,
        port: (port != gateway::DEFAULT_PORT).then_some(port),
    };
    Ok((config, selection))
}

fn is_sealed_key(key: &str) -> bool {
    key.starts_with("__sealed__:v1:") && key.len() > 14
}

struct Ready {
    route: Arc<desktop_inference::Route>,
    allow_history: bool,
    port: u16,
}

fn applied_route(
    root: &Path,
    paths: &DesktopFiles,
    store: &models::Store,
) -> Result<Option<Ready>, String> {
    if !clients::desktop_gateway_active(paths)? {
        return Ok(None);
    }
    if Plan::open(&paths.meta)?
        .get(None, "appliedId")?
        .and_then(Value::as_str)
        != Some(OWNED)
    {
        return Ok(None);
    }
    let config = json!(transaction::decode(files::read(&paths.owned)?.as_deref())?);
    let Some((applied, selection)) = store
        .desktop
        .as_ref()
        .and_then(|item| item.gateway.as_ref().map(|selection| (item, selection)))
    else {
        return if config["inferenceGatewayBaseUrl"] == gateway::desktop_url(gateway::DEFAULT_PORT) {
            Err("找不到 CodeNeo Lite网关的已应用档案，请重新应用模型。".into())
        } else {
            Ok(None)
        };
    };
    let (_, account) = store.applied_model(&applied.id)?;
    let route = selection.inference_route(
        &applied.id,
        account,
        models::key(root, account)?,
    )?;
    if route.supplier_fingerprint() != selection.supplier {
        return Err("已应用的模型服务商或 API Key 已更新，请重新应用档案。".into());
    }
    let key = gateway::desktop_key(root, false)?;
    let port = selection.port();
    if config_hash(
        &config,
        &route.desktop_config(&gateway::desktop_url(port), &key),
    )? != selection.config_hash
    {
        return Err("Claude Desktop 的网关配置已变更，请重新应用档案。".into());
    }
    Ok(Some(Ready {
        route,
        allow_history: selection.allow_history,
        port,
    }))
}

pub(crate) fn direct_configuration(
    profile: &claudekeeper_claude::Profile,
    key: &str,
) -> Result<Value, String> {
    let mut config = claudekeeper_claude::desktop_gateway_config(profile, key, 0)?;
    let base = model_transport::provider_base(&profile.base_url)?;
    desktop_effort::annotate_direct(&base, &mut config);
    Ok(config)
}

pub(crate) async fn discover_efforts(
    root: &Path,
    account: &models::Account,
    model: &models::Model,
) -> Result<Option<desktop_effort::Catalog>, String> {
    let base = model_transport::provider_base(account.base_for(models::Protocol::Anthropic))?;
    let auth = account.auth_for(models::Protocol::Anthropic);
    let key = models::key(root, account)?;
    let names = model_names(model)?;
    Ok(desktop_effort::discover(&base, auth, &key, &names).await)
}

pub async fn status(root: &Path) -> Result<bool, String> {
    let (_, store) = models::read(root)?;
    let paths = DesktopFiles::new(desktop::locations()?);
    let Some(active) = applied_route(root, &paths, &store)? else {
        return Ok(false);
    };
    let service = SERVICE.lock().await;
    if let Some(port) = service.port().filter(|port| *port != active.port) {
        return Err(gateway::port_changed(Client::Desktop, port));
    }
    if !service.is_serving(&active.route) {
        return Err(
            "CodeNeo Lite网关尚未就绪。请从 Lite 打开 Claude Desktop，或重新应用档案。".into(),
        );
    }
    Ok(service.verified(&active.route))
}

pub async fn sync(root: &Path) -> Result<(), String> {
    let mut service = SERVICE.lock().await;
    let (_, store) = models::read(root)?;
    let paths = match desktop::locations() {
        Ok(locations) => DesktopFiles::new(locations),
        Err(error) => {
            service.select(None, false);
            return if store
                .desktop
                .as_ref()
                .is_some_and(|applied| applied.gateway.is_some())
            {
                Err(error)
            } else {
                Ok(())
            };
        }
    };
    let active = applied_route(root, &paths, &store);
    let Ok(Some(active)) = active else {
        service.select(None, false);
        return active.map(|_| ());
    };
    if let Err(error) =
        gateway::prepare_existing(&mut service, root, Client::Desktop, active.port).await
    {
        service.select(None, false);
        return Err(error);
    }
    service.select(Some(active.route), active.allow_history);
    Ok(())
}

pub async fn recover_session(root: &Path, request: u64) -> Result<String, String> {
    let _lock = clients::lock_for_launch(root, Client::Desktop)?;
    sync(root).await?;
    let paths = DesktopFiles::new(desktop::locations()?);
    let active = applied_route(root, &paths, &models::read(root)?.1)?
        .ok_or("Claude Desktop 当前未使用 CodeNeo Lite网关。")?;
    let service = SERVICE.lock().await;
    service.recover_session(&active.route, request)?;
    Ok("已授权接续。请回到 Claude Desktop 重试。".into())
}

#[cfg(test)]
pub(crate) fn labels_for_test(
    root: &Path,
    paths: &DesktopFiles,
    store: &models::Store,
) -> Result<Vec<String>, String> {
    let active = applied_route(root, paths, store)?.expect("applied Lite gateway");
    Ok(active
        .route
        .models()
        .map(|(_, name)| name.to_owned())
        .collect())
}
