use super::{checked_url, connection, validate_connection, Transport};
use crate::{agents, launch, remote, transaction::Client};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    RequestBuilder, Response, Url,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, env, io, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    time::timeout,
};

const DEADLINE: Duration = Duration::from_secs(20);
const STDIO_DEADLINE: Duration = Duration::from_secs(60);
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;
const PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const PROTOCOL_VERSION: &str = "mcp-protocol-version";
const SESSION_ID: &str = "mcp-session-id";
const BAD_REPLY: &str = "服务未返回有效的 MCP 响应";
const CLOSED: &str = "MCP 进程已退出或关闭了连接，请检查启动命令、参数和环境变量";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Check {
    protocol_version: String,
    tool_count: Option<usize>,
}

pub async fn check(client: Client, server: Value, project: Option<&Path>) -> Result<Check, String> {
    let config = connection(client, &server)?;
    validate_connection(&config)?;
    if matches!(client, Client::Codex | Client::Kimi)
        && server["environment_id"]
            .as_str()
            .is_some_and(|id| id != "local")
    {
        return Err("此服务使用客户端的远程执行环境，请在目标客户端中验证".into());
    }
    if config.transport != Transport::Stdio {
        let url = checked_url(config.endpoint.trim())?;
        let headers = request_headers(client, &config.options, &server)?;
        return check_http(config.transport, url, headers).await;
    }

    let home = agents::home()?;
    let base = project.unwrap_or(&home);
    let cwd = server["cwd"].as_str().map(|path| base.join(path));
    let mut command = Command::new(config.endpoint.trim());
    command
        .args(&config.args)
        .env("PATH", launch::agent_path(None, &home)?)
        .envs(&config.options)
        .current_dir(cwd.as_deref().unwrap_or(base))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command.spawn().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => "找不到 MCP 命令或工作目录，请检查路径",
        io::ErrorKind::PermissionDenied => "MCP 命令没有执行权限",
        _ => "无法启动 MCP 进程",
    })?;
    let mut channel = Channel::Stdio {
        input: child.stdin.take().ok_or(CLOSED)?,
        output: BufReader::new(child.stdout.take().ok_or(CLOSED)?),
    };
    let result = run(&mut channel, config.transport).await;
    drop(channel);
    if timeout(Duration::from_millis(500), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    result
}

async fn run(channel: &mut Channel, transport: Transport) -> Result<Check, String> {
    wait_for_discovery(transport, discover(channel, transport)).await
}

async fn wait_for_discovery<F>(transport: Transport, discovery: F) -> Result<Check, String>
where
    F: std::future::Future<Output = Result<Check, String>>,
{
    let deadline = if transport == Transport::Stdio {
        // npx and similar launchers may need to download the server on first use.
        STDIO_DEADLINE
    } else {
        DEADLINE
    };
    timeout(deadline, discovery)
        .await
        .map_err(|_| "MCP 验证超时，请检查服务是否能正常启动和响应".to_owned())?
}

