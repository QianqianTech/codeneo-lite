//! A separate, authenticated Responses route on the shared local listener.
use crate::{model_effort, model_transport, models, vault};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
};

type DigestBytes = [u8; 32];
type ReasoningOwner = (DigestBytes, DigestBytes);
const MAX_REASONING_ITEMS: usize = 4096;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Selection {
    profile_id: String,
    fingerprint: String,
    sealed_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<model_effort::Options>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Route {
    pub endpoint: reqwest::Url,
    pub model: String,
    pub key: String,
    pub upstream_key: String,
    pub effort: Option<model_effort::Options>,
}

impl Selection {
    pub(crate) fn new(
        root: &Path,
        profile_id: &str,
        fingerprint: &str,
        previous: Option<&Self>,
    ) -> Result<Self, String> {
        let sealed_key = match previous.filter(|s| gateway_key(root, s.reference()).is_ok()) {
            Some(previous) => previous.sealed_key.clone(),
            None => {
                let key = format!("{}{}", vault::id()?, vault::id()?);
                URL_SAFE_NO_PAD.encode(vault::protect(root, key.as_bytes())?)
            }
        };
        Ok(Self {
            profile_id: profile_id.into(),
            fingerprint: fingerprint.into(),
            sealed_key,
            port: previous.and_then(|selection| selection.port),
            effort: None,
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

    pub(crate) fn set_effort(&mut self, options: Option<model_effort::Options>) {
        self.effort = options;
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
            effort: self.effort.clone(),
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
    pub(crate) fn prepare_effort(&self, payload: &mut Value) {
        if self.effort.is_none() && payload["model"].as_str() == Some(self.model.as_str()) {
            return;
        }
        let Some(reasoning) = payload.get_mut("reasoning").and_then(Value::as_object_mut) else {
            return;
        };
        let Some(value) = reasoning.get("effort").and_then(Value::as_str) else {
            return;
        };
        let level = model_transport::EffortLevel::parse(value);
        let supported = level.and_then(|level| self.effort.as_ref()?.nearest(level));
        if supported == level && supported.is_some() {
            return;
        }
        if let Some(supported) = supported {
            reasoning.insert("effort".into(), Value::from(supported.as_str()));
        } else {
            reasoning.remove("effort");
        }
        if reasoning.is_empty() {
            payload.as_object_mut().expect("object with reasoning").remove("reasoning");
        }
    }

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

    fn context_scope(&self) -> DigestBytes {
        let mut digest = Sha256::new();
        for field in [
            self.endpoint.as_str(),
            &self.model,
            &self.upstream_key,
            &self.key,
        ] {
            digest.update((field.len() as u64).to_le_bytes());
            digest.update(field.as_bytes());
        }
        digest.finalize().into()
    }

    pub(crate) fn request(
        &self,
        client: &reqwest::Client,
        payload: &Value,
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

#[derive(Default)]
pub(crate) struct ReasoningHistory {
    owners: BTreeSet<ReasoningOwner>,
    order: VecDeque<ReasoningOwner>,
}

fn reasoning_digest(item: &Value) -> Option<DigestBytes> {
    let kind = item["type"].as_str()?;
    if !matches!(kind, "reasoning" | "compaction") {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    if let Some(opaque) = item["encrypted_content"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        digest.update(opaque.as_bytes());
    } else {
        digest.update(item["summary"].to_string());
        digest.update([0]);
        digest.update(item["content"].to_string());
    }
    Some(digest.finalize().into())
}

impl ReasoningHistory {
    fn remember(&mut self, scope: DigestBytes, item: &Value) {
        if let Some(digest) = reasoning_digest(item) {
            let owner = (scope, digest);
            if self.owners.insert(owner) {
                self.order.push_back(owner);
                if self.order.len() > MAX_REASONING_ITEMS {
                    self.owners.remove(&self.order.pop_front().unwrap());
                }
            }
        }
    }
}

pub(crate) struct ReasoningScope {
    history: Arc<Mutex<ReasoningHistory>>,
    route: DigestBytes,
}

impl ReasoningScope {
    pub(crate) fn new(history: Arc<Mutex<ReasoningHistory>>, route: &Route) -> Self {
        Self {
            history,
            route: route.context_scope(),
        }
    }

    pub(crate) fn prepare(&self, payload: &mut Value) {
        // Codex replays its input history. Provider-issued state is reusable
        // only on the model and account that produced it.
        if let Some(input) = payload.get_mut("input").and_then(Value::as_array_mut) {
            let history = self.history.lock().expect("Codex reasoning history");
            input.retain(|item| {
                reasoning_digest(item)
                    .is_none_or(|digest| history.owners.contains(&(self.route, digest)))
            });
        }
    }

    pub(crate) fn observe(&self, data: &[u8]) {
        if memchr::memmem::find(data, b"\"reasoning\"").is_none()
            && memchr::memmem::find(data, b"\"compaction\"").is_none()
        {
            return;
        }
        let Ok(event) = serde_json::from_slice::<Value>(data) else {
            return;
        };
        let mut history = self.history.lock().expect("Codex reasoning history");
        match event["type"].as_str() {
            Some("response.output_item.done") => history.remember(self.route, &event["item"]),
            Some("response.completed") => {
                if let Some(output) = event["response"]["output"].as_array() {
                    for item in output {
                        history.remember(self.route, item);
                    }
                }
            }
            _ => {}
        }
    }
}
