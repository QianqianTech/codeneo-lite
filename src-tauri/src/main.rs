#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod agents;
mod bridge;
mod clients;
mod codex;
mod codex_inference;
mod desktop;
mod desktop_effort;
mod desktop_inference;
mod desktop_models;
mod files;
mod gateway;
mod jsonc;
mod launch;
mod mcp;
mod models;
mod remote;
mod secrets;
mod setup;
mod transaction;
mod tray;
mod usage;
mod usage_prices;
mod vault;
mod workbuddy;

use claudekeeper_net as model_transport;

use tauri::Manager;
use transaction::Client;

#[tauri::command]
fn frameleap_status() -> Result<setup::Status, String> {
    setup::status()
}

#[tauri::command]
async fn configure_frameleap(
    key: String,
    configuring: tauri::State<'_, tokio::sync::Mutex<()>>,
) -> Result<setup::Configured, String> {
    let _guard = configuring
        .try_lock()
        .map_err(|_| "正在配置 FrameLeap，请稍候")?;
    setup::configure(&key).await
}

#[tauri::command]
async fn lite_gateway_ready() -> bool {
    gateway::SERVICE.lock().await.is_ready()
}

#[tauri::command]
async fn lite_overview(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let mut view = work(|| clients::view(&secrets::root()?)).await?;
    if let Some(clients) = view["clients"].as_array_mut() {
        let mut tasks = tokio::task::JoinSet::new();
        for client in Client::ALL {
            tasks.spawn(async move { (client, launch::controls(client).await) });
        }
        while let Some(result) = tasks.join_next().await {
            let (client, controls) = result.map_err(|_| "无法读取客户端的启动状态")?;
            if let Some(entry) = clients.iter_mut().find(|entry| entry["id"] == client.id()) {
                if let Some(issue) = controls.config_issue.as_ref() {
                    entry["status"]["state"] = "unavailable".into();
                    entry["status"]["issue"] = issue.clone().into();
                }
                entry["installed"] = controls.installed.into();
                entry["launch"] =
                    serde_json::to_value(controls).map_err(|_| "无法显示客户端的启动状态")?;
            }
        }
        clients.retain(|entry| entry["launch"]["unsupported"] != true);
        if let Some(desktop) = clients.iter_mut().find(|entry| entry["id"] == "desktop") {
            if desktop["status"]["local_gateway"] == true {
                match desktop_models::status(&secrets::root()?).await {
                    Ok((verified, pending)) => {
                        desktop["status"]["gateway_ready"] = true.into();
                        desktop["status"]["gateway_verified"] = verified.into();
                        desktop["status"]["recover_session"] = serde_json::json!(pending);
                    }
                    Err(error) => append_issue(&mut desktop["status"], error),
                }
            }
        }
        if let Some(codex) = clients.iter_mut().find(|entry| entry["id"] == "codex") {
            if codex["status"]["local_gateway"] == true {
                match codex::gateway_status(&secrets::root()?).await {
                    Ok(()) => codex["status"]["gateway_ready"] = true.into(),
                    Err(error) => append_issue(&mut codex["status"], error),
                }
            }
        }
    }
    let visible: std::collections::HashSet<_> = view["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect();
    for section in ["models", "accounts", "presets"] {
        if let Some(entries) = view[section].as_array_mut() {
            for entry in entries {
                if let Some(clients) = entry["clients"].as_array_mut() {
                    clients.retain(|id| id.as_str().is_some_and(|id| visible.contains(id)));
                }
            }
        }
    }
    tray::refresh(&app);
    Ok(view)
}

fn append_issue(status: &mut serde_json::Value, error: String) {
    let issue = status["issue"].as_str().unwrap_or("");
    status["issue"] = if issue.is_empty() {
        error
    } else {
        format!("{issue}。{error}")
    }
    .into();
}

#[tauri::command]
async fn launch_client(client: Client, action: launch::Action) -> Result<String, String> {
    launch::run(client, action).await
}

#[tauri::command]
async fn switch_model_mode(
    app: tauri::AppHandle,
    client: Client,
    id: Option<String>,
    connection: Option<gateway::Connection>,
    action: Option<launch::Action>,
) -> Result<String, String> {
    if id.is_none() && connection.is_some() {
        return Err("请选择第三方档案后再设置连接方式。".into());
    }
    let result = match (client, action) {
        (Client::Desktop, None | Some(launch::Action::Open | launch::Action::Restart)) => {
            launch::apply_desktop(id.as_deref(), connection, action).await
        }
        (Client::Codex, None | Some(launch::Action::Open | launch::Action::Terminal)) => {
            let root = secrets::root()?;
            let target = id.as_deref().map_or(codex::Target::Official, |id| {
                codex::Target::Profile(id, connection)
            });
            let outcome = codex::switch(&root, target).await?;
            if let Some(action) = action {
                match launch::run(client, action).await {
                    Ok(message) => Ok(format!("Codex 配置已写入。{message}")),
                    Err(error) => Err(format!("Codex 配置已写入。{error}")),
                }
            } else {
                Ok(outcome.message.into_owned())
            }
        }
        (Client::Code, None) if connection.is_none() => clients::switch(client, id)
            .await
            .map(|outcome| outcome.message.into_owned()),
        _ => Err("此客户端不支持该模式或启动方式。".into()),
    };
    tray::refresh(&app);
    result
}

#[tauri::command]
async fn saved_model_catalog(
    client: Client,
    id: String,
) -> Result<model_transport::ProviderCatalog, String> {
    let root = secrets::root()?;
    let input = models::saved_input(&root, client, &id, false)?;
    models::catalog(&root, input, Some(client)).await
}

#[tauri::command]
async fn verify_saved_model(
    client: Client,
    id: String,
    connection: Option<gateway::Connection>,
) -> Result<models::Check, String> {
    let root = secrets::root()?;
    let input = models::saved_input(
        &root,
        client,
        &id,
        connection == Some(gateway::Connection::Gateway),
    )?;
    models::test(&root, input, Some(client)).await
}

#[tauri::command]
async fn save_model(
    app: tauri::AppHandle,
    input: models::Input,
    client: Option<Client>,
) -> Result<String, String> {
    let id = work(move || models::save(&secrets::root()?, input, client)).await?;
    tray::refresh(&app);
    Ok(id)
}

#[tauri::command]
async fn remove_model(app: tauri::AppHandle, id: String) -> Result<(), String> {
    work(move || models::remove(&secrets::root()?, &id)).await?;
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
async fn save_models(
    app: tauri::AppHandle,
    input: models::Input,
    client: Option<Client>,
    selected: Vec<models::SelectedModel>,
) -> Result<Vec<String>, String> {
    let ids = work(move || models::save_models(&secrets::root()?, input, client, selected)).await?;
    tray::refresh(&app);
    Ok(ids)
}

#[tauri::command]
async fn model_catalog(
    input: models::Input,
    client: Option<Client>,
) -> Result<model_transport::ProviderCatalog, String> {
    models::catalog(&secrets::root()?, input, client).await
}

#[tauri::command]
async fn test_model(input: models::Input, client: Option<Client>) -> Result<models::Check, String> {
    models::test(&secrets::root()?, input, client).await
}

#[tauri::command]
async fn list_mcp(client: Client, selection: mcp::Selection) -> Result<mcp::Configured, String> {
    work(move || mcp::list(&secrets::root()?, client, selection)).await
}

#[tauri::command]
async fn get_mcp(
    client: Client,
    destination: mcp::Destination,
    name: String,
) -> Result<mcp::Editable, String> {
    work(move || mcp::editable(client, destination, &name)).await
}

#[tauri::command]
async fn verify_mcp(
    app: tauri::AppHandle,
    client: Client,
    destination: mcp::Destination,
    target: mcp::Verification,
) -> Result<mcp::VerificationResult, String> {
    let resources = mcp_resources(&app)?;
    let (server, location) = work(move || {
        let root = secrets::root()?;
        mcp::migrate_legacy_playwright(&root, client, &destination, &target, &resources)?;
        mcp::verification_entry(&root, client, destination, target)
    })
    .await?;
    let revision = mcp::revision(&location.path, &server);
    let result = mcp::check(client, server, location.directory.as_deref()).await;
    Ok(mcp::VerificationResult { revision, result })
}

#[tauri::command]
async fn save_mcp(
    client: Client,
    destination: mcp::Destination,
    input: mcp::Input,
) -> Result<(), String> {
    work(move || mcp::save(&secrets::root()?, client, destination, input)).await
}

#[tauri::command]
async fn save_mcp_permissions(
    destination: mcp::Destination,
    name: String,
    revision: String,
    always_allow: Vec<String>,
) -> Result<(), String> {
    work(move || {
        mcp::save_permissions(&secrets::root()?, destination, &name, &revision, always_allow)
    })
    .await
}

#[tauri::command]
async fn remove_mcp(
    client: Client,
    destination: mcp::Destination,
    name: String,
) -> Result<(), String> {
    work(move || mcp::remove(&secrets::root()?, client, destination, &name)).await
}

#[tauri::command]
async fn set_mcp_enabled(
    client: Client,
    destination: mcp::Destination,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    work(move || mcp::set_enabled(&secrets::root()?, client, destination, &name, enabled)).await
}

#[tauri::command]
async fn undo_mcp(client: Client, destination: mcp::Destination) -> Result<(), String> {
    work(move || mcp::undo(&secrets::root()?, client, destination)).await
}

#[tauri::command]
fn list_mcp_catalog(client: Client) -> Result<Vec<mcp::catalog::Card<'static>>, String> {
    mcp::catalog::list(client)
}

#[tauri::command]
fn open_mcp_source(id: String) -> Result<(), String> {
    open_url(mcp::catalog::source(&id)?)
}

fn mcp_resources(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    #[cfg(debug_assertions)]
    {
        let _ = app;
        return Ok(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    }
    #[cfg(not(debug_assertions))]
    {
        app.path()
            .resource_dir()
            .map_err(|_| "无法定位内置 MCP 资源".into())
    }
}

#[tauri::command]
async fn prepare_mcp_preset(
    app: tauri::AppHandle,
    client: Client,
    scope: mcp::Scope,
    id: String,
    value: Option<String>,
) -> Result<mcp::Input, String> {
    let resources = mcp_resources(&app)?;
    work(move || {
        mcp::catalog::prepare(client, scope, &id, value.as_deref(), &resources)
    })
    .await
}

#[tauri::command]
async fn parse_mcp_import(client: Client, text: String) -> Result<mcp::Imported, String> {
    work(move || mcp::parse_import(client, &text)).await
}

#[tauri::command]
async fn pick_mcp_directory(window: tauri::WebviewWindow) -> Result<Option<String>, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_title("选择目录")
        .set_parent(&window)
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    let path = mcp::directory(handle.path())?;
    Ok(Some(path.to_str().ok_or("目录路径编码无效")?.into()))
}

#[tauri::command]
async fn token_usage(app: tauri::AppHandle, days: u8) -> Result<usage::Report, String> {
    work(move || {
        let home = agents::home()?;
        let visible: Vec<_> = Client::ALL
            .into_iter()
            .filter(|client| {
                !agents::is_added(*client)
                    || agents::check_install(
                        *client,
                        launch::cached_cli_version(*client, &home).as_deref(),
                    )
            })
            .collect();
        usage::summarize(days, &visible, &app.state::<usage::UsageCache>())
    })
    .await
}

#[tauri::command]
fn open_reference(source: usage_prices::Reference) -> Result<(), String> {
    open_url(source.url())
}

fn open_url(url: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    command
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| "无法打开浏览器".into())
}