async fn discover(channel: &mut Channel, transport: Transport) -> Result<Check, String> {
    let version = if transport == Transport::Sse {
        "2024-11-05"
    } else {
        PROTOCOLS[0]
    };
    let hello = channel
        .call(
            json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                "protocolVersion":version, "capabilities":{},
                "clientInfo":{"name":"codeneo-lite", "version":env!("CARGO_PKG_VERSION")}
            }}),
        )
        .await?;
    let protocol = hello["protocolVersion"].as_str().ok_or(BAD_REPLY)?;
    if !PROTOCOLS.contains(&protocol) {
        return Err("服务使用了 Lite 尚未支持的 MCP 协议版本".into());
    }
    let capabilities = hello["capabilities"].as_object().ok_or(BAD_REPLY)?;
    if hello
        .pointer("/serverInfo/name")
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(BAD_REPLY.into());
    }
    if let Channel::Http(http) = channel {
        http.protocol = Some(HeaderValue::from_str(protocol).map_err(|_| BAD_REPLY)?);
    }
    channel
        .send(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))
        .await?;

    let mut check = Check {
        protocol_version: protocol.to_owned(),
        tool_count: None,
    };
    if capabilities.get("tools").is_none_or(Value::is_null) {
        return Ok(check);
    }
    let mut request = json!({"jsonrpc":"2.0", "id":2, "method":"tools/list"});
    let mut count = 0;
    // Bound discovery in case a server keeps returning a cursor.
    for id in 3..=102 {
        let result = channel.call(request).await?;
        let tools = result["tools"]
            .as_array()
            .ok_or("服务返回的 MCP 工具列表无效")?;
        if tools
            .iter()
            .any(|tool| tool["name"].as_str().is_none_or(str::is_empty))
        {
            return Err("服务返回的 MCP 工具列表无效".into());
        }
        count += tools.len();
        let Some(cursor) = result["nextCursor"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
        else {
            check.tool_count = Some(count);
            return Ok(check);
        };
        request =
            json!({"jsonrpc":"2.0", "id":id, "method":"tools/list", "params":{"cursor":cursor}});
    }
    Err("MCP 工具列表超过验证可读取的页数".into())
}

fn rpc_result(message: Value, id: &Value) -> Result<Option<Value>, String> {
    if message["jsonrpc"] != "2.0" {
        return Err(BAD_REPLY.into());
    }
    if message.get("id") != Some(id) || message.get("method").is_some() {
        return Ok(None);
    }
    if let Some(code) = message.pointer("/error/code").and_then(Value::as_i64) {
        let step = if *id == 1 {
            "初始化"
        } else {
            "读取工具列表"
        };
        return Err(format!("MCP 服务拒绝{step}（错误码 {code}）"));
    }
    message
        .get("result")
        .cloned()
        .map(Some)
        .ok_or_else(|| BAD_REPLY.into())
}

enum Channel {
    Stdio {
        input: ChildStdin,
        output: BufReader<ChildStdout>,
    },
    Http(Http),
    Sse {
        http: Http,
        stream: Events,
    },
}

impl Channel {
    async fn send(&mut self, message: Value) -> Result<Option<Response>, String> {
        match self {
            Self::Stdio { input, .. } => {
                let mut bytes = serde_json::to_vec(&message).map_err(|_| BAD_REPLY)?;
                bytes.push(b'\n');
                input.write_all(&bytes).await.map_err(|_| CLOSED)?;
                input.flush().await.map_err(|_| CLOSED)?;
                Ok(None)
            }
            Self::Http(http) | Self::Sse { http, .. } => Ok(Some(http.post(&message).await?)),
        }
    }

    async fn call(&mut self, message: Value) -> Result<Value, String> {
        let id = message["id"].clone();
        let response = self.send(message).await?;
        match self {
            Self::Stdio { output, .. } => {
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let read = (&mut *output)
                        .take(MAX_RESPONSE + 1)
                        .read_until(b'\n', &mut line)
                        .await
                        .map_err(|_| CLOSED)?;
                    if read == 0 {
                        return Err(CLOSED.into());
                    }
                    if read as u64 > MAX_RESPONSE {
                        return Err(BAD_REPLY.into());
                    }
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let message = serde_json::from_slice(&line).map_err(|_| BAD_REPLY)?;
                    if let Some(result) = rpc_result(message, &id)? {
                        return Ok(result);
                    }
                }
            }
            Self::Sse { stream, .. } => stream.reply(&id).await,
            Self::Http(http) => {
                let mut response = response.ok_or(BAD_REPLY)?;
                if id == 1 {
                    http.session = response.headers().get(SESSION_ID).cloned();
                    if let Some(session) = &mut http.session {
                        session.set_sensitive(true);
                    }
                }
                match content_type(&response) {
                    Some("text/event-stream") => Events::new(response).reply(&id).await,
                    Some("application/json") => {
                        let mut body = Vec::new();
                        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
                            remote::extend(&mut body, &chunk).map_err(|_| BAD_REPLY)?;
                        }
                        let message = serde_json::from_slice(&body).map_err(|_| BAD_REPLY)?;
                        rpc_result(message, &id)?.ok_or_else(|| BAD_REPLY.into())
                    }
                    _ => Err(BAD_REPLY.into()),
                }
            }
        }
    }
}

