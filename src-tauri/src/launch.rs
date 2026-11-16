use crate::{agents, clients, codex, gateway::Connection, model_transport::EffortLevel, secrets, transaction::Client};
use claudekeeper_claude::{cli_candidates, is_launchable_file, managed_key_names};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::ffi::OsStr;
use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

#[path = "launch_desktop.rs"]
mod desktop;
#[cfg(windows)]
#[path = "launch_windows.rs"]
mod windows;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Open,
    Restart,
    Terminal,
}

#[derive(Default, Serialize)]
pub struct Controls {
    pub open: bool,
    pub restart: bool,
    pub terminal: bool,
    pub note: Option<String>,
    pub version: Option<String>,
    pub installed: bool,
    pub unsupported: bool,
    pub config_issue: Option<String>,
}

impl Controls {
    fn issue(note: impl Into<String>) -> Self {
        Self {
            note: Some(note.into()),
            ..Self::default()
        }
    }
}

pub async fn controls(client: Client) -> Controls {
    let Some(home) = dirs::home_dir() else {
        return Controls::issue("找不到当前用户的主目录。");
    };
    if client == Client::Desktop {
        return desktop::controls(&home).await;
    }
    let has_cli = find_cli(client, &home).is_some();
    let has_app = find_app(client, &home).await.is_some();
    let version = if has_cli && (agents::is_added(client) || client == Client::Codex) {
        let home = home.clone();
        tokio::task::spawn_blocking(move || cli_version(client, &home))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    if agents::is_added(client) {
        return agent_controls(
            client,
            has_cli,
            version,
            agents::check_configuration(client),
        );
    }
    let missing = match client {
        Client::Code => "未找到 Claude Code 命令。安装后刷新状态。",
        Client::Codex => "未找到 Codex 桌面应用。安装后刷新状态。",
        Client::Workbuddy => "未找到可启动的 WorkBuddy。安装后刷新状态。",
        _ => "未找到客户端。安装后刷新状态。",
    };
    Controls {
        open: has_app,
        terminal: has_cli,
        note: (!has_app && !has_cli).then(|| missing.into()),
        version,
        installed: has_app || has_cli,
        ..Controls::default()
    }
}

fn agent_controls(
    client: Client,
    installed: bool,
    version: Option<String>,
    config: Result<(), agents::ConfigurationIssue>,
) -> Controls {
    let unsupported = version
        .as_deref()
        .is_some_and(|value| !agents::installed_compatible(client, Some(value)))
        || matches!(config, Err(agents::ConfigurationIssue::Unsupported));
    let config_issue = if installed && version.is_none() {
        Some(format!(
            "无法读取 {} 的版本，请在终端确认后刷新。",
            client.name()
        ))
    } else {
        config.err().map(agents::ConfigurationIssue::message)
    };
    Controls {
        terminal: installed && !unsupported && config_issue.is_none(),
        note: (!installed).then(|| {
            format!(
                "未检测到 {} 命令。安装兼容版本后点击刷新。",
                agents::spec(client).cli.unwrap_or(client.name())
            )
        }),
        version,
        installed,
        unsupported,
        config_issue,
        ..Controls::default()
    }
}

pub async fn run(
    client: Client,
    action: Action,
    working_dir: Option<&Path>,
) -> Result<String, String> {
    run_with_effort(client, action, None, working_dir).await
}

pub async fn run_with_effort(
    client: Client,
    action: Action,
    effort: Option<EffortLevel>,
    working_dir: Option<&Path>,
) -> Result<String, String> {
    if effort.is_some() && (client, action) != (Client::Code, Action::Terminal) {
        return Err("此启动方式没有 Claude Code 思考档位。".into());
    }
    let home = dirs::home_dir().ok_or("找不到当前用户的主目录")?;
    match (client, action) {
        (Client::Desktop, Action::Open | Action::Restart) => {
            desktop::run(&home, Some(action), None).await
        }
        (Client::Code | Client::Codex, Action::Terminal) => {
            open_terminal(client, &home, effort, project_directory(working_dir)?).await
        }
        (_, Action::Terminal) if agents::is_added(client) => {
            open_agent_terminal(client, &home).await
        }
        (Client::Codex | Client::Workbuddy, Action::Open) => {
            if let Some(app) = find_app(client, &home).await {
                let root = secrets::root()?;
                let _lock = clients::lock_for_launch(&root, client)?;
                if client == Client::Codex {
                    codex::sync_gateway(&root).await?;
                }
                app.open().await?;
                Ok(format!("已请求打开 {}。", client.name()))
            } else if client == Client::Codex {
                Err("未找到 Codex 桌面应用。可以从「Codex CLI」选择项目目录并启动。".into())
            } else {
                Err("未找到可启动的 WorkBuddy，请先安装。".into())
            }
        }
        _ => Err("此客户端不支持该启动方式。".into()),
    }
}

pub async fn apply_desktop(
    id: Option<&str>,
    connection: Option<Connection>,
    action: Option<Action>,
) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("找不到当前用户的主目录")?;
    desktop::run(&home, action, Some((id, connection))).await
}

