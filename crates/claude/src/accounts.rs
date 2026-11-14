//! 本机 Claude 登录账号。只读取显示名、邮箱和是否已有会话，不返回令牌。
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAccount {
    pub id: String,
    pub name: String,
    pub email: String,
    pub detail: String,
    pub signed_in: bool,
}

/// 删除目录本身。遇到符号链接只去掉链接，不进入链接指向的位置。
pub fn remove_private_tree(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|err| format!("无法删除账号目录：{err}"))?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        fs::remove_file(path).map_err(|err| format!("无法删除账号目录：{err}"))?;
        return Ok(());
    }
    if kind.is_dir() {
        for entry in fs::read_dir(path).map_err(|err| format!("无法删除账号目录：{err}"))? {
            let entry = entry.map_err(|err| format!("无法删除账号目录：{err}"))?;
            remove_private_tree(&entry.path())?;
        }
        fs::remove_dir(path).map_err(|err| format!("无法删除账号目录：{err}"))?;
        return Ok(());
    }
    if kind.is_file() {
        fs::remove_file(path).map_err(|err| format!("无法删除账号目录：{err}"))?;
        return Ok(());
    }
    Err("账号目录里有无法识别的项目，未继续删除。".into())
}

pub fn valid_extra_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 33
        && bytes[0] == b'p'
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub fn clean_label(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars().filter(|ch| !ch.is_control()) {
        out.push(ch);
        if out.chars().count() >= 32 {
            break;
        }
    }
    let out = out.trim();
    if out.is_empty() {
        "新账号".into()
    } else {
        out.to_string()
    }
}

pub fn user_data_dir_arg(command: &str) -> Option<String> {
    let args = split_args(command);
    for (index, arg) in args.iter().enumerate() {
        let value = if let Some(value) = arg.strip_prefix("--user-data-dir=") {
            value.trim().to_string()
        } else if arg == "--user-data-dir" {
            let Some(value) = args
                .get(index + 1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty() && !value.starts_with('-'))
            else {
                continue;
            };
            value.to_string()
        } else {
            continue;
        };
        if !value.is_empty() && !value.chars().any(|ch| ch.is_control()) {
            return Some(value);
        }
    }
    None
}

pub fn parse_ps_commands(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, command) = line.split_once(char::is_whitespace)?;
            let pid = pid.parse().ok()?;
            (pid > 1).then(|| (pid, command.trim().to_string()))
        })
        .collect()
}

pub fn scan_login_accounts(
    default_dir: &Path,
    code_file: &Path,
    extra_root: &Path,
) -> Vec<StoredAccount> {
    let mut accounts = Vec::new();
    if let Some(account) = default_account(default_dir, code_file) {
        accounts.push(account);
    }
    let mut extras = extra_directories(extra_root);
    extras.sort();
    for dir in extras {
        let id = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        if !valid_extra_id(&id) {
            continue;
        }
        let session = read_object(&dir.join("config.json"))
            .map(|value| desktop_session(&value))
            .unwrap_or_default();
        accounts.push(StoredAccount {
            id,
            name: extra_label(&dir),
            email: String::new(),
            detail: if session.signed_in {
                "独立窗口 · 已保存登录".into()
            } else {
                "独立窗口 · 打开后在 Claude 里登录".into()
            },
            signed_in: session.signed_in,
        });
    }
    accounts
}

