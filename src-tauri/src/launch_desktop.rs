#[cfg(unix)]
use super::output;
use super::{clean_claude_env, spawn_app, Action, Controls};
use crate::{clients, desktop, desktop_models, gateway::Connection, secrets, transaction::Client};
use claudekeeper_claude::is_launchable_file;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const INSTALL: &str = "未找到可启动的 Claude Desktop。安装后刷新状态。";
const MANUAL: &str =
    "无法确认当前 Claude Desktop 窗口的资料目录。请手动退出对应窗口，再从这里打开。";
const START_FAILED: &str = "无法启动 Claude Desktop，请检查安装。";

#[derive(Debug, PartialEq, Eq)]
struct Instance {
    pid: u32,
    executable: PathBuf,
    data_dir: Option<PathBuf>,
    started: u64,
}

struct Target {
    binary: PathBuf,
    data_dir: Option<PathBuf>,
    running: Option<Instance>,
}

#[cfg(target_os = "macos")]
async fn candidates(home: &Path) -> Vec<PathBuf> {
    claudekeeper_claude::desktop_bundle_paths(home)
        .map(|bundle| bundle.join("Contents/MacOS/Claude"))
        .into()
}

#[cfg(target_os = "linux")]
async fn candidates(_home: &Path) -> Vec<PathBuf> {
    super::on_path("claude-desktop")
        .chain(["/usr/bin/claude-desktop", "/opt/Claude/claude-desktop"].map(PathBuf::from))
        .collect()
}

#[cfg(windows)]
async fn candidates(home: &Path) -> Vec<PathBuf> {
    super::windows::desktop_candidates(home).await
}

pub(super) fn is_desktop_binary(path: &Path) -> bool {
    let exe = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "macos")]
    {
        exe.ends_with("Contents/MacOS/Claude")
    }
    #[cfg(target_os = "linux")]
    {
        exe.file_name().is_some_and(|name| name == "claude-desktop")
    }
    #[cfg(windows)]
    {
        let path = super::windows::path_key(&exe);
        path.contains("/anthropicclaude/")
            || path.contains("/programs/claude/")
            || path.contains("/appdata/local/claude/")
            || path.contains("/windowsapps/claude_")
            || path.contains("/windowsapps/claude-desktop_")
            || path.contains("/windowsapps/anthropicpbc.claude_")
    }
}

pub(super) async fn controls(home: &Path) -> Controls {
    let paths = installed(home).await;
    if paths.is_empty() {
        return Controls::issue(INSTALL);
    }
    match inspect(&paths).await {
        Ok(target) => Controls {
            open: true,
            restart: target.running.is_some(),
            installed: true,
            ..Controls::default()
        },
        Err(error) => Controls {
            note: Some(error),
            installed: true,
            ..Controls::default()
        },
    }
}

async fn installed(home: &Path) -> Vec<PathBuf> {
    candidates(home)
        .await
        .into_iter()
        .filter(|path| is_launchable_file(path))
        .collect()
}