fn on_path(name: &str) -> impl Iterator<Item = PathBuf> + '_ {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .filter(|dir| dir.is_absolute())
        .map(move |dir| dir.join(name))
}

fn cli_paths(client: Client, home: &Path) -> Vec<PathBuf> {
    let Some(name) = agents::spec(client).cli else {
        return Vec::new();
    };
    let mut paths = match client {
        Client::Code => cli_candidates(home),
        _ => vec![home.join(".local/bin").join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        })],
    };
    if client != Client::Code && !cfg!(windows) {
        paths.splice(..0, on_path(name));
        if client == Client::Opencode {
            paths.push(home.join(".opencode/bin/opencode"));
        } else if client == Client::Kimi {
            paths.push(home.join(".kimi-code/bin/kimi"));
        }
        paths.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(|dir| Path::new(dir).join(name)));
    }
    #[cfg(windows)]
    {
        let names = [format!("{name}.exe"), format!("{name}.cmd")];
        paths.splice(
            ..0,
            env::var_os("PATH").into_iter().flat_map(|value| {
                env::split_paths(&value)
                    .filter(|dir| dir.is_absolute())
                    .flat_map(|dir| names.iter().map(move |file| dir.join(file)))
                    .collect::<Vec<_>>()
            }),
        );
        for extension in ["exe", "cmd"] {
            let file = format!("{name}.{extension}");
            if let Some(roaming) = dirs::config_dir() {
                paths.push(roaming.join("npm").join(&file));
            }
        }
    }
    #[cfg(target_os = "macos")]
    if client == Client::Codex {
        for root in [PathBuf::from("/Applications"), home.join("Applications")] {
            for name in ["Codex.app", "ChatGPT.app"] {
                let bundle = root.join(name).join("Contents/Resources");
                paths.push(bundle.join("codex-cli/bin/codex"));
                paths.push(bundle.join("codex"));
            }
        }
    }
    paths
        .into_iter()
        .filter(|path| {
            path.is_absolute()
                && is_launchable_file(path)
                && (client != Client::Code || !desktop::is_desktop_binary(path))
        })
        .collect()
}

pub fn find_cli(client: Client, home: &Path) -> Option<PathBuf> {
    cli_paths(client, home).into_iter().next()
}

type Versions = BTreeMap<PathBuf, (Instant, Option<String>)>;
static CLI_VERSIONS: LazyLock<Mutex<Versions>> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub fn cached_cli_version(client: Client, home: &Path) -> Option<String> {
    let binary = find_cli(client, home)?;
    CLI_VERSIONS.lock().ok()?.get(&binary)?.1.clone()
}

pub fn cli_version(client: Client, home: &Path) -> Option<String> {
    let binary = find_cli(client, home)?;
    if let Some((checked, version)) = CLI_VERSIONS.lock().ok()?.get(&binary) {
        if checked.elapsed() < Duration::from_secs(5) {
            return version.clone();
        }
    }
    let version = probe_version(&binary, home);
    CLI_VERSIONS
        .lock()
        .ok()?
        .insert(binary, (Instant::now(), version.clone()));
    version
}

