use crate::{conflict_keys, read_env, HostOs, Layout, Profile};
use std::path::{Path, PathBuf};

pub const DESKTOP_DOWNLOAD: &str = "https://claude.com/download";

pub fn desktop_download_url() -> &'static str {
    DESKTOP_DOWNLOAD
}

pub fn optional_help(key: &str) -> &'static str {
    match key {
        "API_TIMEOUT_MS" => "请求超时毫秒。留空则不写入。",
        "CLAUDE_CODE_MAX_RETRIES" => "失败后的重试次数。留空则不写入。",
        "DISABLE_TELEMETRY" => "设为 1 时关闭 Claude Code 遥测。默认不写。",
        "DISABLE_AUTOUPDATER" => "设为 1 时关闭 Claude Code 自己的更新。默认不写。",
        "DISABLE_ERROR_REPORTING" => "设为 1 时关闭错误上报。默认不写。",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" => "设为 1 时减少非必要网络请求。默认不写。",
        "language" => "界面语言写在 settings.json 根上，不放进 env。",
        _ => "可选变量。默认不写入。",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterSubject {
    ThisApp,
    Claude,
}

pub fn updater_may_touch(subject: UpdaterSubject, claude_updates_enabled: bool) -> bool {
    match subject {
        UpdaterSubject::ThisApp => true,
        UpdaterSubject::Claude => claude_updates_enabled,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFacts {
    pub cli_exists: bool,
    pub version: Option<String>,
    pub desktop_exists: bool,
    pub desktop_reason: String,
    pub base_url: String,
    pub model: String,
    pub proxy: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvOverview {
    pub text: String,
    pub install_offered: bool,
}

pub fn summarize_env(facts: &EnvFacts) -> EnvOverview {
    let cli = if facts.cli_exists {
        facts.version.clone().unwrap_or_else(|| "软件端已安装，没有读到版本".into())
    } else {
        "软件端未安装".into()
    };
    let desktop = if facts.desktop_exists { "桌面版可以启动" } else { "桌面版未安装" };
    let text = format!(
        "{cli}\n{desktop}\n实际目录: {}\nBase URL: {}\n模型: {}\n代理: {}\n设置来源: {}",
        facts.desktop_reason,
        blank(&facts.base_url),
        blank(&facts.model),
        blank(&facts.proxy),
        facts.source
    );
    EnvOverview { text, install_offered: !facts.cli_exists }
}

fn blank(value: &str) -> &str {
    if value.trim().is_empty() { "未设置" } else { value }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    Terminal,
    Desktop,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub kind: LaunchKind,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub message: String,
}

pub fn proxy_process_env(http_port: u16, socks_port: u16) -> Vec<(String, String)> {
    vec![
        ("HTTP_PROXY".into(), format!("http://127.0.0.1:{http_port}")),
        ("HTTPS_PROXY".into(), format!("http://127.0.0.1:{http_port}")),
        ("ALL_PROXY".into(), format!("socks5://127.0.0.1:{socks_port}")),
        ("NO_PROXY".into(), "localhost,127.0.0.1,::1".into()),
    ]
}

/// 仅识别第三方模型档案；启动门槛对官方和第三方模型一视同仁。
pub fn custom_model_launch(active_profile: &str) -> bool {
    let id = active_profile.trim();
    !id.is_empty() && id != "official"
}

pub fn launch_plan(os: HostOs, open_desktop: bool, desktop_running: bool, cli_exists: bool, http_port: u16, socks_port: u16, use_proxy: bool) -> LaunchPlan {
    if open_desktop && desktop_running {
        return LaunchPlan {
            kind: LaunchKind::Blocked,
            argv: Vec::new(),
            env: Vec::new(),
            message: "Claude Desktop 已在运行。请先退出再启动，本软件不会结束它。".into(),
        };
    }
    let env = if use_proxy { proxy_process_env(http_port, socks_port) } else { Vec::new() };
    if open_desktop {
        let argv = match os {
            HostOs::Mac => vec!["open".into(), "-a".into(), "Claude".into()],
            HostOs::Windows => vec!["cmd".into(), "/C".into(), "start".into(), String::new(), "Claude.exe".into()],
            HostOs::Linux => vec!["claude-desktop".into()],
        };
        let message = if use_proxy {
            "将启动 Claude Desktop。代理以 settings.json 里的 HTTP_PROXY 为准，不修改资源管理器或其他已运行进程。"
        } else {
            "将启动 Claude Desktop。当前是直连，不给它写代理。"
        };
        return LaunchPlan {
            kind: LaunchKind::Desktop,
            argv,
            env,
            message: message.into(),
        };
    }
    if !cli_exists {
        return LaunchPlan {
            kind: LaunchKind::Blocked,
            argv: Vec::new(),
            env: Vec::new(),
            message: "没有找到 claude，先安装 Claude Code。".into(),
        };
    }
    let argv = match os {
        HostOs::Mac | HostOs::Linux => vec!["claude".into()],
        HostOs::Windows => vec!["claude.exe".into()],
    };
    let message = if use_proxy {
        "将打开终端，并且只给这个新进程加上本机代理环境。"
    } else {
        "将打开终端。当前是直连，这个新进程不走代理。"
    };
    LaunchPlan {
        kind: LaunchKind::Terminal,
        argv,
        env,
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownFile {
    pub label: &'static str,
    pub path: PathBuf,
    pub exists: bool,
}

pub fn shown_files(layout: &Layout, project: Option<&Path>) -> Vec<ShownFile> {
    let mut out = vec![
        file("用户设置", layout.user_settings()),
        file("全局状态", layout.global_state()),
        file("用户 MCP 所在的全局文件", layout.global_state()),
    ];
    if let Some(project) = project {
        out.push(file("项目设置", layout.project_settings(project)));
        out.push(file("项目本地设置", layout.project_local_settings(project)));
        out.push(file("项目 MCP", layout.project_mcp(project)));
        out.push(file("项目 CLAUDE.md", layout.project_claude_md(project)));
    }
    out
}

fn file(label: &'static str, path: PathBuf) -> ShownFile {
    let exists = path.exists();
    ShownFile { label, path, exists }
}

pub fn render_shown(files: &[ShownFile]) -> String {
    files
        .iter()
        .map(|item| format!("{} {} {}", item.label, if item.exists { "存在" } else { "不存在" }, item.path.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn project_conflicts(layout: &Layout, project: &Path, profile: &Profile) -> Result<Vec<String>, crate::SettingsError> {
    let user = read_env(&layout.user_settings())?;
    let local = read_env(&layout.project_local_settings(project))?;
    Ok(conflict_keys(&user, &local, profile))
}

pub fn switch_confirm_text() -> String {
    format!("将写入用户级 settings.json，并从 env 删除这些托管键：{}。取消则不会写入。", crate::managed_key_names().join("、"))
}

pub fn switch_notice(settings_path: &str, managed: bool, restored_login: Option<&str>) -> String {
    let mut text = format!("已写入 {settings_path}。已经打开的 Claude Code 不会重新读取 settings.json。请新开终端再运行 claude。本软件不会结束已有进程。桌面 Cowork 需要再点「应用到 Cowork」。");
    if managed {
        text.push_str("用户设置可能被托管策略覆盖，不能当成已经生效。");
    }
    if let Some(login) = restored_login {
        text.push(' ');
        text.push_str(login);
        text.push_str(" 不调用 /logout，不删除 Claude Code-credentials。未登录时请自己在 Claude Code 里登录。");
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeShell {
    Mac,
    Windows,
    Linux,
}

pub fn code_shell_label(shell: CodeShell) -> &'static str {
    match shell {
        CodeShell::Mac | CodeShell::Linux => "终端",
        CodeShell::Windows => "CMD",
    }
}

/// 在对应系统的终端里运行 `claude`，并带上这次核对过的代理。
/// Mac 用「终端」，Windows 用 CMD，Ubuntu 等 Linux 用系统的「终端」。
pub fn claude_code_script(shell: CodeShell, http_proxy: &str, socks_port: u16) -> String {
    let socks = format!("socks5://127.0.0.1:{socks_port}");
    let keys = crate::managed_key_names();
    match shell {
        CodeShell::Windows => format!(
            "{}set HTTP_PROXY={http_proxy}&& set HTTPS_PROXY={http_proxy}&& set ALL_PROXY={socks}&& set NO_PROXY=localhost,127.0.0.1,::1&& claude",
            keys.iter().map(|key| format!("set \"{key}=\"&& ")).collect::<String>()
        ),
        CodeShell::Mac | CodeShell::Linux => format!(
            "unset {} http_proxy https_proxy all_proxy no_proxy; export HTTP_PROXY={}; export HTTPS_PROXY={}; export ALL_PROXY={}; export NO_PROXY='localhost,127.0.0.1,::1'; exec claude",
            keys.join(" "),
            posix_quote(http_proxy),
            posix_quote(http_proxy),
            posix_quote(&socks)
        ),
    }
}

pub fn claude_code_tun_script(shell: CodeShell) -> String {
    let keys = crate::managed_key_names();
    match shell {
        CodeShell::Windows => format!("{}claude", keys.iter().map(|key| format!("set \"{key}=\"&& ")).collect::<String>()),
        CodeShell::Mac | CodeShell::Linux => format!("unset {} http_proxy https_proxy all_proxy no_proxy; exec claude", keys.join(" ")),
    }
}

pub fn code_shell_argv(shell: CodeShell, script: &str) -> Vec<Vec<String>> {
    match shell {
        CodeShell::Mac => vec![vec![
            "osascript".into(),
            "-e".into(),
            format!("tell application \"Terminal\" to do script \"{}\"", applescript_escape(script)),
        ]],
        CodeShell::Windows => vec![vec![
            "cmd".into(),
            "/C".into(),
            "start".into(),
            "Claude Code".into(),
            "cmd".into(),
            "/K".into(),
            script.into(),
        ]],
        CodeShell::Linux => vec![
            vec!["x-terminal-emulator".into(), "-e".into(), "bash".into(), "-lc".into(), script.into()],
            vec!["ptyxis".into(), "--".into(), "bash".into(), "-lc".into(), script.into()],
            vec!["gnome-terminal".into(), "--".into(), "bash".into(), "-lc".into(), script.into()],
            vec!["kgx".into(), "--".into(), "bash".into(), "-lc".into(), script.into()],
            vec!["xterm".into(), "-e".into(), "bash".into(), "-lc".into(), script.into()],
        ],
    }
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn applescript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn doctor_result(cli_exists: bool, output: Option<&str>) -> String {
    if !cli_exists {
        return "没有 claude 可执行文件，下面不是官方诊断".into();
    }
    match output {
        Some(text) if !text.trim().is_empty() => text.trim().to_string(),
        _ => "claude doctor 没有输出".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthMode, Profile};
    use std::fs;

    #[test]
    fn custom_model_profile_classification() {
        assert!(!custom_model_launch("official"));
        assert!(!custom_model_launch(""));
        assert!(!custom_model_launch("  official  "));
        assert!(custom_model_launch("kimi"));
        assert!(custom_model_launch("deepseek"));
        assert!(custom_model_launch("p12"));
    }

    #[test]
    fn launch_blocks_a_running_desktop_and_limits_env() {
        let blocked = launch_plan(HostOs::Mac, true, true, true, 3128, 1080, true);
        assert_eq!(blocked.kind, LaunchKind::Blocked);
        assert!(blocked.argv.is_empty());
        assert!(blocked.message.contains("不会结束"));
        let term = launch_plan(HostOs::Mac, false, false, true, 3128, 1088, true);
        assert_eq!(term.kind, LaunchKind::Terminal);
        assert_eq!(term.env.len(), 4);
        assert!(term.env.iter().any(|(k, v)| k == "ALL_PROXY" && v.ends_with(":1088")));
        assert!(term.env.iter().all(|(k, _)| !k.starts_with("ANTHROPIC_")));
        let joined = format!("{:?} {}", term.argv, term.message);
        assert!(!joined.to_ascii_lowercase().contains("kill"));
        assert!(!joined.contains("taskkill"));
        let desktop = launch_plan(HostOs::Windows, true, false, false, 3128, 1080, true);
        let direct = launch_plan(HostOs::Mac, false, false, true, 3128, 1080, false);
        assert!(direct.env.is_empty());
        assert!(direct.message.contains("直连"));
        assert_eq!(desktop.kind, LaunchKind::Desktop);
        let script = claude_code_script(CodeShell::Mac, "http://127.0.0.1:7897", 7897);
        assert!(script.contains("exec claude"));
        // Inherited model variables must be cleared, but never assigned by the launcher.
        let (cleanup, exports) = script.split_once("; export ").unwrap();
        assert!(cleanup.starts_with("unset "));
        assert!(cleanup.contains("ANTHROPIC_API_KEY"));
        assert!(!exports.contains("ANTHROPIC_"));
        let mac = code_shell_argv(CodeShell::Mac, &script);
        assert_eq!(mac[0][0], "osascript");
        assert!(mac[0][2].contains("Terminal"));
        assert!(!mac[0][2].contains("cmd"));
        let win = code_shell_argv(CodeShell::Windows, &claude_code_script(CodeShell::Windows, "http://127.0.0.1:7897", 7897));
        assert_eq!(win.len(), 1);
        assert_eq!(win[0][0], "cmd");
        assert!(win[0].iter().any(|part| part == "/K"));
        assert!(win[0].last().unwrap().contains("claude"));
        let linux = code_shell_argv(CodeShell::Linux, &script);
        assert_eq!(linux[0][0], "x-terminal-emulator");
        assert!(linux.iter().any(|argv| argv[0] == "gnome-terminal"));
        assert_eq!(code_shell_label(CodeShell::Windows), "CMD");
        assert_eq!(code_shell_label(CodeShell::Linux), "终端");
        assert_eq!(code_shell_label(CodeShell::Mac), "终端");
        assert!(!updater_may_touch(UpdaterSubject::Claude, false));
        assert!(updater_may_touch(UpdaterSubject::ThisApp, false));
    }

    #[test]
    fn env_summary_offers_install_and_lists_files_without_contents() {
        let view = summarize_env(&EnvFacts {
            cli_exists: false,
            version: None,
            desktop_exists: false,
            desktop_reason: "使用桌面版用户数据目录".into(),
            base_url: String::new(),
            model: String::new(),
            proxy: String::new(),
            source: "用户设置".into(),
        });
        assert!(view.install_offered);
        assert!(view.text.contains("软件端未安装"));
        assert!(view.text.contains("桌面版未安装"));
        assert!(view.text.contains("使用桌面版用户数据目录"));
        let root = std::env::temp_dir().join(format!("ck-ops-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project = root.join("proj");
        fs::create_dir_all(project.join(".claude")).unwrap();
        fs::write(project.join("CLAUDE.md"), "secret-body-should-not-appear").unwrap();
        let layout = Layout {
            os: HostOs::Mac,
            home: root.join("home"),
            appdata: root.join("app"),
            local_appdata: root.join("local"),
            program_data: root.join("pd"),
        };
        let text = render_shown(&shown_files(&layout, Some(&project)));
        assert!(text.contains("CLAUDE.md"));
        assert!(text.contains("存在"));
        assert!(!text.contains("secret-body"));
        assert_eq!(doctor_result(false, Some("token")), "没有 claude 可执行文件，下面不是官方诊断");
        assert!(!doctor_result(false, Some("token")).contains("token"));
        assert_eq!(optional_help("language"), "界面语言写在 settings.json 根上，不放进 env。");
        let _ = AuthMode::Official;
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_conflict_is_read_only() {
        let root = std::env::temp_dir().join(format!("ck-ops-conflict-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("proj");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(project.join(".claude")).unwrap();
        fs::write(home.join(".claude").join("settings.json"), r#"{"env":{"HTTP_PROXY":"http://127.0.0.1:3128"}}"#).unwrap();
        fs::write(project.join(".claude").join("settings.local.json"), r#"{"env":{"HTTP_PROXY":"http://127.0.0.1:7890","ANTHROPIC_BASE_URL":"https://other.example"}}"#).unwrap();
        let layout = Layout { os: HostOs::Mac, home, appdata: root.clone(), local_appdata: root.clone(), program_data: root.clone() };
        let mut profile = Profile::official();
        profile.base_url = "https://gateway.example".into();
        let keys = project_conflicts(&layout, &project, &profile).unwrap();
        assert!(keys.contains(&"HTTP_PROXY".to_string()));
        assert!(keys.contains(&"ANTHROPIC_BASE_URL".to_string()));
        let _ = fs::remove_dir_all(root);
    }
}