async fn inspect(paths: &[PathBuf]) -> Result<Target, String> {
    let data_dir = desktop::custom_dir()?;
    let instances = instances(paths).await?;
    let default_dirs = if data_dir.is_none() {
        desktop::locations()
            .map(|locations| vec![locations.native, locations.gateway])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let running = select_instance(instances, data_dir.as_deref(), &default_dirs)?;
    let binary = running
        .as_ref()
        .map(|instance| instance.executable.clone())
        .or_else(|| paths.first().cloned())
        .ok_or(INSTALL)?;
    Ok(Target {
        binary,
        data_dir,
        running,
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    #[cfg(windows)]
    return super::windows::path_key(&left) == super::windows::path_key(&right);
    #[cfg(not(windows))]
    return left == right;
}

fn select_instance(
    instances: Vec<Instance>,
    custom_dir: Option<&Path>,
    default_dirs: &[PathBuf],
) -> Result<Option<Instance>, String> {
    let mut selected = None;
    for instance in instances {
        let matches = match (custom_dir, instance.data_dir.as_deref()) {
            (None, None) => true,
            (None, Some(dir)) => default_dirs.iter().any(|default| same_path(default, dir)),
            (Some(expected), Some(dir)) => same_path(expected, dir),
            (Some(_), None) => false,
        };
        if matches {
            if selected.is_some() {
                return Err(MANUAL.into());
            }
            selected = Some(instance);
        }
    }
    Ok(selected)
}

pub(super) async fn run(
    home: &Path,
    action: Option<Action>,
    apply: Option<(Option<&str>, Option<Connection>)>,
) -> Result<String, String> {
    let paths = installed(home).await;
    if paths.is_empty() {
        return Err(INSTALL.into());
    }
    let root = secrets::root()?;
    let _lock = clients::lock_for_launch(&root, Client::Desktop)?;
    let mut target = inspect(&paths).await?;
    let change = match apply {
        Some((id, connection)) => {
            Some(clients::DesktopChange::prepare(&root, id, connection).await?)
        }
        None => None,
    };
    let hot = action != Some(Action::Restart)
        && change.as_ref().is_some_and(|change| !change.reload)
        && target.running.is_some();
    if change.is_some() && !hot && action != Some(Action::Restart) && target.running.is_some() {
        return Err("Claude Desktop 正在运行。请选择「应用并重启」，或完全退出后再应用。".into());
    }
    if change.is_none() {
        desktop_models::sync(&root).await?;
    }
    let applying = change.is_some();
    let was_running = target.running.is_some();
    match (action, target.running.as_ref()) {
        (Some(Action::Open), Some(instance)) if !applying => {
            activate(instance).await?;
            return Ok("已打开正在运行的 Claude Desktop。".into());
        }
        (Some(Action::Restart), Some(instance)) if !hot => {
            // Keep the same explicit profile when ClaudeKeeper started this
            // instance with --user-data-dir for the default directory.
            target.data_dir = target.data_dir.or_else(|| instance.data_dir.clone());
            quit(&paths, instance).await?;
            tokio::time::timeout(Duration::from_secs(8), async {
                while inspect(&paths).await?.running.is_some() {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Ok::<_, String>(())
            })
            .await
            .map_err(|_| "Claude Desktop 尚未退出。请在客户端完成退出后再打开。")??;
        }
        _ => {}
    }
    if let Some(change) = change {
        if let Err(error) = change.commit(&root) {
            if was_running
                && !hot
                && action == Some(Action::Restart)
                && spawn_app(
                    command(&target.binary, target.data_dir.as_deref()),
                    START_FAILED,
                )
                .is_err()
            {
                return Err(format!("{error}。Claude Desktop 需要手动重新打开。"));
            }
            return Err(error);
        }
        desktop_models::sync(&root)
            .await
            .map_err(|error| format!("模式配置已写入。{error}"))?;
        if hot {
            return Ok("Claude Desktop 网关已切换模型。原会话的下一条请求使用新档案。".into());
        }
        if action.is_none() {
            return Ok("Claude Desktop 配置已写入，下次启动时生效。".into());
        }
    }
    let result = start(&target, &paths).await;
    let message = if action == Some(Action::Restart) && was_running {
        "Claude Desktop 已重启。"
    } else {
        "Claude Desktop 已启动。"
    };
    if applying {
        return result
            .map(|_| format!("配置已应用。{message}"))
            .map_err(|error| format!("配置已应用。{error}"));
    }
    result.map(|_| message.into())
}

async fn start(target: &Target, paths: &[PathBuf]) -> Result<(), String> {
    spawn_app(
        command(&target.binary, target.data_dir.as_deref()),
        START_FAILED,
    )?;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if inspect(paths).await?.running.is_some() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
    .await
    .map_err(|_| "未检测到当前 Claude Desktop 窗口启动。请检查客户端并刷新状态。".to_string())?
}

fn command(binary: &Path, data_dir: Option<&Path>) -> Command {
    let mut command = Command::new(binary);
    clean_claude_env(&mut command);
    if let Some(dir) = data_dir {
        let mut argument = OsString::from("--user-data-dir=");
        argument.push(dir);
        command.arg(argument);
    }
    command
}

#[cfg(target_os = "macos")]
async fn activate(instance: &Instance) -> Result<(), String> {
    if instance.data_dir.is_some() {
        return spawn_app(
            command(&instance.executable, instance.data_dir.as_deref()),
            "无法切换到 Claude Desktop。",
        );
    }
    let bundle = instance.executable.ancestors().nth(3).ok_or(MANUAL)?;
    let mut command = Command::new("/usr/bin/open");
    command.arg("-a").arg(bundle);
    super::checked(command, "无法切换到 Claude Desktop。").await
}

#[cfg(windows)]
async fn activate(instance: &Instance) -> Result<(), String> {
    spawn_app(
        command(&instance.executable, instance.data_dir.as_deref()),
        "无法切换到 Claude Desktop。",
    )
}

#[cfg(target_os = "linux")]
async fn activate(_instance: &Instance) -> Result<(), String> {
    Err("Claude Desktop 已在运行。请从系统任务栏切换到该窗口。".into())
}

#[cfg(unix)]
async fn quit(paths: &[PathBuf], expected: &Instance) -> Result<(), String> {
    let live = instances(paths).await?;
    let Some(instance) = live.iter().find(|instance| instance.pid == expected.pid) else {
        return Ok(());
    };
    if instance != expected {
        return Err(MANUAL.into());
    }
    if unsafe { libc::kill(expected.pid as libc::pid_t, libc::SIGTERM) } != 0
        && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    {
        return Err("无法请求 Claude Desktop 退出。请手动退出后重试。".into());
    }
    Ok(())
}

#[cfg(windows)]
async fn quit(_paths: &[PathBuf], expected: &Instance) -> Result<(), String> {
    super::windows::quit(expected.pid, &expected.executable, expected.started).await
}

#[cfg(unix)]
async fn instances(paths: &[PathBuf]) -> Result<Vec<Instance>, String> {
    let user = unsafe { libc::geteuid() }.to_string();
    let mut command = Command::new("/bin/ps");
    command.args(["-ww", "-u", &user, "-o", "pid=,comm="]);
    let result = output(command).await.map_err(|_| MANUAL)?;
    if !result.status.success() {
        return Err(MANUAL.into());
    }
    let mut found = Vec::new();
    for (pid, name) in
        claudekeeper_claude::parse_ps_commands(&String::from_utf8_lossy(&result.stdout))
    {
        #[cfg(target_os = "macos")]
        let executable = PathBuf::from(name);
        #[cfg(target_os = "linux")]
        let executable = {
            if !paths
                .iter()
                .any(|path| path.file_name().is_some_and(|file| file == name.as_str()))
            {
                continue;
            }
            match std::fs::read_link(format!("/proc/{pid}/exe")) {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(MANUAL.into()),
            }
        };
        if !paths.iter().any(|path| same_path(path, &executable)) {
            continue;
        }
        let Some(started) = process_start(pid)? else {
            continue;
        };
        let Some((args, profile_env)) = process_details(pid)? else {
            continue;
        };
        if args.iter().any(|arg| arg.starts_with("--type=")) {
            continue;
        }
        found.push(Instance {
            pid,
            executable,
            data_dir: profile_env.or(data_arg(&args)?),
            started,
        });
    }
    Ok(found)
}

#[cfg(windows)]
async fn instances(paths: &[PathBuf]) -> Result<Vec<Instance>, String> {
    super::windows::desktop_processes(paths)
        .await?
        .into_iter()
        .map(|process| {
            let args = process.args.ok_or(MANUAL)?;
            let data_dir = data_arg(&super::windows::command_args(&args)?)?;
            Ok(Instance {
                pid: process.pid,
                executable: process.executable.ok_or(MANUAL)?,
                data_dir,
                started: process.started,
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn process_start(pid: u32) -> Result<Option<u64>, String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(MANUAL.into());
    }
    let info = unsafe { info.assume_init() };
    Ok(
        (info.pbi_status != libc::SZOMB && info.pbi_uid == unsafe { libc::geteuid() })
            .then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec),
    )
}

#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> Result<Option<u64>, String> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(MANUAL.into()),
    };
    // The command name may contain spaces and parentheses. Fields after the
    // last parenthesis start with the state (field 3) and include starttime (22).
    let fields = stat.rsplit_once(')').ok_or(MANUAL)?.1.trim_start();
    if fields.starts_with("Z ") {
        return Ok(None);
    }
    let started = fields
        .split_ascii_whitespace()
        .nth(19)
        .ok_or(MANUAL)?
        .parse()
        .map_err(|_| MANUAL)?;
    Ok(Some(started))
}

fn data_arg(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut result = None;
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        let value = if arg == "--user-data-dir" {
            args.next().ok_or(MANUAL)?.as_str()
        } else if let Some(value) = arg.strip_prefix("--user-data-dir=") {
            value
        } else {
            continue;
        };
        let dir = PathBuf::from(value);
        if value.chars().any(char::is_control)
            || !dir.is_absolute()
            || result.replace(dir).is_some()
        {
            return Err(MANUAL.into());
        }
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
fn process_details(pid: u32) -> Result<Option<(Vec<String>, Option<PathBuf>)>, String> {
    let bytes = match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(MANUAL.into()),
    };
    let args = String::from_utf8(bytes)
        .map_err(|_| MANUAL)?
        .split_terminator('\0')
        .map(str::to_owned)
        .collect();
    let env = match std::fs::read(format!("/proc/{pid}/environ")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(MANUAL.into()),
    };
    Ok(Some((args, profile_env(env.split(|&byte| byte == 0))?)))
}

#[cfg(unix)]
fn profile_env<'a>(env: impl Iterator<Item = &'a [u8]>) -> Result<Option<PathBuf>, String> {
    let Some(value) = env
        .filter_map(|item| item.strip_prefix(b"CLAUDE_USER_DATA_DIR="))
        .next()
    else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(std::str::from_utf8(value).map_err(|_| MANUAL)?);
    if !path.is_absolute() {
        return Err(MANUAL.into());
    }
    Ok(Some(path))
}

#[cfg(target_os = "macos")]
fn process_details(pid: u32) -> Result<Option<(Vec<String>, Option<PathBuf>)>, String> {
    // ps flattens argv and loses spaces within --user-data-dir. Read only the
    // matched process and retain its arguments and the one profile override.
    let mut maximum = 0i32;
    let mut length = std::mem::size_of_val(&maximum);
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut maximum as *mut i32).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || !(4096..=8 * 1024 * 1024).contains(&maximum) {
        return Err(MANUAL.into());
    }
    let mut bytes = vec![0; maximum as usize];
    let mut length = bytes.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        // A process can disappear after ps reports its executable. XNU
        // sometimes returns EINVAL for its argv once teardown begins.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(None);
        }
        return Err(MANUAL.into());
    }
    parse_macos_details(&bytes[..length]).map(Some)
}