#[tauri::command]
async fn apply_model(
    app: tauri::AppHandle,
    client: Client,
    id: Option<String>,
) -> Result<clients::Outcome, String> {
    let result = clients::switch(client, id).await?;
    tray::refresh(&app);
    Ok(result)
}

#[tauri::command]
async fn apply_agent_models(
    app: tauri::AppHandle,
    client: Client,
    models: Vec<agents::RequestedModel>,
) -> Result<clients::Outcome, String> {
    let result =
        work(move || clients::apply_agent_models(&secrets::root()?, client, models)).await?;
    tray::refresh(&app);
    Ok(result)
}

#[tauri::command]
async fn apply_workbuddy_models(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<clients::Outcome, String> {
    let result = work(move || clients::apply_workbuddy_models(&secrets::root()?, ids)).await?;
    tray::refresh(&app);
    Ok(result)
}

#[tauri::command]
fn open_agent_help(client: Client, install: Option<bool>) -> Result<(), String> {
    let spec = agents::spec(client);
    let url = if install.unwrap_or(false) {
        spec.install
    } else {
        spec.help
    }
    .ok_or("此客户端暂无帮助链接")?;
    open_url(url)
}

#[tauri::command]
async fn undo_model(app: tauri::AppHandle, client: Client) -> Result<clients::Outcome, String> {
    let result = clients::undo(client).await?;
    tray::refresh(&app);
    Ok(result)
}

#[tauri::command]
async fn recover_desktop_model_session(request: u64) -> Result<String, String> {
    desktop_models::recover_session(&secrets::root()?, request).await
}

#[tauri::command]
async fn cancel_code_switch(app: tauri::AppHandle) -> Result<(), String> {
    work(|| clients::cancel_pending(&secrets::root()?)).await?;
    tray::refresh(&app);
    Ok(())
}

async fn work<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|_| "Lite 无法完成本机配置操作")?
}

