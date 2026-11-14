//! A separate, authenticated Responses route on the shared local listener.
use crate::{model_transport, models, vault};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Selection {
    profile_id: String,
    fingerprint: String,
    sealed_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Route {
    pub endpoint: reqwest::Url,
    pub model: String,
    pub key: String,
    pub upstream_key: String,
}

impl Selection {
    pub(crate) fn new(
        root: &Path,
        profile_id: &str,
        fingerprint: &str,
        previous: Option<&Self>,
    ) -> Result<Self, String> {
        if let Some(previous) = previous.filter(|s| {
            s.profile_id == profile_id
                && s.fingerprint == fingerprint
                && gateway_key(root, s.reference()).is_ok()
        }) {
            return Ok(previous.clone());
        }
        let key = format!("{}{}", vault::id()?, vault::id()?);
        Ok(Self {
            profile_id: profile_id.into(),
            fingerprint: fingerprint.into(),
            sealed_key: URL_SAFE_NO_PAD.encode(vault::protect(root, key.as_bytes())?),
            port: None,
        })
    }

    pub(crate) fn reference(&self) -> &str {
        &self.sealed_key
    }

    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or(crate::gateway::DEFAULT_PORT)
    }

    pub(crate) fn set_port(&mut self, port: u16) {
        self.port = (port != crate::gateway::DEFAULT_PORT).then_some(port);
    }

    pub(crate) fn route(
        &self,
        root: &Path,
        model: &models::Model,
        account: &models::Account,
    ) -> Result<Arc<Route>, String> {
        if model.id != self.profile_id || models::route(model, account) != self.fingerprint {
            return Err("Codex 网关档案已更新，请重新应用。".into());
        }
        let endpoint = models::openai_endpoint(account, models::Protocol::Responses, "/responses");
        model_transport::provider_base(&endpoint)?;
        let endpoint: reqwest::Url = endpoint.parse().map_err(|_| "Codex 模型接口地址无效")?;
        Ok(Arc::new(Route {
            endpoint,
            model: model_transport::provider_api_model(&model.model)?,
            key: gateway_key(root, self.reference())?,
            upstream_key: models::key(root, account)?,
        }))
    }
}

fn gateway_key(root: &Path, reference: &str) -> Result<String, String> {
    let key = crate::codex::read_key(root, reference)?;
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Codex 的 CodeNeo Lite网关密钥无效，请重新应用档案。".into());
    }
    Ok(key)
}

impl Route {
    pub(crate) fn authorizes(&self, token: Option<&str>) -> bool {
        token.is_some_and(|token| {
            // Compare HMACs with ring's constant-time verifier.
            let expected = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.key.as_bytes());
            let supplied = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, token.as_bytes());
            let context = b"codeneo-lite codex gateway v1";
            let tag = ring::hmac::sign(&supplied, context);
            ring::hmac::verify(&expected, context, tag.as_ref()).is_ok()
        })
    }

    pub(crate) fn request(
        &self,
        client: &reqwest::Client,
        payload: &serde_json::Value,
    ) -> reqwest::RequestBuilder {
        client
            .post(self.endpoint.clone())
            .bearer_auth(&self.upstream_key)
            .header(
                "accept",
                if payload["stream"] == true {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .json(payload)
    }
}