fn request_headers(
    client: Client,
    options: &BTreeMap<String, String>,
    server: &Value,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in options {
        if matches!(
            client,
            Client::Code | Client::Qwen | Client::Codebuddy | Client::Opencode
        ) && (value.contains("${") || value.contains("{env:"))
        {
            return Err("此请求头引用了客户端环境变量，请在目标客户端中验证".into());
        }
        headers.insert(header_name(name)?, header_value(value)?);
    }
    if client == Client::Codex {
        if let Some(entries) = server["env_http_headers"].as_object() {
            for (name, variable) in entries {
                let variable = variable.as_str().ok_or("MCP 请求头的环境变量配置无效")?;
                headers.insert(header_name(name)?, header_value(&credential(variable)?)?);
            }
        }
    }
    let bearer = match client {
        Client::Codex => server["bearer_token_env_var"].as_str(),
        Client::Kimi => server["bearerTokenEnvVar"].as_str(),
        _ => None,
    };
    if let Some(variable) = bearer {
        headers.insert(
            AUTHORIZATION,
            header_value(&format!("Bearer {}", credential(variable)?))?,
        );
    }
    Ok(headers)
}

fn credential(variable: &str) -> Result<String, String> {
    env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Lite 进程未读取到 MCP 认证所需的环境变量".into())
}

fn header_name(name: &str) -> Result<HeaderName, String> {
    HeaderName::from_bytes(name.as_bytes()).map_err(|_| "MCP 请求头名称无效".into())
}

fn header_value(value: &str) -> Result<HeaderValue, String> {
    let mut header = HeaderValue::from_str(value).map_err(|_| "MCP 请求头值无效")?;
    header.set_sensitive(true);
    Ok(header)
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "连接 MCP 服务超时，请检查地址和网络".into()
    } else {
        "无法连接 MCP 服务，请检查地址、网络或 TLS 证书".into()
    }
}

async fn response(request: RequestBuilder) -> Result<Response, String> {
    let response = request.send().await.map_err(network_error)?;
    let status = response.status();
    match status.as_u16() {
        200..=299 => Ok(response),
        401 if response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|challenge| {
                challenge
                    .to_ascii_lowercase()
                    .contains("resource_metadata=")
            }) =>
        {
            Err("MCP 需要认证（HTTP 401），请检查是否需要在目标客户端完成 OAuth 登录".into())
        }
        401 | 403 => Err(format!(
            "MCP 认证失败（HTTP {}），请检查凭据或服务权限",
            status.as_u16()
        )),
        404 => Err("找不到 MCP 端点或会话（HTTP 404），请检查服务地址".into()),
        _ => Err(format!(
            "MCP 服务返回 HTTP {}，请检查地址和连接方式",
            status.as_u16()
        )),
    }
}

fn content_type(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()
        .map(str::trim)
}

struct Http {
    client: reqwest::Client,
    url: Url,
    headers: HeaderMap,
    session: Option<HeaderValue>,
    protocol: Option<HeaderValue>,
}

impl Http {
    async fn post(&self, message: &Value) -> Result<Response, String> {
        let mut headers = self.headers.clone();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(session) = &self.session {
            headers.insert(SESSION_ID, session.clone());
        }
        if let Some(protocol) = &self.protocol {
            headers.insert(PROTOCOL_VERSION, protocol.clone());
        }
        response(
            self.client
                .post(self.url.clone())
                .headers(headers)
                .json(message),
        )
        .await
    }
}

