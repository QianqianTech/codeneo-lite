//! Shared loopback listener for Claude Desktop Messages and Codex Responses.
use crate::{codex_inference, files, gateway};
use claudekeeper_app::diagnostics::{record, record_context, LogContext};
use futures_util::stream;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, Limited, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    convert::Infallible,
    fs::{self, File},
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinHandle};

// Keep the default route compatible with existing Desktop configurations.
pub(crate) const MODEL: &str = "claude-sonnet-4-6";
const MAX_REQUEST: usize = 24 * 1024 * 1024;
const MAX_SESSIONS: usize = 512;
const RECOVERY_WINDOW: Duration = Duration::from_secs(300);
const LOG_SOURCE: &str = "desktop-gateway";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct GatewayModel {
    id: String,
    upstream: String,
}

impl GatewayModel {
    pub(crate) fn new(upstream: String) -> Self {
        // Desktop accepts a routing prefix before the compatibility model ID.
        // A digest gives each upstream model a stable route regardless of order.
        let id = format!("gateway/{:x}/{MODEL}", Sha256::digest(upstream.as_bytes()));
        Self { id, upstream }
    }

    fn is_route_id(id: &str) -> bool {
        id.strip_prefix("gateway/")
            .and_then(|id| id.split_once('/'))
            .is_some_and(|(digest, model)| {
                model == MODEL
                    && digest.len() == 64
                    && digest.bytes().all(|b| b.is_ascii_hexdigit())
            })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Route {
    pub(crate) profile_id: String,
    pub(crate) base: Url,
    pub(crate) model: String,
    pub(crate) additional_models: Vec<GatewayModel>,
    pub(crate) api_key: bool,
    pub(crate) secret: String,
}

impl Route {
    pub(crate) fn models(&self) -> impl Iterator<Item = (&str, &str)> {
        std::iter::once((MODEL, self.model.as_str())).chain(
            self.additional_models
                .iter()
                .map(|model| (model.id.as_str(), model.upstream.as_str())),
        )
    }

    pub(crate) fn desktop_config(&self, url: &str, key: &str) -> Value {
        let models: Vec<_> = self
            .models()
            .map(|(id, name)| json!({ "name": id, "labelOverride": name }))
            .collect();
        json!({
            "inferenceProvider": "gateway",
            "inferenceGatewayBaseUrl": url,
            "inferenceGatewayApiKey": key,
            "inferenceGatewayAuthScheme": "x-api-key",
            "inferenceModels": models,
            "modelDiscoveryEnabled": false,
            "isClaudeCodeForDesktopEnabled": true,
            "desktopHome": "standard"
        })
    }

    #[allow(dead_code)] // Full-app selection fingerprints include the model.
    pub(crate) fn fingerprint(&self) -> String {
        self.fingerprint_for_model(&self.model)
    }

    #[allow(dead_code)]
    pub(crate) fn fingerprint_for_model(&self, model: &str) -> String {
        self.digest(Some(model))
    }

    pub(crate) fn supplier_fingerprint(&self) -> String {
        self.digest(None)
    }

    fn digest(&self, model: Option<&str>) -> String {
        let mut digest = Sha256::new();
        digest.update([u8::from(self.api_key)]);
        for field in [self.profile_id.as_str(), self.base.as_str()]
            .into_iter()
            .chain(model)
            .chain([self.secret.as_str()])
        {
            digest.update((field.len() as u64).to_le_bytes());
            digest.update(field.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct SessionBinding {
    session: String,
    supplier: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    awaiting_recovery: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    thinking_suppliers: BTreeMap<String, Option<String>>,
}

impl SessionBinding {
    fn new(session: String, supplier: String) -> Self {
        Self {
            session,
            supplier,
            awaiting_recovery: false,
            thinking_suppliers: BTreeMap::new(),
        }
    }

    fn prepare_history(&self, request: &mut Value, supplier: &str) -> Option<Self> {
        let previous = (!self.awaiting_recovery).then_some(self.supplier.as_str());
        let changed = previous != Some(supplier);
        if !changed && self.thinking_suppliers.is_empty() {
            return None;
        }
        let mut updated = changed.then(|| Self {
            session: self.session.clone(),
            supplier: supplier.into(),
            awaiting_recovery: false,
            thinking_suppliers: self.thinking_suppliers.clone(),
        });
        if let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) {
            messages.retain_mut(|message| {
                if message["role"] != "assistant" {
                    return true;
                }
                let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
                    return true;
                };
                let original_len = content.len();
                content.retain(|block| {
                    let Some(digest) = thinking_digest(block) else {
                        return true;
                    };
                    if let Some(updated) = &mut updated {
                        let owner = updated
                            .thinking_suppliers
                            .entry(digest)
                            .or_insert_with(|| previous.map(str::to_owned));
                        return owner.as_deref() == Some(supplier);
                    }
                    self.thinking_suppliers
                        .get(&digest)
                        .is_none_or(|owner| owner.as_deref() == Some(supplier))
                });
                !content.is_empty() || original_len == 0
            });
        }
        updated
    }
}

fn thinking_digest(block: &Value) -> Option<String> {
    // Use the signed identifier when available. Desktop may trim the thinking
    // text or change its cache settings before replaying the same block.
    let (field, value) = match block["type"].as_str()? {
        "thinking" => block["signature"]
            .as_str()
            .filter(|signature| !signature.is_empty())
            .map_or_else(
                || ("thinking", &block["thinking"]),
                |_| ("signature", &block["signature"]),
            ),
        "redacted_thinking" => ("redacted_thinking", &block["data"]),
        _ => return None,
    };
    let mut digest = Sha256::new();
    digest.update(field.as_bytes());
    digest.update([0]);
    digest.update(value.as_str().unwrap_or_default().as_bytes());
    Some(format!("{:x}", digest.finalize()))
}

fn tool_continuation_without_thinking(request: &Value) -> bool {
    let Some([.., assistant, user]) = request["messages"].as_array().map(Vec::as_slice) else {
        return false;
    };
    assistant["role"] == "assistant"
        && user["role"] == "user"
        && user["content"]
            .as_array()
            .is_some_and(|content| content.iter().any(|block| block["type"] == "tool_result"))
        && assistant["content"].as_array().is_some_and(|content| {
            content.iter().any(|block| block["type"] == "tool_use")
                && !content.iter().any(|block| block["type"] == "thinking")
        })
}

struct PendingSession {
    request: u64,
    session: String,
    supplier: String,
    received: Instant,
}

struct Routing {
    storage: PathBuf,
    active: Option<Arc<Route>>,
    allow_unbound_history: bool,
    sessions: Vec<SessionBinding>,
    pending: Option<PendingSession>,
    verified: Option<Arc<Route>>,
}

enum RouteError {
    UnknownModel,
    Unbound { recoverable: bool },
    Session(&'static str),
}

struct RoutedRequest {
    route: Arc<Route>,
    model: String,
    binding: Option<SessionBinding>,
}

impl Routing {
    fn open(storage: &Path) -> Self {
        let sessions = match fs::read(storage) {
            Ok(bytes) => match serde_json::from_slice::<Vec<SessionBinding>>(&bytes) {
                Ok(sessions) if sessions.len() <= MAX_SESSIONS => sessions,
                _ => {
                    record(
                        "warn",
                        LOG_SOURCE,
                        "无法读取 Desktop 会话绑定，旧会话可手动接续。",
                    );
                    Vec::new()
                }
            },
            Err(error) => {
                if error.kind() != ErrorKind::NotFound {
                    record(
                        "warn",
                        LOG_SOURCE,
                        "无法打开 Desktop 会话绑定，旧会话可手动接续。",
                    );
                }
                Vec::new()
            }
        };
        Self {
            storage: storage.to_owned(),
            active: None,
            allow_unbound_history: false,
            sessions,
            pending: None,
            verified: None,
        }
    }

    fn bind(&mut self, binding: SessionBinding) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.session == binding.session)
        {
            self.pending = None;
        }
        self.sessions
            .retain(|previous| previous.session != binding.session);
        if self.sessions.len() == MAX_SESSIONS {
            self.sessions.remove(0);
        }
        self.sessions.push(binding);
        self.save_sessions();
    }

    fn save_sessions(&self) {
        if serde_json::to_vec(&self.sessions)
            .ok()
            .is_none_or(|bytes| claudekeeper_claude::write_atomic(&self.storage, &bytes).is_err())
        {
            record(
                "warn",
                LOG_SOURCE,
                "Desktop 会话绑定暂存于内存，无法保存到磁盘。",
            );
        }
    }

    fn recovery(&self, route: &Route) -> Option<&PendingSession> {
        self.pending.as_ref().filter(|pending| {
            self.active.as_deref() == Some(route)
                && pending.received.elapsed() < RECOVERY_WINDOW
                && pending.supplier == route.supplier_fingerprint()
        })
    }

    fn recover(&mut self, route: &Route, request: u64) -> Result<(), String> {
        let pending = self
            .recovery(route)
            .filter(|pending| pending.request == request)
            .ok_or("待接续会话已变更或过期，请在 Desktop 重试该会话，再刷新状态。")?;
        let mut binding = SessionBinding::new(pending.session.clone(), pending.supplier.clone());
        binding.awaiting_recovery = true;
        self.bind(binding);
        Ok(())
    }

    fn route(
        &mut self,
        request: &mut Value,
        inference: bool,
        log: &RequestLog,
    ) -> Result<RoutedRequest, RouteError> {
        let route = self
            .active
            .clone()
            .ok_or(RouteError::Session("请选择 Desktop 第三方档案"))?;
        let supplier = route.supplier_fingerprint();
        let session = request
            .pointer("/metadata/user_id")
            .and_then(Value::as_str)
            .filter(|raw| raw.len() <= 4096)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|meta| {
                meta.get("session_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= 128)
                    .map(|id| format!("{:x}", Sha256::digest(id.as_bytes())))
            });
        let fresh = request
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| {
                matches!(messages.as_slice(), [first] if first["role"] == "user" &&
                first.get("content").is_some_and(|content| {
                    content.is_string() || content.as_array().is_some_and(|parts| {
                        !parts.iter().any(|part| part["type"] == "tool_result")
                    })
                }))
            });
        let binding = session
            .as_ref()
            .and_then(|key| self.sessions.iter().find(|binding| binding.session == *key));
        let requested = request["model"].as_str().unwrap_or_default();
        let model = route
            .models()
            .find(|(id, _)| *id == requested)
            .map(|(_, name)| name)
            // A resumed thread can still carry a model ID from a previous list.
            .or_else(|| {
                ((!fresh || binding.is_some()) && GatewayModel::is_route_id(requested))
                    .then_some(route.model.as_str())
            })
            .ok_or(RouteError::UnknownModel)?
            .to_owned();
        if let Some(binding) =
            binding.filter(|binding| !binding.awaiting_recovery || binding.supplier == supplier)
        {
            let updated = binding.prepare_history(request, &supplier);
            if updated.is_some() && inference {
                log.record(
                    "info",
                    "会话路由",
                    "DESKTOP_SESSION_SWITCHED",
                    "续聊请求已切换到当前供应商档案。",
                );
            }
            // DeepSeek requires its previous thinking when continuing an open
            // tool turn. Disable thinking for that turn when replaying a tool
            // call and result without compatible thinking from the previous model.
            if (updated.is_some() || !binding.thinking_suppliers.is_empty())
                && route.base.host_str() == Some("api.deepseek.com")
                && tool_continuation_without_thinking(request)
            {
                request["thinking"] = json!({"type": "disabled"});
            }
            return Ok(RoutedRequest {
                route,
                model,
                binding: updated.filter(|_| inference),
            });
        }
        if !fresh {
            if self.allow_unbound_history
                && self
                    .sessions
                    .iter()
                    .all(|binding| binding.supplier == supplier)
            {
                log.record(
                    "info",
                    "会话路由",
                    "DESKTOP_SESSION_CONTINUED",
                    "旧会话沿用已应用的供应商档案。",
                );
            } else {
                let recoverable = session.is_some();
                if let Some(session) = session {
                    // Retries for the same session keep the confirmation valid.
                    let request = self
                        .recovery(&route)
                        .filter(|pending| pending.session == session)
                        .map_or(log.id, |pending| pending.request);
                    self.pending = Some(PendingSession {
                        request,
                        session,
                        supplier,
                        received: Instant::now(),
                    });
                }
                return Err(RouteError::Unbound { recoverable });
            }
        }
        if let Some(key) = session.filter(|_| inference) {
            self.bind(SessionBinding::new(key, supplier));
        }
        Ok(RoutedRequest {
            route,
            model,
            binding: None,
        })
    }
}

#[derive(Default)]
pub(crate) struct Service {
    listener: Option<Listener>,
}

struct Listener {
    key: String,
    address: std::net::SocketAddr,
    routing: Arc<Mutex<Routing>>,
    codex_route: Arc<Mutex<Option<Arc<codex_inference::Route>>>>,
    task: JoinHandle<()>,
    _lock: File,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
        if let Ok(mut routing) = self.routing.lock() {
            routing.active = None;
        }
        if let Ok(mut route) = self.codex_route.lock() {
            *route = None;
        }
    }
}

impl Service {
    #[allow(dead_code)]
    pub(crate) const fn new() -> Self {
        Self { listener: None }
    }

