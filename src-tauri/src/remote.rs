use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    Client, StatusCode, Url,
};
use serde_json::{json, Value};
use std::{fmt, sync::Arc, time::Duration};
use tokio::sync::{mpsc, Mutex};

const ENDPOINT: &str = "https://frameleap.qianqianai.com/mcp";
const PROTOCOL: &str = "2025-06-18";
const LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Network,
    Http(u16),
    Protocol,
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => f.write_str("无法连接 FrameLeap，请检查网络后重试"),
            Self::Http(401) => f.write_str("FrameLeap MCP Key 无效、已过期或已撤销"),
            Self::Http(403) => f.write_str("此 Key 没有 MCP 权限，请使用 FrameLeap 工作区 MCP Key"),
            Self::Http(404) => {
                f.write_str("FrameLeap MCP 端点或会话不可用，请重开 Claude Desktop 后重试")
            }
            Self::Http(429) => f.write_str("FrameLeap 请求过于频繁，请稍后重试"),
            Self::Http(status) => write!(f, "FrameLeap 暂未接受 MCP 请求（HTTP {status}）"),
            Self::Protocol => f.write_str("FrameLeap 未返回有效的 MCP 响应"),
            Self::Closed => f.write_str("Claude Desktop 已关闭 MCP 连接"),
        }
    }
}

#[derive(Clone, Default)]
struct Session {
    id: Option<HeaderValue>,
    protocol: Option<HeaderValue>,
}

#[derive(Clone)]
pub struct Remote {
    http: Client,
    endpoint: Url,
    auth: HeaderValue,
    session: Arc<Mutex<Session>>,
}

impl Remote {
    pub fn new(key: &str) -> Result<Self, Error> {
        Self::at(key, ENDPOINT, true)
    }