fn probe_version(binary: &Path, home: &Path) -> Option<String> {
    use std::io::Read;
    let mut command = Command::new(binary);
    command
        .arg("--version")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
        if binary
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"))
        {
            command = Command::new("cmd.exe");
            command
                .creation_flags(0x08000000)
                .raw_arg(r#"/D /V:OFF /S /C ""%CODENEO_LITE_CLI%" --version""#)
                .env("CODENEO_LITE_CLI", binary)
                .current_dir(home)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
        }
    }
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut version = String::new();
    child.stdout?.take(2048).read_to_string(&mut version).ok()?;
    version.split_whitespace().find_map(|part| {
        let part = part.trim_start_matches('v');
        let mut fields = part.split('.');
        (fields.next()?.parse::<u16>().is_ok() && fields.next()?.parse::<u16>().is_ok()).then(
            || {
                part.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || ".-+".contains(*c))
                    .take(80)
                    .collect()
            },
        )
    })
}

enum App {
    #[cfg(target_os = "macos")]
    Bundle(PathBuf),
    #[cfg(windows)]
    Store(String),
    #[cfg(not(target_os = "macos"))]
    Executable(PathBuf),
}

impl App {
    async fn open(self) -> Result<(), String> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Bundle(bundle) => {
                let mut command = Command::new("/usr/bin/open");
                command.arg("-a").arg(bundle);
                checked(command, "系统无法打开此应用。").await
            }
            #[cfg(windows)]
            Self::Store(id) => {
                let mut command = Command::new("explorer.exe");
                command.arg(format!(r"shell:AppsFolder\{id}"));
                spawn_app(command, "系统无法打开此应用。")
            }
            #[cfg(not(target_os = "macos"))]
            Self::Executable(path) => spawn_app(Command::new(path), "系统无法打开此应用。"),
        }
    }
}

#[cfg(target_os = "macos")]
async fn find_app(client: Client, home: &Path) -> Option<App> {
    let names: &[&str] = match client {
        Client::Codex if env::var_os("CODEX_HOME").is_none() => &["Codex.app", "ChatGPT.app"],
        Client::Workbuddy => &["WorkBuddy.app", "WorkBuddy AI.app"],
        _ => return None,
    };
    for root in [PathBuf::from("/Applications"), home.join("Applications")] {
        for name in names {
            let bundle = root.join(name);
            let plist = bundle.join("Contents/Info.plist");
            if !plist.is_file() {
                continue;
            }
            if client == Client::Codex {
                let mut command = Command::new("/usr/bin/plutil");
                command
                    .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
                    .arg(plist);
                let found = output(command).await.ok().is_some_and(|result| {
                    result.status.success()
                        && String::from_utf8_lossy(&result.stdout).trim() == "com.openai.codex"
                });
                if !found {
                    continue;
                }
            }
            return Some(App::Bundle(bundle));
        }
    }
    None
}

#[cfg(windows)]
async fn find_app(client: Client, _home: &Path) -> Option<App> {
    match client {
        Client::Codex if env::var_os("CODEX_HOME").is_none() => {
            windows::codex_app().await.map(App::Store)
        }
        Client::Workbuddy => {
            let local = dirs::data_local_dir();
            let paths = [
                local.as_ref().map(|dir| dir.join("Programs")),
                env::var_os("ProgramFiles").map(PathBuf::from),
                env::var_os("ProgramFiles(x86)").map(PathBuf::from),
            ];
            paths
                .into_iter()
                .flatten()
                .flat_map(|dir| {
                    ["WorkBuddy", "WorkBuddy AI"]
                        .map(move |name| dir.join(name).join("WorkBuddy.exe"))
                })
                .chain(on_path("WorkBuddy.exe"))
                .find(|path| is_launchable_file(path))
                .map(App::Executable)
        }
        _ => None,
    }
}

#[cfg(target_os = "linux")]
async fn find_app(client: Client, _home: &Path) -> Option<App> {
    let (name, extra) = match client {
        Client::Workbuddy => ("workbuddy", "/opt/WorkBuddy/workbuddy"),
        _ => return None,
    };
    on_path(name)
        .chain([PathBuf::from(extra)])
        .find(|path| is_launchable_file(path))
        .map(App::Executable)
}

fn clean_claude_env(command: &mut Command) {
    for key in managed_key_names().iter().copied().chain([
        clients::CODE_MODEL_OPTION,
        clients::CODE_MODEL_OPTION_NAME,
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ]) {
        command.env_remove(key);
    }
}