fn start_pending_check(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            if let Ok(Some(result)) = work(|| clients::process_pending(&secrets::root()?)).await {
                match result {
                    Ok(message) => tray::changed(&app, "code", &message, false),
                    Err(error) => tray::changed(&app, "code", &error, true),
                }
            }
        }
    });
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let flag = args.next();
    if flag.as_deref() == Some(std::ffi::OsStr::new(codex::KEY_FLAG)) {
        let reference = args.next().and_then(|arg| arg.into_string().ok());
        let result = reference
            .filter(|_| args.next().is_none())
            .ok_or_else(|| "缺少 Codex 凭据引用，请在 Lite 重新应用模型".to_string())
            .and_then(|reference| codex::read_key(&secrets::root()?, &reference));
        match result {
            Ok(key) => println!("{key}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if flag.as_deref() == Some(std::ffi::OsStr::new(desktop::BRIDGE_FLAG)) {
        let slot = args.next().and_then(|arg| arg.into_string().ok());
        let result = match slot.filter(|_| args.next().is_none()) {
            Some(slot) => bridge::run(&slot),
            None => Err("FrameLeap 凭据引用缺失，请在 Lite 重新配置".into()),
        };
        std::process::exit(match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("[FrameLeap MCP] {error}");
                1
            }
        });
    }
    tauri::Builder::default()
        .manage(tokio::sync::Mutex::new(()))
        .manage(usage::UsageCache::default())
        .setup(|app| {
            if let Ok(root) = secrets::root() {
                if files::private_dir(&root).is_ok() {
                    claudekeeper_app::diagnostics::initialize(root);
                }
            }
            let _ = tray::install(app);
            start_pending_check(app.handle().clone());
            tauri::async_runtime::spawn(async {
                if let Err(error) = gateway::resume().await {
                    claudekeeper_app::diagnostics::record("warn", "gateway-startup", &error);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray::has_tray(window.app_handle()) && window.hide().is_ok() {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            apply_agent_models,
            apply_workbuddy_models,
            open_agent_help,
            undo_mcp,
            frameleap_status,
            configure_frameleap,
            lite_gateway_ready,
            lite_overview,
            launch_client,
            switch_model_mode,
            saved_model_catalog,
            verify_saved_model,
            recover_desktop_model_session,
            save_model,
            save_models,
            remove_model,
            model_catalog,
            test_model,
            list_mcp,
            list_mcp_catalog,
            open_mcp_source,
            prepare_mcp_preset,
            parse_mcp_import,
            pick_mcp_directory,
            get_mcp,
            verify_mcp,
            save_mcp,
            save_mcp_permissions,
            remove_mcp,
            set_mcp_enabled,
            token_usage,
            open_reference,
            apply_model,
            undo_model,
            cancel_code_switch
        ])
        .run(tauri::generate_context!())
        .expect("CodeNeo Lite 无法启动");
}