    fn at(key: &str, endpoint: &str, https_only: bool) -> Result<Self, Error> {
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| Error::Protocol)?;
        auth.set_sensitive(true);
        let http = Client::builder()
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .read_timeout(Duration::from_secs(45))
            .timeout(Duration::from_secs(360))
            .user_agent(concat!("codeneo-lite/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self {
            http,
            endpoint: Url::parse(endpoint).map_err(|_| Error::Protocol)?,
            auth,
            session: Default::default(),
        })
    }

    pub async fn verify(&self) -> Result<usize, Error> {
        let hello = self
            .capture(
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": PROTOCOL, "capabilities": {},
                    "clientInfo": {"name": "codeneo-lite", "version": env!("CARGO_PKG_VERSION")}
                }}),
            )
            .await?;
        if hello
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err(Error::Protocol);
        }
        let (tx, _rx) = mpsc::channel(1);
        self.exchange(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            &tx,
        )
        .await?;
        let result = self
            .capture(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or(Error::Protocol)?;
        if !tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some("fl_status"))
        {
            return Err(Error::Protocol);
        }
        Ok(tools.len())
    }

    async fn capture(&self, request: Value) -> Result<Value, Error> {
        let (tx, mut rx) = mpsc::channel(8);
        let id = request["id"].clone();
        let (sent, response) = tokio::join!(
            async {
                let result = self.exchange(&request, &tx).await;
                drop(tx);
                result
            },
            async {
                let mut result = None;
                while let Some(message) = rx.recv().await {
                    if message.get("id") == Some(&id) {
                        result = message.get("result").cloned();
                    }
                }
                result
            },
        );
        sent?;
        response.ok_or(Error::Protocol)
    }

    pub async fn exchange(
        &self,
        message: &Value,
        output: &mpsc::Sender<Value>,
    ) -> Result<(), Error> {
        let discovery = matches!(
            message.get("method").and_then(Value::as_str),
            Some("initialize" | "tools/list")
        );
        let mut emitted = false;
        let result = self.once(message, output, &mut emitted).await;
        if discovery
            && !emitted
            && matches!(result, Err(Error::Network | Error::Http(502 | 503 | 504)))
        {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return self.once(message, output, &mut emitted).await;
        }
        result
    }

    async fn once(
        &self,
        message: &Value,
        output: &mpsc::Sender<Value>,
        emitted: &mut bool,
    ) -> Result<(), Error> {
        let method = message.get("method").and_then(Value::as_str);
        let id = method.and_then(|_| message.get("id"));
        let init = method == Some("initialize");
        let mut request = self
            .http
            .post(self.endpoint.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(AUTHORIZATION, self.auth.clone())
            .json(message);
        if !init {
            let session = self.session.lock().await;
            if let Some(id) = &session.id {
                request = request.header("Mcp-Session-Id", id.clone());
            }
            if let Some(protocol) = &session.protocol {
                request = request.header("MCP-Protocol-Version", protocol.clone());
            }
        }
        let mut response = request.send().await.map_err(|_| Error::Network)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http(status.as_u16()));
        }
        if id.is_none() {
            return Ok(());
        }
        if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
            return Err(Error::Protocol);
        }
        let mut sid = response.headers().get("mcp-session-id").cloned();
        if let Some(value) = &mut sid {
            value.set_sensitive(true);
        }
        let content = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if content.starts_with("text/event-stream") {
            let mut events = Sse::default();
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
                for event in events.push(&chunk)? {
                    let value = event.json()?;
                    if self.relay(value, id, init, &sid, output, emitted).await? {
                        return Ok(());
                    }
                }
            }
            Err(Error::Protocol)
        } else if content.starts_with("application/json") {
            let mut buffer = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
                extend(&mut buffer, &chunk)?;
            }
            let value = serde_json::from_slice(&buffer).map_err(|_| Error::Protocol)?;
            if self.relay(value, id, init, &sid, output, emitted).await? {
                Ok(())
            } else {
                Err(Error::Protocol)
            }
        } else {
            Err(Error::Protocol)
        }
    }

    async fn relay(
        &self,
        value: Value,
        id: Option<&Value>,
        init: bool,
        sid: &Option<HeaderValue>,
        output: &mpsc::Sender<Value>,
        emitted: &mut bool,
    ) -> Result<bool, Error> {
        if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(Error::Protocol);
        }
        let reply = id == value.get("id")
            && (value.get("result").is_some() || value.get("error").is_some());
        if init && reply && value.get("result").is_some() {
            let protocol = value
                .pointer("/result/protocolVersion")
                .and_then(Value::as_str)
                .and_then(|raw| HeaderValue::from_str(raw).ok())
                .ok_or(Error::Protocol)?;
            *self.session.lock().await = Session {
                id: sid.clone(),
                protocol: Some(protocol),
            };
        }
        output.send(value).await.map_err(|_| Error::Closed)?;
        *emitted = true;
        Ok(reply)
    }
}

pub(crate) fn extend(buffer: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    if buffer.len() + bytes.len() > LIMIT {
        return Err(Error::Protocol);
    }
    buffer.extend_from_slice(bytes);
    Ok(())
}

pub(crate) struct Event {
    pub kind: Option<String>,
    pub data: Vec<u8>,
}

impl Event {
    pub fn json(&self) -> Result<Value, Error> {
        serde_json::from_slice(&self.data).map_err(|_| Error::Protocol)
    }
}

#[derive(Default)]
pub(crate) struct Sse {
    line: Vec<u8>,
    data: Vec<u8>,
    kind: Option<String>,
}

impl Sse {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Event>, Error> {
        let mut events = Vec::new();
        for part in chunk.split_inclusive(|b| *b == b'\n') {
            extend(&mut self.line, part)?;
            if part.last() != Some(&b'\n') {
                continue;
            }
            self.line.pop();
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            if self.line.is_empty() {
                let kind = self.kind.take();
                self.data.pop();
                if !self.data.is_empty() {
                    events.push(Event {
                        kind,
                        data: std::mem::take(&mut self.data),
                    });
                }
            } else if let Some(kind) = self.line.strip_prefix(b"event:") {
                let kind = kind.strip_prefix(b" ").unwrap_or(kind);
                self.kind = Some(String::from_utf8_lossy(kind).into_owned());
            } else if let Some(data) = self.line.strip_prefix(b"data:") {
                extend(&mut self.data, data.strip_prefix(b" ").unwrap_or(data))?;
                extend(&mut self.data, b"\n")?;
            }
            self.line.clear();
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
        time::Instant,
    };