    fn live_listener(&self) -> Option<&Listener> {
        self.listener
            .as_ref()
            .filter(|listener| !listener.task.is_finished())
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.live_listener().map(|listener| listener.address.port())
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.live_listener().is_some_and(|listener| {
            let codex_ready = listener
                .codex_route
                .lock()
                .is_ok_and(|route| route.is_some());
            codex_ready
                || listener
                    .routing
                    .lock()
                    .is_ok_and(|routing| routing.active.is_some())
        })
    }

    pub(crate) async fn bind(
        &mut self,
        key: &str,
        preferred_port: u16,
        storage: &Path,
    ) -> Result<std::net::SocketAddr, String> {
        if let Some(listener) = &self.listener {
            if !listener.task.is_finished() && listener.key == key {
                return Ok(listener.address);
            }
        }
        self.listener = None;
        let root = storage.parent().ok_or("无法定位网关资料目录")?;
        files::private_dir(root)?;
        let lock = files::try_lock(&root.join("gateway.lock"))?
            .ok_or("已有 CodeNeo Lite 实例正在使用此资料目录的网关，请在原窗口操作。")?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, preferred_port);
        let listener = match TcpListener::bind(address).await {
            Err(error) if preferred_port != 0 && error.kind() == ErrorKind::AddrInUse => {
                record(
                    "info",
                    LOG_SOURCE,
                    &format!("本机端口 {preferred_port} 已被占用，正在分配新端口。"),
                );
                TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).await
            }
            result => result,
        }
        .map_err(|error| {
            let message = format!("CodeNeo Lite 网关无法监听本机端口（{error}）。");
            record("error", LOG_SOURCE, &message);
            message
        })?;
        let address = listener.local_addr().map_err(|_| "无法读取推理网关端口")?;
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|_| "无法初始化 CodeNeo Lite网关")?;
        let routing = Arc::new(Mutex::new(Routing::open(storage)));
        let codex_route = Arc::new(Mutex::new(None));
        let state = Arc::new(Gateway {
            client,
            key: key.to_owned(),
            routing: routing.clone(),
            codex_route: codex_route.clone(),
            requests: Arc::new(Semaphore::new(12)),
        });
        let task = tokio::spawn(async move {
            let connections = Arc::new(Semaphore::new(32));
            let timer = hyper_util::rt::TokioTimer::new();
            while let Ok((socket, _)) = listener.accept().await {
                let Ok(permit) = connections.clone().try_acquire_owned() else {
                    continue;
                };
                let state = state.clone();
                let timer = timer.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let service = service_fn(move |request| state.clone().handle(request));
                    let _ = http1::Builder::new()
                        .timer(timer)
                        .header_read_timeout(Duration::from_secs(10))
                        .keep_alive(false)
                        .serve_connection(TokioIo::new(socket), service)
                        .await;
                });
            }
        });
        self.listener = Some(Listener {
            key: key.to_owned(),
            routing,
            codex_route,
            task,
            address,
            _lock: lock,
        });
        record(
            "info",
            LOG_SOURCE,
            &format!("CodeNeo Lite网关正在监听端口 {}。", address.port()),
        );
        Ok(address)
    }

    pub(crate) fn select(
        &self,
        route: Option<Arc<Route>>,
        allow_unbound_history: bool,
    ) -> (Option<Arc<Route>>, bool) {
        let Some(listener) = &self.listener else {
            return (None, false);
        };
        let mut routing = listener.routing.lock().expect("Desktop inference");
        let previous = std::mem::replace(&mut routing.active, route);
        let previous_allow_unbound =
            std::mem::replace(&mut routing.allow_unbound_history, allow_unbound_history);
        if previous != routing.active {
            if previous.as_deref().map(Route::supplier_fingerprint)
                != routing.active.as_deref().map(Route::supplier_fingerprint)
            {
                routing.pending = None;
            }
            let message = if routing.active.is_some() {
                "本机推理网关已设置新会话的供应商路由。"
            } else {
                "本机推理网关已清除新会话的供应商路由。"
            };
            drop(routing);
            record("info", LOG_SOURCE, message);
        }
        (previous, previous_allow_unbound)
    }

    pub(crate) fn select_codex(&self, route: Option<Arc<codex_inference::Route>>) {
        if let Some(listener) = &self.listener {
            *listener.codex_route.lock().expect("Codex inference") = route;
        }
    }

    pub(crate) fn is_serving_codex(&self, route: &codex_inference::Route) -> bool {
        self.listener
            .as_ref()
            .filter(|listener| !listener.task.is_finished())
            .is_some_and(|listener| {
                listener
                    .codex_route
                    .lock()
                    .expect("Codex inference")
                    .as_deref()
                    == Some(route)
            })
    }

    pub(crate) fn verified(&self, route: &Route) -> bool {
        self.listener
            .as_ref()
            .filter(|listener| !listener.task.is_finished())
            .is_some_and(|listener| {
                let routing = listener.routing.lock().expect("Desktop inference");
                routing.verified.as_deref() == Some(route)
            })
    }

    #[allow(dead_code)]
    pub(crate) fn is_serving(&self, route: &Route) -> bool {
        self.listener
            .as_ref()
            .filter(|listener| !listener.task.is_finished())
            .is_some_and(|listener| {
                listener
                    .routing
                    .lock()
                    .expect("Desktop inference")
                    .active
                    .as_deref()
                    == Some(route)
            })
    }

    pub(crate) fn pending_session(&self, route: &Route) -> Option<u64> {
        let listener = self
            .listener
            .as_ref()
            .filter(|listener| !listener.task.is_finished())?;
        let routing = listener.routing.lock().expect("Desktop inference");
        routing.recovery(route).map(|pending| pending.request)
    }

    pub(crate) fn recover_session(&self, route: &Route, request: u64) -> Result<(), String> {
        let listener = self
            .listener
            .as_ref()
            .filter(|listener| !listener.task.is_finished())
            .ok_or("CodeNeo Lite网关尚未启动。")?;
        listener
            .routing
            .lock()
            .expect("Desktop inference")
            .recover(route, request)
    }
}

