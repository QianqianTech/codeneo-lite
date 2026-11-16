use crate::{agents, clients, launch, secrets, transaction::Client};
use serde_json::{json, Value};
use tauri::{
    menu::{Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    App, AppHandle, Emitter, Manager,
};

const ID: &str = "lite-tray";

fn show(app: &AppHandle, page: &str) {
    #[cfg(target_os = "macos")]
    let _ = app.show();
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("lite-page", page);
    }
}

fn items(app: &AppHandle, view: &Value, client: Client) -> tauri::Result<Submenu<tauri::Wry>> {
    let status = view["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["id"] == client.id())
        .map(|entry| &entry["status"])
        .unwrap_or(&Value::Null);
    let compatible = !agents::is_added(client)
        || dirs::home_dir().is_some_and(|home| {
            agents::check_install(client, launch::cached_cli_version(client, &home).as_deref())
        });
    let can_apply = compatible
        && !matches!(
            status["state"].as_str(),
            None | Some("managed" | "unavailable")
        );
    let submenu = Submenu::with_items(app, client.name(), true, &[])?;
    let page = client.id();
    let account = status["account"].as_str().unwrap_or("无法读取");
    let model = status["model"].as_str().unwrap_or("");
    let current = if model.is_empty() {
        account.to_string()
    } else {
        format!("{account} · {model}")
    };
    submenu.append(&MenuItem::with_id(
        app,
        format!("{page}-current"),
        current,
        false,
        None::<&str>,
    )?)?;
    if status["pending"].is_object() {
        let label = if status["pending"]["issue"].is_string() {
            "待重新应用"
        } else {
            "等待 Code 会话退出"
        };
        submenu.append(&MenuItem::with_id(
            app,
            "code-pending",
            label,
            false,
            None::<&str>,
        )?)?;
    }
    submenu.append(&MenuItem::with_id(
        app,
        format!("launch-{page}"),
        if client == Client::Code || agents::is_added(client) {
            format!("在终端启动 {}", client.name())
        } else {
            format!("打开 {}", client.name())
        },
        compatible,
        None::<&str>,
    )?)?;
    if client == Client::Codex
        && dirs::home_dir().is_some_and(|home| launch::find_cli(client, &home).is_some())
    {
        submenu.append(&MenuItem::with_id(
            app,
            "launch-codex-cli",
            "启动 Codex CLI",
            true,
            None::<&str>,
        )?)?;
    }
    submenu.append(&MenuItem::with_id(
        app,
        format!("page-{page}"),
        "打开管理窗口",
        true,
        None::<&str>,
    )?)?;
    if client != Client::Workbuddy && (!agents::is_added(client) || status["state"] == "lite") {
        let label = if client == Client::Codex || agents::is_added(client) {
            "恢复原配置"
        } else {
            "恢复 Claude 官方"
        };
        submenu.append(&MenuItem::with_id(
            app,
            format!("apply-{page}-official"),
            label,
            can_apply || (agents::is_added(client) && status["state"] == "lite"),
            None::<&str>,
        )?)?;
    }
    let mut groups: std::collections::BTreeMap<&str, Vec<&Value>> = Default::default();
    for model in view["models"].as_array().into_iter().flatten() {
        let supported = model["clients"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == page));
        if !supported {
            continue;
        }
        if let Some(account) = model["account"].as_str() {
            groups.entry(account).or_default().push(model);
        }
    }
    for (id, models) in groups {
        let account = view["accounts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|account| account["id"].as_str() == Some(id));
        let name = account
            .and_then(|value| value["name"].as_str())
            .unwrap_or("模型档案");
        let group = Submenu::with_items(app, name, can_apply, &[])?;
        for profile in models {
            let (Some(id), Some(label)) = (profile["id"].as_str(), profile["name"].as_str()) else {
                continue;
            };
            let label = if status["selected"].as_str() == Some(id) {
                format!("✓ {label}")
            } else {
                label.to_string()
            };
            group.append(&MenuItem::with_id(
                app,
                format!("apply-{page}-{id}"),
                label,
                can_apply,
                None::<&str>,
            )?)?;
        }
        submenu.append(&group)?;
    }
    Ok(submenu)
}

fn menu(app: &AppHandle, view: &Value) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for client in Client::ALL {
        if agents::is_added(client) {
            let Some(home) = dirs::home_dir() else {
                continue;
            };
            if !agents::check_install(client, launch::cached_cli_version(client, &home).as_deref())
            {
                continue;
            }
        }
        menu.append(&items(app, view, client)?)?;
    }
    for (id, label) in [
        ("page-models", "模型库"),
        ("page-tools", "MCP 工具"),
        ("page-usage", "用量与费用"),
        ("open-codeneo-site", "访问 CodeNeo 官网"),
        ("quit-lite", "退出 CodeNeo Lite"),
    ] {
        menu.append(&MenuItem::with_id(app, id, label, true, None::<&str>)?)?;
    }
    Ok(menu)
}