    const KEY: &str = "fl_test000000000000000000000000000000000000000";

    fn mock(
        responses: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (Remote, thread::JoinHandle<Vec<(String, Value)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let remote = Remote::at(KEY, &endpoint, false).unwrap();
        let join = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut calls = Vec::new();
            for (status, headers, body) in responses {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(err)
                            if err.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(err) => panic!("mock MCP did not receive the expected request: {err}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut input_headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    input_headers.push_str(&line.to_ascii_lowercase());
                }
                let len: usize = input_headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let mut input_body = vec![0; len];
                reader.read_exact(&mut input_body).unwrap();
                calls.push((input_headers, serde_json::from_slice(&input_body).unwrap()));
                write!(stream, "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
            }
            calls
        });
        (remote, join)
    }

    #[tokio::test]
    async fn discovery_keeps_the_session_and_retries_the_tool_list() {
        let (remote, mock) = mock(vec![
            (
                "200 OK",
                "Content-Type: application/json\r\nMcp-Session-Id: test-session\r\n",
                r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}"#,
            ),
            ("202 Accepted", "", ""),
            ("503 Service Unavailable", "", ""),
            (
                "200 OK",
                "Content-Type: application/json\r\n",
                r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"fl_status"}]}}"#,
            ),
        ]);
        assert_eq!(remote.verify().await.unwrap(), 1);
        let calls = mock.join().unwrap();
        assert_eq!(calls[0].1["method"], "initialize");
        assert!(!calls[0].0.contains("mcp-session-id:"));
        for (headers, _) in &calls {
            assert!(headers.starts_with("post /mcp http/1.1"));
            assert!(headers.contains(&format!("authorization: bearer {KEY}")));
        }
        for (headers, _) in &calls[1..] {
            assert!(headers.contains("mcp-session-id: test-session"));
            assert!(headers.contains("mcp-protocol-version: 2025-06-18"));
        }
        assert_eq!(calls[1].1["method"], "notifications/initialized");
        assert_eq!(calls[2].1, calls[3].1);
    }

    #[tokio::test]
    async fn rejects_redirects_and_does_not_replay_tool_calls() {
        let (remote, server) = mock(vec![(
            "302 Found",
            "Location: http://127.0.0.1:9/other\r\n",
            "",
        )]);
        assert_eq!(remote.verify().await, Err(Error::Http(302)));
        server.join().unwrap();

        let (remote, server) = mock(vec![("503 Service Unavailable", "", "")]);
        let (tx, _rx) = mpsc::channel(1);
        let tool = json!({"jsonrpc": "2.0", "method": "tools/call", "id": 3, "params": {"name":"fl_create_video"}});
        assert_eq!(remote.exchange(&tool, &tx).await, Err(Error::Http(503)));
        assert_eq!(server.join().unwrap()[0].1, tool);
    }

    #[test]
    fn sse_preserves_progress_and_results_across_chunks() {
        let mut sse = Sse::default();
        assert!(sse
            .push(b": keep-alive\r\n\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\r\n")
            .unwrap()
            .is_empty());
        let first = sse
            .push(b"data: \"method\":\"notifications/progress\"}\r\n\r\ndata: {\"jsonrpc\":\"2.")
            .unwrap();
        assert_eq!(first[0].kind.as_deref(), Some("message"));
        assert_eq!(first[0].json().unwrap()["method"], "notifications/progress");
        let last = sse.push(b"0\",\"id\":3,\"result\":{}}\n\n").unwrap();
        assert_eq!(last[0].json().unwrap()["id"], 3);
    }
}