pub(crate) async fn pick_project_directory(window: &tauri::WebviewWindow) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("选择项目目录")
        .set_parent(window)
        .pick_folder()
        .await
        .map(|handle| handle.path().to_path_buf())
}

pub(crate) fn project_directory(path: Option<&Path>) -> Result<&Path, String> {
    let path = path.ok_or("请选择项目目录。")?;
    if !path.is_absolute() {
        return Err("项目目录必须使用绝对路径。".into());
    }
    let metadata = fs::metadata(path).map_err(|_| "所选项目目录无法访问。")?;
    if !metadata.is_dir() {
        return Err("所选项目位置必须是目录。".into());
    }
    #[cfg(unix)]
    quote(path.as_os_str())?;
    Ok(path)
}

async fn open_terminal(
    client: Client,
    home: &Path,
    effort: Option<EffortLevel>,
    working_dir: &Path,
) -> Result<String, String> {
    let binary = find_cli(client, home)
        .ok_or_else(|| format!("未找到 {} 命令，请先安装。", client.name()))?;
    let (config_name, config_dir) = if client == Client::Code {
        let path = clients::code_path()?;
        (
            "CLAUDE_CONFIG_DIR",
            path.parent()
                .ok_or("找不到 Claude Code 的配置目录")?
                .to_path_buf(),
        )
    } else {
        ("CODEX_HOME", codex::config_home()?)
    };
    let root = secrets::root()?;
    let _lock = clients::lock_for_launch(&root, client)?;
    if client == Client::Codex {
        codex::sync_gateway(&root).await?;
    }
    if let Some(effort) = effort {
        clients::require_code_effort(&root, &config_dir.join("settings.json"), effort)?;
    }
    let profile = (client == Client::Codex)
        .then(codex::cli_launch_profile)
        .flatten();
    terminal(
        client,
        home,
        &binary,
        Some((config_name, &config_dir)),
        effort,
        working_dir,
        profile,
    )
    .await?;
    Ok(format!("已请求在终端启动 {}。", client.name()))
}

async fn open_agent_terminal(client: Client, home: &Path) -> Result<String, String> {
    let binary = find_cli(client, home)
        .ok_or_else(|| format!("未找到 {} 命令，请先安装。", client.name()))?;
    agents::ensure_compatible(client)?;
    let root = secrets::root()?;
    let _lock = clients::lock_for_launch(&root, client)?;
    terminal(client, home, &binary, None, None, home, None).await?;
    Ok(format!("已请求在终端启动 {}。", client.name()))
}

#[cfg(unix)]
fn quote(value: &OsStr) -> Result<String, String> {
    let text = value.to_str().ok_or("终端启动路径需要使用 UTF-8 编码")?;
    if text.chars().any(char::is_control) {
        return Err("终端启动路径包含控制字符。".into());
    }
    Ok(format!("'{}'", text.replace('\'', "'\\''")))
}

fn terminal_path(home: &Path, binary: &Path) -> Result<std::ffi::OsString, String> {
    let mut paths: Vec<_> = binary.parent().map(Path::to_path_buf).into_iter().collect();
    paths.push(home.join(".local/bin"));
    #[cfg(target_os = "macos")]
    paths.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    if let Some(path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&path).filter(|p| p.is_absolute()));
    }
    env::join_paths(paths).map_err(|_| "无法构造终端 PATH。".into())
}

pub(crate) fn agent_path(binary: Option<&Path>, home: &Path) -> Result<std::ffi::OsString, String> {
    let mut paths: Vec<_> = env::var_os("PATH")
        .into_iter()
        .flat_map(|path| {
            env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .collect::<Vec<_>>()
        })
        .collect();
    paths.extend(binary.and_then(Path::parent).map(Path::to_path_buf));
    paths.push(home.join(".local/bin"));
    #[cfg(target_os = "macos")]
    paths.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    env::join_paths(paths).map_err(|_| "无法构造终端 PATH。".into())
}