struct Gateway {
    client: Client,
    key: String,
    routing: Arc<Mutex<Routing>>,
    codex_route: Arc<Mutex<Option<Arc<codex_inference::Route>>>>,
    requests: Arc<Semaphore>,
}

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

fn json_response(status: StatusCode, data: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(
            Full::new(Bytes::from(data.to_string()))
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .expect("JSON response")
}

fn error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(
        status,
        json!({
            "type": "error", "error": {"type": "api_error", "message": message}
        }),
    )
}

fn non_retryable_error(status: StatusCode, kind: &str, message: &str) -> Response<Body> {
    let mut response = json_response(
        status,
        json!({"type": "error", "error": {"type": kind, "message": message}}),
    );
    response.headers_mut().insert(
        "x-should-retry",
        hyper::header::HeaderValue::from_static("false"),
    );
    response
}

struct RequestLog {
    id: u64,
    endpoint: &'static str,
    started: Instant,
}

impl RequestLog {
    fn new(method: &Method, path: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let endpoint = match (method.as_str(), path) {
            ("GET", "/v1/models") => "GET /v1/models",
            ("POST", "/v1/messages") => "POST /v1/messages",
            ("POST", "/v1/messages/count_tokens") => "POST /v1/messages/count_tokens",
            ("POST", gateway::CODEX_RESPONSES) => "POST /codex/v1/responses",
            _ => "未知推理接口",
        };
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            endpoint,
            started: Instant::now(),
        }
    }

    fn record(&self, level: &str, stage: &str, code: &str, message: &str) {
        record_context(
            level,
            LOG_SOURCE,
            &format!("{} · {message}", self.endpoint),
            LogContext {
                task_id: format!("gateway-request-{}-{}", std::process::id(), self.id),
                stage: stage.into(),
                code: code.into(),
                elapsed_ms: self.started.elapsed().as_millis() as u64,
            },
        );
    }

    fn reject(&self, status: StatusCode, code: &str, reason: &str) -> Response<Body> {
        self.record(
            "warn",
            "本机网关",
            code,
            &format!("HTTP {} · {reason}", status.as_u16()),
        );
        error(status, reason)
    }

    fn reject_session(&self, code: &str, reason: &str) -> Response<Body> {
        self.record("warn", "会话路由", code, &format!("HTTP 403 · {reason}"));
        non_retryable_error(StatusCode::FORBIDDEN, "permission_error", reason)
    }
}

#[derive(Default)]
struct SseEvents {
    line: Vec<u8>,
    long_line: bool,
    stopped: bool,
    failed: bool,
}