pub fn refresh(app: &AppHandle) {
    if app.tray_by_id(ID).is_none() {
        return;
    }
    let Ok(view) = secrets::root().and_then(|root| clients::view(&root)) else {
        return;
    };
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let (Some(tray), Ok(menu)) = (handle.tray_by_id(ID), menu(&handle, &view)) {
            let _ = tray.set_menu(Some(menu));
        }
    });
}

pub fn changed(app: &AppHandle, client: &str, message: &str, error: bool) {
    refresh(app);
    let _ = app.emit(
        "lite-change",
        json!({"client": client, "message": message, "error": error}),
    );
}

pub fn install(app: &App) -> tauri::Result<()> {
    let view = secrets::root()
        .and_then(|root| clients::view(&root))
        .unwrap_or(Value::Null);
    let builder = TrayIconBuilder::with_id(ID)
        .tooltip("CodeNeo Lite")
        .menu(&menu(app.handle(), &view)?)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            if id == "quit-lite" {
                app.exit(0);
                return;
            }
            if id == "open-codeneo-site" {
                let _ = crate::open_url("https://www.codeneo.cn");
                return;
            }
            if let Some(page) = id.strip_prefix("page-") {
                show(app, page);
                return;
            }
            if let Some(page) = id.strip_prefix("launch-") {
                let codex_cli = page == "codex-cli";
                let Some(client) = Client::ALL
                    .into_iter()
                    .find(|client| client.id() == page || (codex_cli && *client == Client::Codex))
                else {
                    return;
                };
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let action = if codex_cli {
                        launch::Action::Terminal
                    } else {
                        let controls = launch::controls(client).await;
                        if (client == Client::Code || agents::is_added(client) || !controls.open)
                            && controls.terminal
                        {
                            launch::Action::Terminal
                        } else {
                            launch::Action::Open
                        }
                    };
                    let needs_directory = action == launch::Action::Terminal
                        && matches!(client, Client::Code | Client::Codex);
                    let working_dir = if needs_directory {
                        let mut dialog = rfd::AsyncFileDialog::new().set_title("选择项目目录");
                        if let Some(window) = app.get_webview_window("main") {
                            dialog = dialog.set_parent(&window);
                        }
                        dialog
                            .pick_folder()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    } else {
                        None
                    };
                    let result = if needs_directory && working_dir.is_none() {
                        Err("未选择项目目录，已取消启动。".to_string())
                    } else {
                        launch::run(client, action, working_dir.as_deref()).await
                    };
                    let error = result.is_err();
                    let message = result.unwrap_or_else(|message| message);
                    if error {
                        show(&app, if codex_cli { "codex-cli" } else { client.id() });
                    }
                    let _ = app.emit(
                        "lite-launch",
                        json!({"client": client.id(), "message": message, "error": error}),
                    );
                });
                return;
            }
            let Some(action) = id.strip_prefix("apply-") else {
                return;
            };
            let Some((page, id)) = action.split_once('-') else {
                return;
            };
            let Some(client) = Client::ALL.into_iter().find(|client| client.id() == page) else {
                return;
            };
            let model = (id != "official").then(|| id.to_string());
            let page = page.to_string();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let result = clients::switch(client, model).await;
                match result {
                    Ok(outcome) => changed(&app, &page, &outcome.message, false),
                    Err(error) => {
                        show(&app, &page);
                        changed(&app, &page, &error, true);
                    }
                }
            });
        });
    #[cfg(target_os = "macos")]
    let builder = builder
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/trayTemplate.png"
        ))?)
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.icon(tauri::image::Image::from_bytes(include_bytes!(
        "../icons/icon.png"
    ))?);
    builder.build(app)?;
    Ok(())
}

pub fn has_tray(app: &AppHandle) -> bool {
    app.tray_by_id(ID).is_some()
}