#[cfg(unix)]
fn shell_script(
    client: Client,
    binary: &Path,
    config_name: &str,
    config_dir: &Path,
    effort: Option<EffortLevel>,
    working_dir: &Path,
    profile: Option<&str>,
) -> Result<String, String> {
    let binary = quote(binary.as_os_str())?;
    let config_dir = quote(config_dir.as_os_str())?;
    let working_dir = quote(working_dir.as_os_str())?;
    let mut script = String::new();
    if client == Client::Code {
        script.push_str(&format!(
            "unset {} {} {} http_proxy https_proxy all_proxy no_proxy; ",
            managed_key_names().join(" "),
            clients::CODE_MODEL_OPTION,
            clients::CODE_MODEL_OPTION_NAME
        ));
    }
    #[cfg(debug_assertions)]
    if client == Client::Codex {
        if let Some(value) = env::var_os("CODENEO_LITE_DATA_DIR") {
            let value = quote(&value)?;
            script.push_str(&format!("export CODENEO_LITE_DATA_DIR={value}; "));
        }
    }
    script.push_str(&format!(
        "cd {working_dir} && export {config_name}={config_dir} && exec {binary}"
    ));
    if let Some(profile) = profile {
        if !profile.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("Codex CLI 配置名称无效".into());
        }
        script.push_str(&format!(" --profile {profile}"));
    }
    if let Some(effort) = effort {
        script.push_str(&format!(" --effort {}", effort.as_str()));
    }
    Ok(script)
}

#[cfg(unix)]
async fn terminal(
    client: Client,
    home: &Path,
    binary: &Path,
    config: Option<(&str, &Path)>,
    effort: Option<EffortLevel>,
    working_dir: &Path,
    profile: Option<&str>,
) -> Result<(), String> {
    let path = quote(&terminal_path(home, binary)?)?;
    let script = match config {
        Some((name, dir)) => shell_script(client, binary, name, dir, effort, working_dir, profile)?,
        None => agent_script(client, home, binary)?,
    };
    unix_terminal(&path, &script).await
}

#[cfg(unix)]
fn agent_script(client: Client, home: &Path, binary: &Path) -> Result<String, String> {
    let binary = quote(binary.as_os_str())?;
    let home = quote(home.as_os_str())?;
    let mut script = format!("cd {home} && ");
    for variable in agents::config_env(client) {
        if let Some(value) = env::var_os(variable).filter(|value| !value.is_empty()) {
            let value = quote(&value)?;
            script.push_str(&format!("export {variable}={value}; "));
        } else {
            script.push_str(&format!("unset {variable}; "));
        }
    }
    script.push_str(&format!("exec {binary}"));
    Ok(script)
}

#[cfg(unix)]
async fn unix_terminal(path: &str, script: &str) -> Result<(), String> {
    use claudekeeper_claude::{code_shell_argv, CodeShell};
    let script = format!("export PATH={path}\"${{PATH:+:$PATH}}\"; {script}");
    let script = format!("exec /bin/sh -c {}", quote(OsStr::new(&script))?);
    #[cfg(target_os = "macos")]
    {
        let argv = &code_shell_argv(CodeShell::Mac, &script)[0];
        let mut command = Command::new("/usr/bin/osascript");
        command.args(&argv[1..]);
        checked(
            command,
            "无法打开系统终端。请在系统设置的「隐私与安全性 → 自动化」中检查终端权限。",
        )
        .await
    }
    #[cfg(target_os = "linux")]
    {
        for argv in code_shell_argv(CodeShell::Linux, &script) {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]);
            match detached(command) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err("无法打开系统终端。".into()),
            }
        }
        Err("找不到可用的系统终端。".into())
    }
}