impl SseEvents {
    fn observe(&mut self, bytes: &[u8]) {
        for part in bytes.split_inclusive(|&byte| byte == b'\n') {
            if self.line.len() + part.len() > 64 {
                self.long_line = true;
            }
            if !self.long_line {
                self.line.extend_from_slice(part);
            }
            if part.ends_with(b"\n") {
                if !self.long_line {
                    match self.line.strip_prefix(b"event:").map(<[u8]>::trim_ascii) {
                        Some(b"message_stop" | b"response.completed") => self.stopped = true,
                        Some(b"error" | b"response.failed" | b"response.incomplete") => {
                            self.failed = true
                        }
                        _ => {}
                    }
                }
                self.line.clear();
                self.long_line = false;
            }
        }
    }
}

struct ForwardedBody {
    response: reqwest::Response,
    _permit: tokio::sync::OwnedSemaphorePermit,
    verify: Option<DesktopResponse>,
    log: RequestLog,
    has_data: bool,
    expected_format: bool,
    sse: Option<SseEvents>,
}

struct DesktopResponse {
    routing: Arc<Mutex<Routing>>,
    route: Arc<Route>,
    binding: Option<SessionBinding>,
}

impl ForwardedBody {
    fn finish(self) {
        let failure = if self.sse.as_ref().is_some_and(|sse| sse.failed) {
            Some(("DESKTOP_UPSTREAM_STREAM_ERROR", "上游推理流返回错误事件。"))
        } else if !self.has_data {
            Some(("DESKTOP_UPSTREAM_EMPTY_BODY", "上游响应体为空。"))
        } else if !self.expected_format {
            Some(("DESKTOP_UPSTREAM_FORMAT", "上游响应格式与请求不符。"))
        } else if self.sse.as_ref().is_some_and(|sse| !sse.stopped) {
            Some((
                "DESKTOP_UPSTREAM_STREAM_INCOMPLETE",
                "上游推理流在完成事件前结束。",
            ))
        } else {
            None
        };
        if let Some((code, message)) = failure {
            self.log.record("error", "上游响应", code, message);
            return;
        }
        self.log.record(
            "info",
            "上游响应",
            "DESKTOP_UPSTREAM_COMPLETE",
            "上游响应已完整转发。",
        );
        if let Some(verify) = self.verify {
            let mut routing = verify.routing.lock().expect("Desktop inference");
            if routing.active.as_ref() == Some(&verify.route) {
                if let Some(mut binding) = verify.binding {
                    // A completed request can race a second request for the
                    // same session. Keep provenance already recorded by it.
                    if let Some(current) = routing
                        .sessions
                        .iter()
                        .find(|row| row.session == binding.session)
                    {
                        binding
                            .thinking_suppliers
                            .extend(current.thinking_suppliers.clone());
                    }
                    routing.bind(binding);
                }
                routing.verified = Some(verify.route);
            }
        }
    }
}

impl Gateway {
    async fn handle(
        self: Arc<Self>,
        request: Request<Incoming>,
    ) -> Result<Response<Body>, Infallible> {
        Ok(self.forward(request).await)
    }

    async fn forward(&self, request: Request<Incoming>) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let path = parts.uri.path();
        let log = RequestLog::new(&parts.method, path);
        if path == gateway::CODEX_RESPONSES {
            return self.forward_codex(parts, body, log).await;
        }
        let supplied = parts
            .headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .or_else(|| {
                parts
                    .headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.strip_prefix("Bearer "))
            });
        if supplied != Some(self.key.as_str()) {
            return log.reject(
                StatusCode::UNAUTHORIZED,
                "DESKTOP_GATEWAY_AUTH",
                "CodeNeo Lite网关认证失败",
            );
        }
        if parts.method == Method::GET && path == "/v1/models" {
            let active = self
                .routing
                .lock()
                .expect("Desktop inference")
                .active
                .clone();
            let data: Vec<_> = active
                .iter()
                .flat_map(|route| route.models())
                .map(|(id, model)| {
                    json!({
                        "type": "model", "id": id, "display_name": model,
                        "created_at": "2026-01-01T00:00:00Z"
                    })
                })
                .collect();
            let first_id = data.first().and_then(|model| model["id"].as_str());
            let last_id = data.last().and_then(|model| model["id"].as_str());
            log.record(
                "info",
                "模型目录",
                "DESKTOP_GATEWAY_MODELS",
                &format!("本机网关返回 {} 个模型路由（HTTP 200）。", data.len()),
            );
            return json_response(
                StatusCode::OK,
                json!({"data": data, "has_more": false, "first_id": first_id, "last_id": last_id}),
            );
        }
        if parts.method != Method::POST
            || !matches!(path, "/v1/messages" | "/v1/messages/count_tokens")
        {
            return log.reject(
                StatusCode::NOT_FOUND,
                "DESKTOP_GATEWAY_ENDPOINT",
                "CodeNeo Lite网关只提供模型推理接口",
            );
        }
        let Ok(permit) = self.requests.clone().try_acquire_owned() else {
            return log.reject(
                StatusCode::TOO_MANY_REQUESTS,
                "DESKTOP_GATEWAY_BUSY",
                "CodeNeo Lite网关请求过多",
            );
        };
        log.record(
            "info",
            "本机网关",
            "DESKTOP_GATEWAY_RECEIVED",
            "本机网关收到请求。",
        );
        let mut payload = match inference_payload(body, &log).await {
            Ok(payload) => payload,
            Err(response) => return response,
        };
        let inference = path == "/v1/messages";
        let route =
            self.routing
                .lock()
                .expect("Desktop inference")
                .route(&mut payload, inference, &log);
        let RoutedRequest {
            route,
            model,
            binding,
        } = match route {
            Ok(target) => target,
            Err(RouteError::UnknownModel) => {
                return log.reject(
                    StatusCode::BAD_REQUEST,
                    "DESKTOP_GATEWAY_MODEL",
                    "当前供应商档案中没有此模型，请在 Desktop 选择已配置的模型。",
                )
            }
            Err(RouteError::Unbound { recoverable }) => {
                let (code, message) = if recoverable {
                    ("DESKTOP_SESSION_UNBOUND", "此旧会话需要接续授权。请在 CodeNeo Lite 刷新 Claude Desktop 状态，然后在「恢复与撤销」中选择「接续旧会话」。")
                } else {
                    (
                        "DESKTOP_SESSION_NO_ID",
                        "无法识别此旧会话，请在 Claude Desktop 新建会话。",
                    )
                };
                return log.reject_session(code, message);
            }
            Err(RouteError::Session(message)) => {
                return log.reject(StatusCode::CONFLICT, "DESKTOP_GATEWAY_ROUTE", message)
            }
        };
        let wants_stream = payload["stream"] == true;
        payload["model"] = json!(model);
        if let Some(object) = payload.as_object_mut() {
            object.remove("metadata");
        }
        let mut url = route.base.clone();
        url.set_path(&format!(
            "{}{path}",
            route.base.path().trim_end_matches('/')
        ));
        let mut request = self.client.post(url).json(&payload);
        for name in ["anthropic-version", "anthropic-beta"] {
            for value in parts.headers.get_all(name) {
                request = request.header(name, value);
            }
        }
        if !parts.headers.contains_key("anthropic-version") {
            request = request.header("anthropic-version", "2023-06-01");
        }
        request = if route.api_key {
            request.header("x-api-key", &route.secret)
        } else {
            request.bearer_auth(&route.secret)
        };
        log.record(
            "info",
            "上游请求",
            "DESKTOP_UPSTREAM_REQUEST",
            &claudekeeper_net::redact(
                &format!("正在调用供应商模型 {model}。"),
                &[&route.secret, &self.key],
            ),
        );
        let verify = inference.then(|| DesktopResponse {
            routing: self.routing.clone(),
            route,
            binding,
        });
        forward_response(request, wants_stream, permit, log, verify).await
    }

    async fn forward_codex(
        &self,
        parts: hyper::http::request::Parts,
        body: Incoming,
        log: RequestLog,
    ) -> Response<Body> {
        if parts.method != Method::POST || parts.uri.query().is_some() {
            return log.reject(
                StatusCode::NOT_FOUND,
                "CODEX_GATEWAY_ENDPOINT",
                "Codex 网关只接收 Responses 推理请求",
            );
        }
        let token = parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let route = self.codex_route.lock().expect("Codex inference").clone();
        let Some(route) = route.filter(|route| route.authorizes(token)) else {
            return log.reject(
                StatusCode::UNAUTHORIZED,
                "CODEX_GATEWAY_AUTH",
                "Codex 网关档案已变更或凭据无效。请重新打开 Codex 或启动新会话。",
            );
        };
        let Ok(permit) = self.requests.clone().try_acquire_owned() else {
            return log.reject(
                StatusCode::TOO_MANY_REQUESTS,
                "CODEX_GATEWAY_BUSY",
                "CodeNeo Lite网关请求过多",
            );
        };
        let payload = match inference_payload(body, &log).await {
            Ok(payload) => payload,
            Err(response) => return response,
        };
        if payload["model"].as_str() != Some(route.model.as_str()) {
            return log.reject(
                StatusCode::BAD_REQUEST,
                "CODEX_GATEWAY_MODEL",
                "此模型与已应用的 Codex 档案不符。请使用档案中的模型或新建会话。",
            );
        }
        // The request retains the authorized route even if the active profile
        // changes while its response is streaming.
        let request = route.request(&self.client, &payload);
        forward_response(request, payload["stream"] == true, permit, log, None).await
    }
}

