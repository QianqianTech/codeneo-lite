use crate::{remote::Remote, secrets};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinSet,
};

const MAX_LINE: u64 = 4_000_000;
const MAX_IN_FLIGHT: usize = 32;

pub fn run(slot: &str) -> Result<(), String> {
    let key = secrets::load(&secrets::root()?, slot)?
        .ok_or("未找到 FrameLeap Key，请在 Lite 重新配置")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "无法启动 FrameLeap 转发器")?;
    runtime.block_on(async {
        let remote = Remote::new(&key).map_err(|e| e.to_string())?;
        serve(
            BufReader::new(tokio::io::stdin()),
            tokio::io::stdout(),
            remote,
        )
        .await
    })
}

async fn send_error(out: &mpsc::Sender<Value>, id: Value, code: i32, message: &str) {
    let _ = out
        .send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
        .await;
}

async fn relay(remote: &Remote, out: &mpsc::Sender<Value>, message: &Value) {
    if let Err(err) = remote.exchange(message, out).await {
        if message.get("method").is_some() {
            if let Some(id) = message.get("id") {
                send_error(out, id.clone(), -32000, &err.to_string()).await;
            }
        }
    }
}

async fn serve<R, W>(mut input: R, mut output: W, remote: Remote) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<Value>(32);
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let mut line = serde_json::to_vec(&message).map_err(std::io::Error::other)?;
            line.push(b'\n');
            output.write_all(&line).await?;
            output.flush().await?;
        }
        Ok::<(), std::io::Error>(())
    });
    let mut requests = JoinSet::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut input)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)
            .await;
        let bytes = read.map_err(|_| "无法读取 Claude Desktop 的 MCP 请求")?;
        if bytes == 0 || tx.is_closed() {
            break;
        }
        if bytes as u64 > MAX_LINE {
            send_error(&tx, Value::Null, -32600, "MCP 请求超过大小限制").await;
            break;
        }
        let message: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) if line.iter().all(u8::is_ascii_whitespace) => continue,
            Err(_) => {
                send_error(&tx, Value::Null, -32700, "无效的 MCP JSON 请求").await;
                continue;
            }
        };
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            send_error(&tx, Value::Null, -32600, "无效的 MCP 请求").await;
            continue;
        }
        if matches!(
            message.get("method").and_then(Value::as_str),
            Some("initialize" | "notifications/initialized")
        ) {
            relay(&remote, &tx, &message).await;
            continue;
        }
        while requests.try_join_next().is_some() {}
        if requests.len() >= MAX_IN_FLIGHT {
            if let Some(id) = message.get("id") {
                send_error(&tx, id.clone(), -32000, "FrameLeap 请求过多，请稍后重试").await;
            }
            continue;
        }
        let remote = remote.clone();
        let tx = tx.clone();
        requests.spawn(async move {
            relay(&remote, &tx, &message).await;
        });
    }
    requests.shutdown().await;
    drop(tx);
    writer
        .await
        .map_err(|_| "MCP 输出任务异常")?
        .map_err(|_| "Claude Desktop 已关闭 MCP 输出".into())
}