#[cfg(target_os = "macos")]
fn parse_macos_details(bytes: &[u8]) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let (count, body) = bytes.split_first_chunk::<4>().ok_or(MANUAL)?;
    let count = usize::try_from(i32::from_ne_bytes(*count)).map_err(|_| MANUAL)?;
    let end_path = body.iter().position(|&byte| byte == 0).ok_or(MANUAL)?;
    let body = &body[end_path + 1..];
    let start = body.iter().position(|&byte| byte != 0).ok_or(MANUAL)?;
    let mut fields = body[start..].split(|&byte| byte == 0);
    let args = fields
        .by_ref()
        .take(count)
        .map(|arg| String::from_utf8(arg.to_vec()).map_err(|_| MANUAL.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if count == 0 || args.len() != count {
        return Err(MANUAL.into());
    }
    Ok((args, profile_env(fields)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(pid: u32, dir: Option<&Path>) -> Instance {
        Instance {
            pid,
            executable: PathBuf::from("Claude"),
            data_dir: dir.map(Path::to_path_buf),
            started: 0,
        }
    }

    #[test]
    fn restart_selects_only_the_managed_profile() {
        let temp = tempfile::tempdir().unwrap();
        let default = temp.path().join("Claude");
        let custom = temp.path().join("Work Account");
        let instances = || vec![instance(100, None), instance(101, Some(&custom))];
        assert_eq!(
            select_instance(instances(), None, std::slice::from_ref(&default))
                .unwrap()
                .unwrap()
                .pid,
            100
        );
        assert_eq!(
            select_instance(instances(), Some(&custom), &[])
                .unwrap()
                .unwrap()
                .pid,
            101
        );
        assert!(
            select_instance(vec![instance(101, Some(&custom))], None, &[])
                .unwrap()
                .is_none()
        );
        assert!(select_instance(
            vec![instance(100, None), instance(102, Some(&default))],
            None,
            &[default]
        )
        .is_err());
    }

    #[test]
    fn desktop_launch_clears_inherited_model_env_and_keeps_the_profile_argument() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("My Profile");
        let binary = temp.path().join("Claude");
        let command = command(&binary, Some(&dir));
        let mut expected = OsString::from("--user-data-dir=");
        expected.push(&dir);
        assert_eq!(command.get_program(), binary.as_os_str());
        assert_eq!(command.get_args().collect::<Vec<_>>(), [&expected]);
        for key in [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "HTTPS_PROXY",
            "https_proxy",
        ] {
            assert!(command
                .get_envs()
                .any(|(name, value)| name == key && value.is_none()));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_argv_keeps_spaces_and_distinguishes_environment_overrides() {
        let mut bytes = 2i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(b"/Applications/Claude.app/Contents/MacOS/Claude\0\0Claude\0--user-data-dir=/tmp/Default Data\0CLAUDE_USER_DATA_DIR=/tmp/Work Data\0");
        let (args, profile) = parse_macos_details(&bytes).unwrap();
        assert_eq!(
            data_arg(&args).unwrap().as_deref(),
            Some(Path::new("/tmp/Default Data"))
        );
        assert_eq!(profile.as_deref(), Some(Path::new("/tmp/Work Data")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quit_targets_only_the_verified_executable() {
        use std::{fs, os::unix::fs::PermissionsExt};
        if Command::new("/bin/ps").args(["-p", "1"]).output().is_err() {
            return; // The restricted development sandbox can deny ps entirely.
        }
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("Desktop fixture");
        // Use a native executable so the OS reports the same path used for
        // identity checks. Its only activity is blocking on a private pipe.
        fs::copy("/bin/cat", &binary).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let paths = [binary.clone()];
        let mut first = tokio::process::Command::new(&binary)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut second = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let live = instances(&paths).await.unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(Some(live[0].pid), first.id());
        quit(&paths, &live[0]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), first.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(instances(&paths).await.unwrap().is_empty());
        assert!(second.try_wait().unwrap().is_none());
        second.kill().await.unwrap();
    }
}
