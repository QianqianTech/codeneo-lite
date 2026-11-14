use super::{checked, output};
use claudekeeper_claude::is_launchable_file;
use serde::Deserialize;
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub(super) fn powershell(script: &str) -> Command {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut command = Command::new("powershell.exe");
    command.creation_flags(CREATE_NO_WINDOW).args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $ErrorActionPreference='Stop'; {script}"),
    ]);
    command
}

pub(super) fn path_key(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if let Some(rest) = path.strip_prefix("//?/unc/") {
        return format!("//{rest}");
    }
    path.strip_prefix("//?/").unwrap_or(&path).to_owned()
}

pub(super) fn command_args(line: &str) -> Result<Vec<String>, String> {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    let line: Vec<_> = line.encode_utf16().chain([0]).collect();
    let mut count = 0;
    let args = unsafe { CommandLineToArgvW(line.as_ptr(), &mut count) };
    if args.is_null() {
        return Err("无法读取 Claude Desktop 的启动参数。".into());
    }
    // The Windows shell allocates the array and strings in a single block.
    let values = unsafe {
        let values = std::slice::from_raw_parts(args, count as usize)
            .iter()
            .map(|&ptr| {
                let length = (0..).take_while(|&offset| *ptr.add(offset) != 0).count();
                String::from_utf16_lossy(std::slice::from_raw_parts(ptr, length))
            })
            .collect();
        LocalFree(args.cast());
        values
    };
    Ok(values)
}

pub(super) async fn desktop_candidates(home: &Path) -> Vec<PathBuf> {
    let local = dirs::data_local_dir().unwrap_or_else(|| home.join("AppData/Local"));
    let legacy = local.join("AnthropicClaude");
    let mut paths = vec![
        legacy.join("Claude.exe"),
        local.join("Programs/Claude/Claude.exe"),
        local.join("Claude/Claude.exe"),
    ];
    let mut versions: Vec<_> = std::fs::read_dir(&legacy)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("app-"))
        .filter_map(|entry| {
            Some((
                entry.metadata().ok()?.modified().ok(),
                entry.path().join("Claude.exe"),
            ))
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    paths.extend(versions.into_iter().map(|(_, path)| path));
    paths.extend(store_desktop().await);
    paths
}

async fn store_desktop() -> Vec<PathBuf> {
    // MSIX package enumeration is slow on some Windows hosts. A short cache
    // avoids querying it after every configuration refresh.
    static CACHE: tokio::sync::Mutex<Option<(Instant, Vec<PathBuf>)>> =
        tokio::sync::Mutex::const_new(None);
    let mut cache = CACHE.lock().await;
    if let Some((checked, paths)) = &*cache {
        if checked.elapsed() < Duration::from_secs(if paths.is_empty() { 3 } else { 15 }) {
            return paths.clone();
        }
    }
    let command = powershell(
        r#"
        Get-AppxPackage -Name '*Claude*' |
          Where-Object { $_.Name -in @('Claude', 'Claude-Desktop', 'AnthropicPBC.Claude') } |
          ForEach-Object {
              if ($_.InstallLocation) {
                  Get-ChildItem -LiteralPath $_.InstallLocation -Filter Claude.exe -File -Recurse -ErrorAction SilentlyContinue |
                    Select-Object -ExpandProperty FullName
              }
          }
    "#,
    );
    let paths: Vec<PathBuf> = output(command)
        .await
        .ok()
        .filter(|result| result.status.success())
        .map(|result| {
            String::from_utf8_lossy(&result.stdout)
                .lines()
                .map(str::trim)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute() && is_launchable_file(path))
                .collect()
        })
        .unwrap_or_default();
    *cache = Some((Instant::now(), paths.clone()));
    paths
}

pub(super) async fn codex_app() -> Option<String> {
    let command = powershell(
        r#"
        Get-StartApps | Where-Object { $_.AppID -like 'OpenAI.Codex_2p2nqsd0c76!*' } |
          Select-Object -First 1 -ExpandProperty AppID
    "#,
    );
    let result = output(command).await.ok()?;
    let text = String::from_utf8(result.stdout).ok()?;
    let id = text.trim();
    let (_, app) = id.split_once('!')?;
    (result.status.success()
        && id.starts_with("OpenAI.Codex_2p2nqsd0c76!")
        && !app.is_empty()
        && app
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')))
    .then(|| id.to_string())
}

#[derive(Deserialize)]
pub(super) struct DesktopProcess {
    pub pid: u32,
    pub executable: Option<PathBuf>,
    pub args: Option<String>,
    pub started: u64,
}

pub(super) async fn desktop_processes(paths: &[PathBuf]) -> Result<Vec<DesktopProcess>, String> {
    let command = powershell(
        r#"
        $session = (Get-Process -Id $PID).SessionId
        $items = @(Get-CimInstance Win32_Process -Filter "Name='Claude.exe'" |
          Where-Object { $_.SessionId -eq $session -and $_.CommandLine -notmatch '(?:^|\s)--type=' } |
          ForEach-Object {
              @{
                  pid = $_.ProcessId
                  executable = $_.ExecutablePath
                  args = $_.CommandLine
                  started = $_.CreationDate.ToUniversalTime().Ticks
              }
          })
        ConvertTo-Json -InputObject $items -Compress
    "#,
    );
    let result = output(command)
        .await
        .map_err(|_| "无法读取 Claude Desktop 进程。")?;
    if !result.status.success() {
        return Err("无法读取 Claude Desktop 进程。".into());
    }
    let processes: Vec<DesktopProcess> = serde_json::from_slice(&result.stdout)
        .map_err(|_| "无法确认 Claude Desktop 的进程路径。")?;
    if processes.iter().any(|process| process.executable.is_none()) {
        return Err("无法确认同名 Claude 进程的可执行路径。".into());
    }
    let paths: Vec<_> = paths.iter().map(|path| path_key(path)).collect();
    Ok(processes
        .into_iter()
        .filter(|process| {
            process.pid > 1
                && process
                    .executable
                    .as_deref()
                    .is_some_and(|path| paths.contains(&path_key(path)))
        })
        .collect())
}

pub(super) async fn quit(pid: u32, path: &Path, started: u64) -> Result<(), String> {
    let mut command = powershell(
        r#"
        $p = Get-Process -Id ([int]$env:CODENEO_LITE_PID) -ErrorAction SilentlyContinue
        if (-not $p) { exit 0 }
        if ($p.SessionId -ne (Get-Process -Id $PID).SessionId -or
            $p.Path -ine $env:CODENEO_LITE_EXE -or
            [math]::Abs($p.StartTime.ToUniversalTime().Ticks - [long]$env:CODENEO_LITE_START) -gt 10) {
            throw 'Process identity changed'
        }
        if (-not $p.CloseMainWindow()) { throw 'The app did not accept the close request' }
    "#,
    );
    command
        .env("CODENEO_LITE_PID", pid.to_string())
        .env("CODENEO_LITE_EXE", path)
        .env("CODENEO_LITE_START", started.to_string());
    checked(
        command,
        "无法请求 Claude Desktop 退出。请从系统托盘手动退出后重试。",
    )
    .await
}