async fn inference_payload(body: Incoming, log: &RequestLog) -> Result<Value, Response<Body>> {
    let Ok(Ok(body)) = tokio::time::timeout(
        Duration::from_secs(30),
        Limited::new(body, MAX_REQUEST).collect(),
    )
    .await
    else {
        return Err(log.reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "GATEWAY_BODY",
            "推理请求过大或读取超时",
        ));
    };
    match serde_json::from_slice::<Value>(&body.to_bytes()) {
        Ok(payload) if payload.is_object() => Ok(payload),
        _ => Err(log.reject(StatusCode::BAD_REQUEST, "GATEWAY_JSON", "推理请求格式无效")),
    }
}

async fn read_kimi_concurrency_error(response: &mut reqwest::Response) -> Option<&'static str> {
    const MAX_ERROR_BYTES: usize = 8 * 1024;
    if !matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    ) || !matches!(
        response.url().path(),
        "/coding/v1/messages" | "/coding/v1/messages/count_tokens"
    ) || !crate::model_transport::is_kimi_code_endpoint(response.url())
        || response
            .content_length()
            .is_some_and(|length| length > MAX_ERROR_BYTES as u64)
    {
        return None;
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if body.len() + chunk.len() > MAX_ERROR_BYTES {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        crate::model_transport::kimi_code_concurrency_message(&body)
    })
    .await
    .ok()
    .flatten()
}

async fn upstream_error(mut response: reqwest::Response, log: &RequestLog) -> Response<Body> {
    let status = response.status();
    if let Some(message) = read_kimi_concurrency_error(&mut response).await {
        log.record(
            "warn",
            "上游响应",
            "KIMI_UPSTREAM_CONCURRENCY_LIMIT",
            &format!("Kimi Code 返回 HTTP {} · 并发请求受限", status.as_u16()),
        );
        return non_retryable_error(StatusCode::TOO_MANY_REQUESTS, "rate_limit_error", message);
    }
    log.record(
        "error",
        "上游响应",
        "DESKTOP_UPSTREAM_HTTP_ERROR",
        &format!("供应商返回 HTTP {}。", status.as_u16()),
    );
    let status = if status.is_redirection() {
        StatusCode::BAD_GATEWAY
    } else {
        status
    };
    error(status, "第三方模型接口返回错误，请核对模型、权限和额度")
}