fn default_account(default_dir: &Path, code_file: &Path) -> Option<StoredAccount> {
    let desktop = read_object(&default_dir.join("config.json"))
        .map(|value| desktop_session(&value))
        .unwrap_or_default();
    let code = read_object(code_file).as_ref().and_then(code_account);
    if code.is_none() && !desktop.signed_in && desktop.account_uuid.is_none() {
        return None;
    }
    let name = code
        .as_ref()
        .map(|item| item.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "本机 Claude 账号".into());
    let email = code
        .as_ref()
        .map(|item| item.email.clone())
        .unwrap_or_default();
    let mut parts = Vec::new();
    if desktop.signed_in {
        parts.push("Desktop 已保存登录");
    } else if desktop.account_uuid.is_some() {
        parts.push("Desktop 有账号记录");
    }
    if code.is_some() {
        parts.push("Code 账号记录");
    }
    if let Some(plan) = code.as_ref().and_then(|item| item.plan.as_deref()) {
        parts.push(plan);
    }
    if let (Some(desktop_id), Some(code_id)) = (
        &desktop.account_uuid,
        code.as_ref().and_then(|item| item.account_uuid.as_ref()),
    ) {
        if desktop_id != code_id {
            parts.push("Desktop 与 Code 不是同一个账号");
        }
    }
    if parts.is_empty() {
        parts.push("本机登录记录");
    }
    Some(StoredAccount {
        id: "default".into(),
        name,
        email,
        detail: parts.join(" · "),
        signed_in: desktop.signed_in || code.is_some(),
    })
}

fn extra_label(dir: &Path) -> String {
    read_object(&dir.join("keeper.json"))
        .and_then(|value| value.get("label").and_then(Value::as_str).map(clean_label))
        .unwrap_or_else(|| "新账号".into())
}

fn extra_directories(root: &Path) -> Vec<PathBuf> {
    if !ordinary_dir(root) {
        return Vec::new();
    }
    fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (ordinary_dir(&path) && valid_extra_id(entry.file_name().to_str().unwrap_or("")))
                .then_some(path)
        })
        .collect()
}

#[derive(Default)]
struct DesktopSession {
    account_uuid: Option<String>,
    signed_in: bool,
}

fn desktop_session(value: &Value) -> DesktopSession {
    let account_uuid = value
        .get("lastKnownAccountUuid")
        .and_then(Value::as_str)
        .filter(|value| is_uuid(value))
        .map(str::to_string);
    let signed_in = ["oauth:tokenCache", "oauth:tokenCacheV2"]
        .into_iter()
        .any(|key| {
            value
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|token| !token.trim().is_empty())
        });
    DesktopSession {
        account_uuid,
        signed_in,
    }
}

struct CodeAccount {
    account_uuid: Option<String>,
    name: String,
    email: String,
    plan: Option<String>,
}

fn code_account(value: &Value) -> Option<CodeAccount> {
    let account = value.get("oauthAccount")?.as_object()?;
    if account.is_empty() {
        return None;
    }
    let text = |key: &str| {
        account
            .get(key)
            .and_then(Value::as_str)
            .map(clip)
            .filter(|text| !text.is_empty())
    };
    let name = text("displayName")
        .or_else(|| text("fullName"))
        .or_else(|| text("accountName"))
        .unwrap_or_default();
    let email = text("emailAddress")
        .or_else(|| text("email"))
        .unwrap_or_default();
    let account_uuid = text("accountUuid").filter(|value| is_uuid(value));
    if name.is_empty() && email.is_empty() && account_uuid.is_none() {
        return None;
    }
    let plan = text("organizationType")
        .as_deref()
        .and_then(plan_label)
        .map(str::to_string);
    Some(CodeAccount {
        account_uuid,
        name,
        email,
        plan,
    })
}

fn plan_label(value: &str) -> Option<&'static str> {
    match value {
        "claude_pro" | "pro" => Some("Pro"),
        "claude_max" | "max" => Some("Max"),
        "claude_team" | "team" => Some("Team"),
        "claude_enterprise" | "enterprise" => Some("Enterprise"),
        _ => None,
    }
}

fn clip(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}

fn is_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(len, part)| {
            part.len() == *len && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn ordinary_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .ok()
        .is_some_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
}

fn read_object(path: &Path) -> Option<Value> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 32 * 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?)
        .ok()
        .filter(|value: &Value| value.is_object())
}

