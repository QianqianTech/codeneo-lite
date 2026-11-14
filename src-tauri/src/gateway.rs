use crate::{
    clients, codex, desktop_inference, desktop_models, files, secrets, transaction::Client, vault,
};
use serde::Deserialize;
use std::path::Path;

pub(crate) const DEFAULT_PORT: u16 = 15733;
const CODEX_PATH: &str = "/codex/v1";
pub(crate) const CODEX_RESPONSES: &str = "/codex/v1/responses";

pub(crate) static SERVICE: tokio::sync::Mutex<desktop_inference::Service> =
    tokio::sync::Mutex::const_new(desktop_inference::Service::new());

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Connection {
    Direct,
    Gateway,
}

pub(crate) fn desktop_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub(crate) fn codex_url(port: u16) -> String {
    format!("{}{CODEX_PATH}", desktop_url(port))
}

pub(crate) fn desktop_port(url: &str) -> Option<u16> {
    let port = url.strip_prefix("http://127.0.0.1:")?;
    let parsed: u16 = port.parse().ok()?;
    (parsed != 0 && port == parsed.to_string()).then_some(parsed)
}

pub(crate) fn codex_port(url: &str) -> Option<u16> {
    desktop_port(url.strip_suffix(CODEX_PATH)?)
}

pub(crate) fn port_changed(client: Client, port: u16) -> String {
    format!(
        "已为 CodeNeo Lite 网关分配端口 {port}。请重新应用档案并重启 {}。",
        client.name()
    )
}

pub(crate) fn desktop_key(root: &Path, create: bool) -> Result<String, String> {
    let path = root.join("desktop-gateway-key.bin");
    if let Some(bytes) = vault::read(root, &path)? {
        let key = String::from_utf8(bytes).map_err(|_| "CodeNeo Lite网关密钥无效")?;
        if key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(key);
        }
        return Err("CodeNeo Lite网关密钥无效，请重新应用档案。".into());
    }
    if !create {
        return Err("CodeNeo Lite网关密钥缺失，请重新应用档案。".into());
    }
    let key = format!("{}{}", vault::id()?, vault::id()?);
    vault::write(root, &path, key.as_bytes())?;
    Ok(key)
}

pub(crate) async fn prepare(
    service: &mut desktop_inference::Service,
    root: &Path,
    configured_port: Option<u16>,
) -> Result<u16, String> {
    let path = root.join("gateway-port");
    let previous = files::read(&path)?;
    let key = desktop_key(root, true)?;
    let preferred = configured_port.or_else(|| {
        previous
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|port| port.trim().parse::<u16>().ok())
            .filter(|port| *port != 0)
    });
    let port = service
        .bind(
            &key,
            preferred.unwrap_or(DEFAULT_PORT),
            &root.join("desktop-sessions.json"),
        )
        .await?
        .port();
    let current = port.to_string();
    if previous.as_deref() != Some(current.as_bytes()) {
        files::replace(&path, previous.as_deref(), current.as_bytes())?;
    }
    Ok(port)
}

pub(crate) async fn prepare_existing(
    service: &mut desktop_inference::Service,
    root: &Path,
    client: Client,
    preferred: u16,
) -> Result<(), String> {
    let port = prepare(service, root, Some(preferred)).await?;
    if port != preferred {
        return Err(port_changed(client, port));
    }
    Ok(())
}

pub(crate) async fn resume() -> Result<(), String> {
    let root = secrets::root()?;
    let _lock = clients::lock_for_launch(&root, Client::Desktop)?;
    let desktop = desktop_models::sync(&root).await;
    let codex = codex::sync_gateway(&root).await;
    desktop.and(codex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn occupied_port_falls_back_and_restarts_on_the_saved_port() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let preferred = occupied.local_addr().unwrap().port();
        let data = tempfile::tempdir().unwrap();
        let root = data.path();
        let mut service = desktop_inference::Service::new();
        let mut duplicate = desktop_inference::Service::new();

        let error = prepare_existing(&mut service, root, Client::Desktop, preferred)
            .await
            .unwrap_err();
        let port = service.port().unwrap();
        assert_ne!(port, preferred);
        assert_eq!(error, port_changed(Client::Desktop, port));
        assert_eq!(
            prepare(&mut service, root, Some(preferred)).await.unwrap(),
            port
        );
        assert!(prepare(&mut duplicate, root, Some(preferred))
            .await
            .is_err());

        let other = tempfile::tempdir().unwrap();
        let other_port = prepare(&mut duplicate, other.path(), Some(preferred))
            .await
            .unwrap();
        assert_ne!(other_port, preferred);
        assert_ne!(other_port, port);

        // Wait for the aborted listener task to release its socket.
        drop(service);
        for _ in 0..100 {
            if let Ok(socket) = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
                drop(socket);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut restarted = desktop_inference::Service::new();
        assert_eq!(prepare(&mut restarted, root, None).await.unwrap(), port);
    }
}