async fn forward_response(
    request: reqwest::RequestBuilder,
    wants_stream: bool,
    permit: tokio::sync::OwnedSemaphorePermit,
    log: RequestLog,
    verify: Option<DesktopResponse>,
) -> Response<Body> {
    let response = match request.send().await {
        Ok(response) => response,
        Err(reason) => {
            let detail = if reason.is_timeout() {
                "上游请求超时。"
            } else if reason.is_connect() {
                "无法连接上游。"
            } else {
                "上游请求传输失败。"
            };
            log.record("error", "上游请求", "DESKTOP_UPSTREAM_CONNECTION", detail);
            return error(StatusCode::BAD_GATEWAY, "无法连接第三方模型接口");
        }
    };
    let status = response.status();
    if !status.is_success() {
        return upstream_error(response, &log).await;
    }
    let upstream_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let stream = upstream_type == Some("text/event-stream");
    let expected_format = if wants_stream {
        stream
    } else {
        upstream_type == Some("application/json") || stream
    };
    let content_type = if stream {
        "text/event-stream"
    } else {
        "application/json"
    };
    log.record(
        "info",
        "上游响应",
        "DESKTOP_UPSTREAM_HEADERS",
        &format!("供应商已返回 HTTP {} 响应头。", status.as_u16()),
    );
    let forwarding = ForwardedBody {
        response,
        _permit: permit,
        verify,
        log,
        has_data: false,
        expected_format,
        sse: stream.then(SseEvents::default),
    };
    let frames = stream::unfold(Some(forwarding), |state| async move {
        let mut state = state?;
        match state.response.chunk().await {
            Ok(Some(bytes)) => {
                state.has_data |= !bytes.is_empty();
                if let Some(sse) = &mut state.sse {
                    sse.observe(&bytes);
                }
                Some((Ok(Frame::data(bytes)), Some(state)))
            }
            Ok(None) => {
                state.finish();
                None
            }
            Err(_) => {
                state.log.record(
                    "error",
                    "上游响应",
                    "DESKTOP_UPSTREAM_STREAM_INTERRUPTED",
                    "上游响应传输中断。",
                );
                Some((Err(std::io::Error::other("第三方推理流中断")), None))
            }
        }
    });
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(StreamBody::new(frames).boxed_unsync())
        .expect("inference response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::ResponseBuilderExt;
    use tokio::sync::Notify;

    #[test]
    fn parses_only_sse_event_names_across_chunks() {
        let mut sse = SseEvents::default();
        sse.observe(b"data: {\"text\":\"event: message_stop\"}\n\nevent: mess");
        assert!(!sse.stopped);
        sse.observe(b"age_stop\r\n\r\n");
        assert!(sse.stopped);
        sse.observe(&[b'x'; 100]);
        sse.observe(b"event: error\nevent: err");
        assert!(!sse.failed);
        sse.observe(b"or\n\n");
        assert!(sse.failed);
    }

    #[tokio::test]
    async fn desktop_distinguishes_kimi_concurrency_from_permission_errors() {
        let kimi = "https://api.kimi.com/coding/v1/messages";
        let concurrency = "You've reached your concurrent request limit. Please wait for your ongoing requests to finish and try again.";
        let permission = "Your current subscription does not have access to k3.1-pro.";
        let request_log = RequestLog::new(&Method::POST, "/v1/messages");
        for (url, upstream_status, message, rate_limited) in [
            (kimi, StatusCode::FORBIDDEN, concurrency, true),
            (kimi, StatusCode::TOO_MANY_REQUESTS, concurrency, true),
            (
                "https://api.kimi.com/coding/v1/messages/count_tokens",
                StatusCode::FORBIDDEN,
                concurrency,
                true,
            ),
            (kimi, StatusCode::FORBIDDEN, permission, false),
            (
                "https://api.kimi.com/coding/v1/responses",
                StatusCode::FORBIDDEN,
                concurrency,
                false,
            ),
            (
                "https://custom.example/v1/messages",
                StatusCode::FORBIDDEN,
                concurrency,
                false,
            ),
        ] {
            let upstream = Response::builder()
                .url(Url::parse(url).unwrap())
                .status(upstream_status)
                .body(
                    json!({
                        "type": "error",
                        "error": {"type": "permission_error", "message": message},
                        "details": "upstream-private-content"
                    })
                    .to_string(),
                )
                .unwrap()
                .into();
            let response = upstream_error(upstream, &request_log).await;
            let (parts, body) = response.into_parts();
            let text = body.collect().await.unwrap().to_bytes();
            assert!(!std::str::from_utf8(&text)
                .unwrap()
                .contains("upstream-private-content"));
            let body: Value = serde_json::from_slice(&text).unwrap();
            if rate_limited {
                assert_eq!(parts.status, StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(parts.headers["x-should-retry"], "false");
                assert_eq!(body["error"]["type"], "rate_limit_error");
                assert!(body["error"]["message"].as_str().unwrap().contains("并发"));
            } else {
                assert_eq!(parts.status, upstream_status);
                assert!(!parts.headers.contains_key("x-should-retry"));
                assert_eq!(body["error"]["type"], "api_error");
            }
        }
    }

    async fn upstream(
        model: &'static str,
        wait: Option<Arc<Notify>>,
        inspect: impl Fn(&Value) + Send + Sync + Copy + 'static,
    ) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let wait = wait.clone();
                tokio::spawn(async move {
                    let handler = service_fn(move |request: Request<Incoming>| {
                        let wait = wait.clone();
                        async move {
                            assert_eq!(request.uri().path(), "/v1/messages");
                            assert_eq!(
                                request.headers()["authorization"],
                                format!("Bearer {model}-secret")
                            );
                            let bytes = request.into_body().collect().await.unwrap().to_bytes();
                            let data: Value = serde_json::from_slice(&bytes).unwrap();
                            let routed_model = data["model"].as_str().unwrap().to_owned();
                            assert!(
                                routed_model == model || routed_model == format!("{model}-alt")
                            );
                            assert!(data.get("metadata").is_none());
                            inspect(&data);
                            let frames = stream::unfold(
                                (0, wait, routed_model),
                                move |(part, wait, routed_model)| async move {
                                    let line = match part {
                                        0 => format!(
                                            "event: message_start\ndata: {routed_model}\n\n"
                                        ),
                                        1 => {
                                            if let Some(wait) = &wait {
                                                wait.notified().await;
                                            }
                                            "event: message_stop\ndata: {}\n\n".to_string()
                                        }
                                        _ => return None,
                                    };
                                    Some((
                                        Ok::<_, std::io::Error>(Frame::data(Bytes::from(line))),
                                        (part + 1, wait, routed_model),
                                    ))
                                },
                            );
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .header("content-type", "text/event-stream")
                                    .body(StreamBody::new(frames).boxed_unsync())
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), handler)
                        .await;
                });
            }
        });
        Url::parse(&format!("http://{address}")).unwrap()
    }

    fn profile(model: &str, base: Url) -> Arc<Route> {
        Arc::new(Route {
            profile_id: model.into(),
            base,
            model: model.into(),
            additional_models: vec![GatewayModel::new(format!("{model}-alt"))],
            api_key: false,
            secret: format!("{model}-secret"),
        })
    }

    #[tokio::test]
    async fn codex_responses_stream_stays_on_its_supplier_when_the_profile_changes() {
        let data = tempfile::tempdir().unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/api/v1/responses",
            upstream.local_addr().unwrap()
        ))
        .unwrap();
        let gate = Arc::new(Notify::new());
        let upstream_gate = gate.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = upstream.accept().await.unwrap();
            let handler = service_fn(move |request: Request<Incoming>| {
                let gate = upstream_gate.clone();
                async move {
                    assert_eq!(request.method(), Method::POST);
                    assert_eq!(request.uri().path(), "/api/v1/responses");
                    assert_eq!(
                        request.headers()["authorization"],
                        "Bearer fixture-provider-a"
                    );
                    assert_eq!(request.headers()["accept"], "text/event-stream");
                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                    let input: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(input["model"], "fixture-model");
                    assert_eq!(input["input"], "test prompt");
                    assert_eq!(input["store"], false);
                    let events = stream::unfold((0, gate), |(step, gate)| async move {
                        let bytes = match step {
                            0 => "event: response.output_text.delta\ndata: {\"delta\":\"OK\"}\n\n",
                            1 => {
                                gate.notified().await;
                                "event: response.completed\ndata: {}\n\n"
                            }
                            _ => return None,
                        };
                        Some((
                            Ok::<_, std::io::Error>(Frame::data(Bytes::from_static(
                                bytes.as_bytes(),
                            ))),
                            (step + 1, gate),
                        ))
                    });
                    Ok::<_, Infallible>(
                        Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(StreamBody::new(events).boxed_unsync())
                            .unwrap(),
                    )
                }
            });
            http1::Builder::new()
                .keep_alive(false)
                .serve_connection(TokioIo::new(socket), handler)
                .await
                .unwrap();
        });
        let mut service = Service::new();
        let address = service
            .bind("fixture-desktop-key", 0, &data.path().join("sessions.json"))
            .await
            .unwrap();
        assert!(!service.is_ready());
        let desktop = profile("fixture-desktop", endpoint.clone());
        service.select(Some(desktop.clone()), false);
        assert!(service.is_ready());
        let codex = |key: &str, upstream_key: &str| {
            Arc::new(codex_inference::Route {
                endpoint: endpoint.clone(),
                model: "fixture-model".into(),
                key: key.into(),
                upstream_key: upstream_key.into(),
            })
        };
        let first = codex("fixture-local-a", "fixture-provider-a");
        let second = codex("fixture-local-b", "fixture-provider-b");
        service.select_codex(Some(first));
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let call = |key: &str, model: &str| {
            client.post(format!("http://{address}{}", gateway::CODEX_RESPONSES))
            .bearer_auth(key).json(&json!({"model": model, "input": "test prompt", "store": false, "stream": true}))
        };
        assert_eq!(
            call("fixture-desktop-key", "fixture-model")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call("fixture-local-a", "other-model")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        let mut old = call("fixture-local-a", "fixture-model")
            .send()
            .await
            .unwrap();
        assert_eq!(old.headers()["content-type"], "text/event-stream");
        let first_chunk = old.chunk().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&first_chunk).contains("\"delta\":\"OK\""));
        service.select_codex(Some(second.clone()));
        assert!(service.is_serving(&desktop));
        assert!(service.is_serving_codex(&second));
        assert_eq!(
            call("fixture-local-a", "fixture-model")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let desktop_models = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth("fixture-desktop-key")
            .send()
            .await
            .unwrap();
        assert_eq!(
            desktop_models.json::<Value>().await.unwrap()["data"][0]["display_name"],
            "fixture-desktop"
        );
        gate.notify_one();
        assert!(old.text().await.unwrap().contains("response.completed"));
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        service.select(None, false);
        assert!(service.is_ready());
        service.select_codex(None);
        assert!(!service.is_ready());
    }

    // Run with CODENEO_TEST_CODEX set to a native CLI binary. The whole turn
    // uses a fixture provider and a temporary CODEX_HOME.
    #[tokio::test]
    #[ignore = "requires an installed Codex CLI"]
    async fn native_codex_completes_a_turn_through_the_gateway() {
        let binary = std::env::var_os("CODENEO_TEST_CODEX").expect("set CODENEO_TEST_CODEX");
        let fixture = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/v1/responses",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let provider = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let handler = service_fn(move |req: Request<Incoming>| async move {
                assert_eq!(req.uri().path(), "/v1/responses");
                assert_eq!(req.headers()["authorization"], "Bearer fixture-upstream");
                let body = req.into_body().collect().await.unwrap().to_bytes();
                let input: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(input["model"], "fixture-model");
                assert_eq!(input["stream"], true);
                let response = json!({"id":"resp_fixture", "object":"response", "created_at":0,
                    "model":"fixture-model", "status":"in_progress", "output":[]});
                let message = json!({"id":"msg_fixture", "type":"message", "role":"assistant", "status":"completed",
                    "content":[{"type":"output_text","text":"CODENEO-GATEWAY-FIXTURE-OK","annotations":[]}]});
                let mut complete = response.clone();
                complete["status"] = json!("completed");
                complete["output"] = json!([message]);
                complete["usage"] = json!({"input_tokens":5,"output_tokens":5,"total_tokens":10,
                    "input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}});
                let events = [
                    json!({"type":"response.created", "response":response}),
                    json!({"type":"response.output_item.added", "output_index":0,
                        "item":{"id":"msg_fixture","type":"message","role":"assistant","status":"in_progress","content":[]}}),
                    json!({"type":"response.output_text.delta", "item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"CODENEO-GATEWAY-FIXTURE-OK"}),
                    json!({"type":"response.output_item.done", "output_index":0,"item":message}),
                    json!({"type":"response.completed", "response":complete}),
                ];
                let text: String = events
                    .iter()
                    .map(|event| {
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().unwrap()
                        )
                    })
                    .collect();
                Ok::<_, Infallible>(
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Full::new(Bytes::from(text)))
                        .unwrap(),
                )
            });
            http1::Builder::new()
                .keep_alive(false)
                .serve_connection(TokioIo::new(socket), handler)
                .await
                .unwrap();
        });
        let mut service = Service::new();
        let address = service
            .bind(
                "fixture-desktop-key",
                0,
                &fixture.path().join("sessions.json"),
            )
            .await
            .unwrap();
        service.select_codex(Some(Arc::new(codex_inference::Route {
            endpoint,
            model: "fixture-model".into(),
            key: "fixture-codex-key".into(),
            upstream_key: "fixture-upstream".into(),
        })));
        fs::write(
            fixture.path().join("config.toml"),
            format!(
                r#"
model = "fixture-model"
model_provider = "codeneo-lite"
web_search = "disabled"
[model_providers.codeneo-lite]
name = "CodeNeo Lite网关"
wire_api = "responses"
base_url = "http://{address}/codex/v1"
supports_websockets = false
[model_providers.codeneo-lite.capabilities]
remote_compaction = "unsupported"
[model_providers.codeneo-lite.auth]
command = "/usr/bin/printf"
args = ["%s", "fixture-codex-key"]
"#
            ),
        )
        .unwrap();
        let work = fixture.path().join("work");
        fs::create_dir(&work).unwrap();
        let mut command = tokio::process::Command::new(binary);
        command
            .env_clear()
            .env("HOME", fixture.path())
            .env("CODEX_HOME", fixture.path())
            .env("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
            .current_dir(work)
            .kill_on_drop(true)
            .args([
                "--strict-config",
                "-a",
                "never",
                "exec",
                "-s",
                "read-only",
                "--skip-git-repo-check",
                "--ephemeral",
                "--color",
                "never",
                "--json",
                "--disable",
                "apps",
                "--disable",
                "plugins",
                "Reply with the fixture text.",
            ]);
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("CODENEO-GATEWAY-FIXTURE-OK"));
        tokio::time::timeout(Duration::from_secs(2), provider)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn switching_preserves_in_flight_streams_and_uses_the_new_supplier_for_the_next_turn() {
        let gate = Arc::new(Notify::new());
        let a = profile("k3", upstream("k3", Some(gate.clone()), |_| {}).await);
        let b = profile(
            "deepseek-flash",
            upstream("deepseek-flash", None, |data| {
                if data["messages"].as_array().unwrap().len() > 1 {
                    assert_eq!(data["messages"][1]["content"], "existing answer");
                    assert_eq!(data["messages"][2]["content"], "follow-up");
                }
            })
            .await,
        );
        let data = tempfile::tempdir().unwrap();
        let mut service = Service::default();
        let address = service
            .bind("local-key", 0, &data.path().join("sessions.json"))
            .await
            .unwrap();
        let url = format!("http://{address}/v1/messages");
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap();
        let new_message = |session: &str| {
            json!({
                "model":MODEL,"stream":true,"messages":[{"role":"user","content":"fixture"}],
                "metadata":{"user_id":json!({"session_id":session}).to_string()}
            })
        };
        let send = |data: Value| {
            client
                .post(&url)
                .header("x-api-key", "local-key")
                .json(&data)
                .send()
        };
        let list_models = || async {
            client
                .get(format!("http://{address}/v1/models"))
                .header("x-api-key", "local-key")
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        };
        assert_eq!(list_models().await["data"], json!([]));
        service.select(Some(a.clone()), true);
        let catalog = list_models().await;
        assert_eq!(catalog["data"][0]["id"], MODEL);
        assert_eq!(catalog["data"][0]["display_name"], a.model);
        assert_eq!(catalog["data"][1]["display_name"], "k3-alt");
        let alternate = catalog["data"][1]["id"].clone();
        assert_eq!(catalog["last_id"], alternate);
        let mut old = send(new_message("first")).await.unwrap();
        let first = old.chunk().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&first).contains("data: k3\n"));
        service.select(Some(b.clone()), false);
        let catalog = list_models().await;
        assert_eq!(catalog["data"][0]["display_name"], b.model);
        assert!(send(new_message("second"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .contains("data: deepseek-flash\n"));
        assert!(service.verified(&b));
        gate.notify_one();
        assert!(old.text().await.unwrap().contains("message_stop"));

        let mut continuation = new_message("first");
        continuation["messages"] = json!([
            {"role":"user","content":"fixture"},
            {"role":"assistant","content":"existing answer"},
            {"role":"user","content":"follow-up"},
        ]);
        for (model, expected) in [
            (json!(MODEL), "data: deepseek-flash\n"),
            (
                catalog["data"][1]["id"].clone(),
                "data: deepseek-flash-alt\n",
            ),
            (alternate, "data: deepseek-flash\n"),
        ] {
            continuation["model"] = model;
            assert!(send(continuation.clone())
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
                .contains(expected));
        }
        continuation["model"] = json!("unconfigured-model");
        assert_eq!(
            send(continuation.clone()).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        continuation["model"] = json!(MODEL);
        continuation["metadata"] = Value::Null;
        assert_eq!(
            send(continuation).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client.post(url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn restarting_continues_existing_sessions_and_recovers_unknown_history() {
        let a = profile("fixture-a", upstream("fixture-a", None, |_| {}).await);
        let b = profile("fixture-b", upstream("fixture-b", None, |_| {}).await);
        let data = tempfile::tempdir().unwrap();
        let storage = data.path().join("sessions.json");
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap();
        let send = |address, data: Value| {
            client
                .post(format!("http://{address}/v1/messages"))
                .header("x-api-key", "local-key")
                .json(&data)
                .send()
        };
        let mut first = Service::default();
        let address = first.bind("local-key", 0, &storage).await.unwrap();
        let mut single_model = a.as_ref().clone();
        single_model.additional_models.clear();
        first.select(Some(Arc::new(single_model)), true);
        let mut message = json!({
            "model":MODEL,"stream":true,"messages":[{"role":"user","content":"fixture"}],
            "metadata":{"user_id":json!({"session_id":"fixture-session"}).to_string()}
        });
        let reply = send(address, message.clone())
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(reply.contains("data: fixture-a\n"));
        drop(first);

        let mut restored = Service::default();
        let address = restored.bind("local-key", 0, &storage).await.unwrap();
        restored.select(Some(a.clone()), true);
        message["messages"] = json!([
            {"role":"user","content":"fixture"},
            {"role":"assistant","content":"first answer"},
            {"role":"user","content":"follow-up"}
        ]);
        message["model"] = json!(a.additional_models[0].id);
        let reply = send(address, message.clone())
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(reply.contains("data: fixture-a-alt\n"));

        message["metadata"]["user_id"] = json!(json!({"session_id":"legacy-session"}).to_string());
        let reply = send(address, message.clone())
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(reply.contains("data: fixture-a-alt\n"));
        assert!(restored.pending_session(&a).is_none());

        let known_session = message.clone();
        restored.select(Some(b.clone()), false);
        let reply = send(address, known_session)
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(reply.contains("data: fixture-b\n"));
        assert!(restored.pending_session(&b).is_none());

        message["metadata"]["user_id"] =
            json!(json!({"session_id":"unconfirmed-session"}).to_string());
        let rejected = send(address, message.clone()).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        assert_eq!(rejected.headers()["x-should-retry"], "false");
        let request = restored.pending_session(&b).unwrap();
        assert!(restored.recover_session(&a, request).is_err());
        assert!(restored.recover_session(&b, request + 1).is_err());
        assert_eq!(
            send(address, message.clone()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(restored.pending_session(&b), Some(request));
        restored.recover_session(&b, request).unwrap();
        assert!(restored.pending_session(&b).is_none());
        restored.select(Some(a.clone()), false);
        assert_eq!(
            send(address, message.clone()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        restored.select(Some(b.clone()), false);
        let reply = send(address, message).await.unwrap().text().await.unwrap();
        assert!(reply.contains("data: fixture-b\n"));

        let stored = fs::read_to_string(storage).unwrap();
        let sessions: Vec<SessionBinding> = serde_json::from_str(&stored).unwrap();
        assert_eq!(sessions.len(), 3);
        assert_eq!(
            sessions
                .iter()
                .find(
                    |binding| binding.session == format!("{:x}", Sha256::digest(b"legacy-session"))
                )
                .unwrap()
                .supplier,
            b.supplier_fingerprint()
        );
        assert!(
            !stored.contains("fixture")
                && !stored.contains("legacy-session")
                && !stored.contains("first answer")
        );
    }

    #[test]
    fn legacy_binding_keeps_text_tools_and_only_the_current_suppliers_thinking_after_restart() {
        let data = tempfile::tempdir().unwrap();
        let storage = data.path().join("desktop-sessions.json");
        let kimi = profile("k3", Url::parse("https://kimi.example").unwrap());
        let deepseek = profile(
            "deepseek-flash",
            Url::parse("https://api.deepseek.com/anthropic").unwrap(),
        );
        let session = format!("{:x}", Sha256::digest(b"old-thread"));
        fs::write(
            &storage,
            json!([{"session": session, "supplier": kimi.supplier_fingerprint()}]).to_string(),
        )
        .unwrap();
        let content = json!([
            {"type":"thinking","thinking":"Kimi reasoning","signature":"kimi-signature"},
            {"type":"redacted_thinking","data":"opaque-kimi-data"},
            {"type":"text","text":"I will look it up"},
            {"type":"tool_use","id":"tool-1","name":"read","input":{"path":"example"}},
        ]);
        let mut history = json!({
            "model": MODEL,
            "thinking": {"type":"adaptive"},
            "metadata":{"user_id": json!({"session_id":"old-thread"}).to_string()},
            "messages":[
                {"role":"user","content":"Remember this"},
                {"role":"assistant","content":content},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"value"},{"type":"text","text":"Follow up"}]}
            ]
        });
        let mut routing = Routing::open(&storage);
        routing.active = Some(deepseek.clone());
        let mut expected = history.clone();
        expected["messages"][1]["content"] = json!(content.as_array().unwrap()[2..]);
        expected["thinking"] = json!({"type":"disabled"});
        let log = RequestLog::new(&Method::POST, "/v1/messages");
        for (inference, completed) in [(false, false), (true, false), (true, true), (true, false)] {
            let mut payload = history.clone();
            let Ok(target) = routing.route(&mut payload, inference, &log) else {
                panic!("the existing Desktop session should continue");
            };
            assert_eq!(target.model, "deepseek-flash");
            assert_eq!(payload, expected);
            if completed {
                routing.bind(
                    target
                        .binding
                        .expect("the first completed turn records the switch"),
                );
            }
        }
        let stored = fs::read_to_string(&storage).unwrap();
        assert!(!stored.contains("kimi-signature") && !stored.contains("Kimi reasoning"));
        assert!(!stored.contains("opaque-kimi-data") && !stored.contains("old-thread"));

        let new_turn = json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"DeepSeek reasoning","signature":"deepseek-signature"},
            {"type":"text","text":"DeepSeek replied"}
        ]});
        history["messages"]
            .as_array_mut()
            .unwrap()
            .push(new_turn.clone());
        history["messages"][1]["content"][0]["thinking"] = json!("");
        expected["messages"].as_array_mut().unwrap().push(new_turn);
        expected["thinking"] = history["thinking"].clone();
        let mut routing = Routing::open(&storage);
        routing.active = Some(deepseek);
        let mut payload = history.clone();
        assert!(routing.route(&mut payload, true, &log).is_ok());
        assert_eq!(payload, expected);

        routing.active = Some(kimi);
        let mut payload = history.clone();
        assert!(routing.route(&mut payload, true, &log).is_ok());
        assert_eq!(payload["messages"][1], history["messages"][1]);
        assert_eq!(payload["messages"][2], history["messages"][2]);
        assert_eq!(payload["thinking"], history["thinking"]);
        assert_eq!(
            payload["messages"][3]["content"],
            json!([{"type":"text","text":"DeepSeek replied"}])
        );
    }
}