fn split_args(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in command.chars() {
        match quote {
            Some(mark) if ch == mark => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_reads_names_without_tokens() {
        let root = tempfile::tempdir().unwrap();
        let desktop = root.path().join("Claude");
        fs::create_dir_all(&desktop).unwrap();
        fs::write(desktop.join("config.json"), r#"{"lastKnownAccountUuid":"7bcefd7f-2a52-48ae-910b-eff6a4ebf578","oauth:tokenCache":"desktop-secret","locale":"zh-CN"}"#).unwrap();
        let code = root.path().join(".claude.json");
        fs::write(&code, r#"{"oauthAccount":{"displayName":"alex","emailAddress":"a@example.invalid","accountUuid":"7bcefd7f-2a52-48ae-910b-eff6a4ebf578","organizationType":"claude_pro","token":"code-secret"}}"#).unwrap();
        let extras = root.path().join("extra");
        let id = format!("p{}", "ab".repeat(16));
        let extra = extras.join(&id);
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("keeper.json"), r#"{"label":"工作号"}"#).unwrap();
        let found = scan_login_accounts(&desktop, &code, &extras);
        let rendered = format!("{found:?}");
        assert!(!rendered.contains("desktop-secret"));
        assert!(!rendered.contains("code-secret"));
        assert_eq!(found[0].name, "alex");
        assert_eq!(found[0].email, "a@example.invalid");
        assert!(found[0].signed_in);
        assert!(found[0].detail.contains("Desktop 已保存登录"));
        assert!(found[0].detail.contains("Pro"));
        assert_eq!(found[1].id, id);
        assert_eq!(found[1].name, "工作号");
        assert!(!found[1].signed_in);
    }

    #[test]
    fn different_desktop_and_code_accounts_stay_visible() {
        let root = tempfile::tempdir().unwrap();
        let desktop = root.path().join("Claude");
        fs::create_dir_all(&desktop).unwrap();
        fs::write(
            desktop.join("config.json"),
            r#"{"lastKnownAccountUuid":"11111111-1111-1111-1111-111111111111"}"#,
        )
        .unwrap();
        let code = root.path().join(".claude.json");
        fs::write(&code, r#"{"oauthAccount":{"displayName":"ada","accountUuid":"22222222-2222-2222-2222-222222222222"}}"#).unwrap();
        let found = scan_login_accounts(&desktop, &code, &root.path().join("missing"));
        assert_eq!(found.len(), 1);
        assert!(found[0].detail.contains("不是同一个账号"));
        assert!(!found[0].detail.contains("Desktop 已保存登录"));
    }

    #[test]
    fn command_line_user_data_dir_is_parsed_without_taking_the_next_flag() {
        assert_eq!(
            user_data_dir_arg(r#"Claude --user-data-dir=/tmp/one --other"#).as_deref(),
            Some("/tmp/one")
        );
        assert_eq!(
            user_data_dir_arg(r#"Claude --user-data-dir "/tmp/my accounts""#).as_deref(),
            Some("/tmp/my accounts")
        );
        assert_eq!(
            user_data_dir_arg("Claude --user-data-dir --enable-logging"),
            None
        );
        assert_eq!(
            parse_ps_commands("  42 /Apps/Claude --user-data-dir=/tmp/a\n1 ignore\n"),
            vec![(42, "/Apps/Claude --user-data-dir=/tmp/a".into())]
        );
        assert!(valid_extra_id(&format!("p{}", "ab".repeat(16))));
        assert!(!valid_extra_id("../Claude"));
        assert_eq!(clean_label("  工作\n号  "), "工作号");
    }

    #[test]
    fn remove_private_tree_drops_nested_files() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("account");
        fs::create_dir_all(account.join("Session Storage")).unwrap();
        fs::write(account.join("Session Storage").join("keeper.json"), b"{}").unwrap();
        remove_private_tree(&account).unwrap();
        assert!(!account.exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_private_tree_unlinks_without_following() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("account");
        fs::create_dir_all(&account).unwrap();
        let outside = root.path().join("keep.txt");
        fs::write(&outside, b"stay").unwrap();
        std::os::unix::fs::symlink(&outside, account.join("SingletonLock")).unwrap();
        remove_private_tree(&account).unwrap();
        assert!(!account.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"stay");
    }
}