async fn check_http(
    transport: Transport,
    url: Url,
    mut headers: HeaderMap,
) -> Result<Check, String> {
    // Protocol headers are negotiated for this connection.
    for name in [
        PROTOCOL_VERSION,
        SESSION_ID,
        "content-length",
        "transfer-encoding",
        "host",
    ] {
        headers.remove(name);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .user_agent(concat!("codeneo-lite/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(network_error)?;
    let mut http = Http {
        client,
        url,
        headers,
        session: None,
        protocol: None,
    };
    if transport == Transport::Sse {
        let result = timeout(DEADLINE, async {
            let mut headers = http.headers.clone();
            headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
            let connected = response(http.client.get(http.url.clone()).headers(headers)).await?;
            if content_type(&connected) != Some("text/event-stream") {
                return Err(BAD_REPLY.into());
            }
            let mut stream = Events::new(connected);
            http.url = stream.endpoint(&http.url).await?;
            discover(&mut Channel::Sse { http, stream }, transport).await
        })
        .await
        .map_err(|_| "MCP 验证超时，请检查 SSE 服务是否正常响应")?;
        return result;
    }

    let mut channel = Channel::Http(http);
    let result = run(&mut channel, transport).await;
    if let Channel::Http(http) = channel {
        if let Some(session) = http.session {
            let mut headers = http.headers;
            headers.insert(SESSION_ID, session);
            if let Some(protocol) = http.protocol {
                headers.insert(PROTOCOL_VERSION, protocol);
            }
            let _ = timeout(
                Duration::from_secs(1),
                http.client.delete(http.url).headers(headers).send(),
            )
            .await;
        }
    }
    result
}

struct Events {
    response: Response,
    parser: remote::Sse,
    pending: std::vec::IntoIter<remote::Event>,
}

impl Events {
    fn new(response: Response) -> Self {
        Self {
            response,
            parser: Default::default(),
            pending: Vec::new().into_iter(),
        }
    }

    async fn next(&mut self) -> Result<remote::Event, String> {
        loop {
            if let Some(event) = self.pending.next() {
                return Ok(event);
            }
            let chunk = self
                .response
                .chunk()
                .await
                .map_err(network_error)?
                .ok_or("MCP 事件流已关闭，请检查服务地址和连接方式")?;
            self.pending = self.parser.push(&chunk).map_err(|_| BAD_REPLY)?.into_iter();
        }
    }

    async fn endpoint(&mut self, base: &Url) -> Result<Url, String> {
        loop {
            let event = self.next().await?;
            if event.kind.as_deref() == Some("endpoint") {
                let data = std::str::from_utf8(&event.data).map_err(|_| BAD_REPLY)?;
                let url = base.join(data.trim()).map_err(|_| BAD_REPLY)?;
                if url.origin() != base.origin()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err("SSE 服务返回了不安全的请求地址".into());
                }
                return Ok(url);
            }
        }
    }

    async fn reply(&mut self, id: &Value) -> Result<Value, String> {
        loop {
            let event = self.next().await?;
            if matches!(event.kind.as_deref(), None | Some("message")) {
                let message = event.json().map_err(|_| BAD_REPLY)?;
                if let Some(result) = rpc_result(message, id)? {
                    return Ok(result);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
    use hyper::{
        body::{Bytes, Frame, Incoming},
        service::service_fn,
        Request,
    };
    use hyper_util::rt::TokioIo;
    use std::{convert::Infallible, sync::Arc};
    use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};

    type Body = BoxBody<Bytes, Infallible>;

    fn full(body: impl Into<Bytes>) -> Body {
        Full::new(body.into()).boxed()
    }

    async fn local_http<F, Fut>(handler: F) -> (String, JoinHandle<()>)
    where
        F: Fn(Request<Incoming>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = hyper::Response<Body>> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let future = handler(request);
                        async move { Ok::<_, Infallible>(future.await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (format!("http://{addr}/mcp"), task)
    }

    fn hello(protocol: &str) -> Value {
        json!({"jsonrpc":"2.0", "id":1, "result":{
            "protocolVersion":protocol, "capabilities":{"tools":{}},
            "serverInfo":{"name":"fixture", "version":"1"}
        }})
    }

    async fn delayed_check(seconds: u64) -> Result<Check, String> {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        Ok(Check {
            protocol_version: PROTOCOLS[0].into(),
            tool_count: Some(0),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn stdio_allows_slow_startup_but_still_bounds_verification() {
        let result = wait_for_discovery(Transport::Stdio, delayed_check(21))
            .await
            .unwrap();
        assert_eq!(result.tool_count, Some(0));

        let error = wait_for_discovery(Transport::Stdio, delayed_check(61))
            .await
            .unwrap_err();
        assert!(error.contains("超时"));

        let error = wait_for_discovery(Transport::Http, delayed_check(21))
            .await
            .unwrap_err();
        assert!(error.contains("超时"));
    }

    #[tokio::test]
    async fn streamable_http_preserves_session_and_counts_paginated_tools() {
        let (seen, mut calls) = mpsc::unbounded_channel();
        let (url, listener) = local_http(move |request| {
            let seen = seen.clone();
            async move {
                assert_eq!(request.uri().path(), "/mcp");
                assert_eq!(request.headers()[AUTHORIZATION], "Bearer fixture");
                if request.method() == hyper::Method::DELETE {
                    assert_eq!(request.headers()[SESSION_ID], "fixture-session");
                    seen.send("DELETE".into()).unwrap();
                    return hyper::Response::builder().status(204).body(full("")).unwrap();
                }
                assert_eq!(request.method(), hyper::Method::POST);
                assert_eq!(request.headers()[ACCEPT], "application/json, text/event-stream");
                let (parts, body) = request.into_parts();
                let body: Value = serde_json::from_slice(&body.collect().await.unwrap().to_bytes()).unwrap();
                let method = body["method"].as_str().unwrap();
                seen.send(method.to_owned()).unwrap();
                if method != "initialize" {
                    assert_eq!(parts.headers[SESSION_ID], "fixture-session");
                    assert_eq!(parts.headers[PROTOCOL_VERSION], "2025-06-18");
                }
                match method {
                    "initialize" => hyper::Response::builder()
                        .header(CONTENT_TYPE, "application/json; charset=utf-8")
                        .header(SESSION_ID, "fixture-session")
                        .body(full(hello("2025-06-18").to_string())).unwrap(),
                    "notifications/initialized" => hyper::Response::builder().status(202).body(full("")).unwrap(),
                    "tools/list" if body["id"] == 2 => hyper::Response::builder()
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(full(concat!("id: ready\ndata:\n\n", "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"one\"}],\"nextCursor\":\"second\"}}\n\n"))).unwrap(),
                    "tools/list" => {
                        assert_eq!(body["params"]["cursor"], "second");
                        hyper::Response::builder().header(CONTENT_TYPE, "application/json")
                            .body(full(r#"{"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"two"}]}}"#)).unwrap()
                    }
                    _ => panic!("unexpected request"),
                }
            }
        }).await;
        let result = check(
            Client::Codex,
            json!({"url":url,"http_headers":{"Authorization":"Bearer fixture"}}),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            Check {
                protocol_version: "2025-06-18".into(),
                tool_count: Some(2)
            }
        );
        let mut methods = Vec::new();
        while let Ok(method) = calls.try_recv() {
            methods.push(method);
        }
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/list",
                "DELETE"
            ]
        );
        listener.abort();
    }

    #[tokio::test]
    async fn legacy_sse_posts_to_the_announced_path_and_reads_the_event_stream() {
        let (events, receiver) = mpsc::channel::<String>(4);
        let receiver = Arc::new(tokio::sync::Mutex::new(Some(receiver)));
        let (url, listener) = local_http(move |request| {
            let (events, receiver) = (events.clone(), receiver.clone());
            async move {
                if request.method() == hyper::Method::GET {
                    assert_eq!(request.uri().path(), "/mcp");
                    let receiver = receiver.lock().await.take().unwrap();
                    events
                        .send("event: endpoint\r\ndata: /messages?session=fixture\r\n\r\n".into())
                        .await
                        .unwrap();
                    let stream = futures_util::stream::unfold(receiver, |mut receiver| async {
                        let event = receiver.recv().await?;
                        Some((Ok(Frame::data(Bytes::from(event))), receiver))
                    });
                    return hyper::Response::builder()
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(BodyExt::boxed(StreamBody::new(stream)))
                        .unwrap();
                }
                assert_eq!(
                    request.uri().path_and_query().unwrap().as_str(),
                    "/messages?session=fixture"
                );
                let body: Value = serde_json::from_slice(
                    &request.into_body().collect().await.unwrap().to_bytes(),
                )
                .unwrap();
                let reply = match body["method"].as_str().unwrap() {
                    "initialize" => Some(hello("2024-11-05")),
                    "notifications/initialized" => None,
                    "tools/list" => Some(json!({"jsonrpc":"2.0", "id":2,"result":{"tools":[]}})),
                    _ => panic!("unexpected request"),
                };
                if let Some(reply) = reply {
                    events
                        .send(format!("event: message\ndata: {reply}\n\n"))
                        .await
                        .unwrap();
                }
                hyper::Response::builder()
                    .status(202)
                    .body(full(""))
                    .unwrap()
            }
        })
        .await;
        let result = check(Client::Code, json!({"type":"sse", "url":url}), None)
            .await
            .unwrap();
        assert_eq!(
            result,
            Check {
                protocol_version: "2024-11-05".into(),
                tool_count: Some(0)
            }
        );
        listener.abort();
    }

    #[tokio::test]
    async fn http_failures_do_not_surface_the_response_body_or_follow_redirects() {
        let (url, listener) = local_http(|request| async move {
            match request.uri().path() {
                "/mcp" => hyper::Response::builder()
                    .status(401)
                    .body(full("fixture-secret"))
                    .unwrap(),
                "/redirect" => hyper::Response::builder()
                    .status(302)
                    .header("location", "/other")
                    .body(full(""))
                    .unwrap(),
                _ => panic!("unexpected request"),
            }
        })
        .await;
        let error = check(Client::Codex, json!({"url":url}), None)
            .await
            .unwrap_err();
        assert!(error.contains("HTTP 401"));
        assert!(!error.contains("fixture-secret"));
        let url = Url::parse(&url).unwrap().join("/redirect").unwrap();
        let error = check(Client::Codex, json!({"url":url.as_str()}), None)
            .await
            .unwrap_err();
        assert!(error.contains("HTTP 302"));
        listener.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_uses_the_configured_args_environment_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("server.sh");
        let finished = dir.path().join("closed");
        std::fs::write(&fixture, r#"
            [ "$1" = "two words" ] && [ "$2" = "" ] && [ "$MCP_FIXTURE" = "fixture" ] || exit 9
            while IFS= read -r line; do
                case "$line" in
                    *'"method":"initialize"'*)
                        printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/progress"}'
                        printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}' ;;
                    *'"method":"notifications/initialized"'*) ready=1 ;;
                    *'"method":"tools/list"'*)
                        [ "$ready" = 1 ] || exit 8
                        printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"fixture"}]}}' ;;
                    *) exit 7 ;;
                esac
            done
            touch closed
        "#).unwrap();
        let result = check(
            Client::Codex,
            json!({"command":"/bin/sh", "args":[fixture, "two words", ""],
            "env":{"MCP_FIXTURE":"fixture"}, "cwd":"."}),
            Some(dir.path()),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            Check {
                protocol_version: "2025-06-18".into(),
                tool_count: Some(1)
            }
        );
        assert!(finished.exists());

        let error = check(
            Client::Codex,
            json!({"command":"/bin/sh", "args":["-c", "exit 0"]}),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error, CLOSED);
    }
}