#[cfg(windows)]
async fn terminal(
    client: Client,
    home: &Path,
    binary: &Path,
    config: Option<(&str, &Path)>,
    effort: Option<EffortLevel>,
    working_dir: &Path,
    profile: Option<&str>,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;
    let mut command = Command::new("cmd.exe");
    command.creation_flags(CREATE_NEW_CONSOLE);
    let mut arguments = String::new();
    if let Some(profile) = profile {
        if !profile.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("Codex CLI 配置名称无效".into());
        }
        arguments.push_str(&format!(" --profile {profile}"));
    }
    if let Some(effort) = effort {
        arguments.push_str(&format!(" --effort {}", effort.as_str()));
    }
    command.raw_arg(format!(r#"/D /V:OFF /S /K ""%CODENEO_LITE_CLI%"{arguments}""#));
    if client == Client::Code {
        clean_claude_env(&mut command);
    }
    // CMD expands the path once from the environment inside quotes. Delayed
    // expansion and AutoRun are disabled, so punctuation in the path stays literal.
    command
        .env("CODENEO_LITE_CLI", binary)
        .current_dir(working_dir)
        .env("PATH", terminal_path(home, binary)?);
    if let Some((name, dir)) = config {
        command.env(name, dir);
    }
    #[cfg(debug_assertions)]
    if client == Client::Codex {
        if let Some(value) = env::var_os("CODENEO_LITE_DATA_DIR") {
            command.env("CODENEO_LITE_DATA_DIR", value);
        }
    }
    let mut child = tokio::process::Command::from(command)
        .spawn()
        .map_err(|_| "无法打开 CMD。")?;
    tauri::async_runtime::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

async fn output(command: Command) -> io::Result<Output> {
    let mut command = tokio::process::Command::from(command);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    tokio::time::timeout(Duration::from_secs(8), command.output())
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
}

async fn checked(command: Command, error: &'static str) -> Result<(), String> {
    if output(command).await.is_ok_and(|out| out.status.success()) {
        Ok(())
    } else {
        Err(error.into())
    }
}

fn detached(mut command: Command) -> io::Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = tokio::process::Command::from(command).spawn()?;
    tauri::async_runtime::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

fn spawn_app(command: Command, error: &'static str) -> Result<(), String> {
    detached(command).map_err(|_| error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_availability_distinguishes_missing_and_incompatible_clients() {
        let missing = agent_controls(Client::Qwen, false, None, Ok(()));
        assert!(!missing.installed);
        assert!(!missing.unsupported);
        assert!(!missing.terminal);
        assert!(missing.config_issue.is_none());

        let supported = agent_controls(Client::Qwen, true, Some("0.27.3".into()), Ok(()));
        assert!(supported.installed);
        assert!(!supported.unsupported);
        assert!(supported.terminal);

        let incompatible = agent_controls(Client::Opencode, true, Some("2.2.7".into()), Ok(()));
        assert!(incompatible.unsupported);
        assert!(!incompatible.terminal);

        let incompatible_config = agent_controls(
            Client::Opencode,
            false,
            None,
            Err(agents::ConfigurationIssue::Unsupported),
        );
        assert!(incompatible_config.unsupported);
        assert!(!incompatible_config.terminal);

        let unreadable = agent_controls(
            Client::Qwen,
            true,
            Some("0.27.3".into()),
            Err(agents::ConfigurationIssue::Unreadable(
                "配置无法读取".into(),
            )),
        );
        assert!(unreadable.installed);
        assert!(!unreadable.unsupported);
        assert!(!unreadable.terminal);
        assert_eq!(unreadable.config_issue.as_deref(), Some("配置无法读取"));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_uses_the_config_dir_without_inheriting_model_overrides() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("claude's binary $(exit 2)");
        let profile = temp.path().join("Profile with spaces");
        let project = temp.path().join("项目 with spaces and 'quotes'");
        fs::create_dir(&project).unwrap();
        fs::write(
            &bin,
            b"#!/bin/sh\nprintf '%s\\n%s\\n%s\\n%s\\n' \"$CLAUDE_CONFIG_DIR\" \"${ANTHROPIC_MODEL-unset}\" \"$*\" \"$PWD\"",
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        let script = shell_script(
            Client::Code,
            &bin,
            "CLAUDE_CONFIG_DIR",
            &profile,
            Some(EffortLevel::Max),
            &project,
            None,
        )
        .unwrap();
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("ANTHROPIC_MODEL", "stale")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}\nunset\n--effort max\n{}\n", profile.display(), project.display())
        );
    }

    #[test]
    fn terminal_working_directory_rejects_invalid_selections() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, "").unwrap();
        assert_eq!(project_directory(Some(temp.path())).unwrap(), temp.path());
        assert!(project_directory(None).is_err());
        assert!(project_directory(Some(Path::new(""))).is_err());
        assert!(project_directory(Some(Path::new("."))).is_err());
        assert!(project_directory(Some(&temp.path().join("missing"))).is_err());
        assert!(project_directory(Some(&file)).is_err());
    }
}
