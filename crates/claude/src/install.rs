use std::path::{Path, PathBuf};

pub const CLI_INSTALL_URL: &str = "https://claude.ai/install.sh";
pub const DESKTOP_INSTALL_URL: &str = "https://api.anthropic.com/api/desktop/darwin/universal/dmg/latest/redirect";
pub const DESKTOP_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";

pub fn is_local_proxy_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("socks5://")) else {
        return false;
    };
    let Some((host, port)) = rest.split_once(':') else { return false };
    if host != "127.0.0.1" && !host.eq_ignore_ascii_case("localhost") {
        return false;
    }
    matches!(port.parse::<u16>(), Ok(port) if port != 0)
}

pub fn effective_url_allowed(url: &str) -> bool {
    let Some(host) = https_host(url) else { return false };
    let path = url.split_once("://").and_then(|(_, rest)| rest.find('/').map(|idx| &rest[idx..])).unwrap_or("");
    if host == "storage.googleapis.com" {
        return path.to_ascii_lowercase().contains("claude");
    }
    host == "api.anthropic.com"
        || host == "downloads.claude.ai"
        || host == "claude.ai"
        || host.ends_with(".claude.ai")
        || host.ends_with(".anthropic.com")
}

fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let hostport = rest.split('/').next().unwrap_or(rest);
    let host = hostport.split('@').next_back().unwrap_or(hostport);
    let host = host.split(':').next().unwrap_or(host).trim().to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

pub fn safe_exec_name(name: &str) -> &str {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('\0') {
        "Claude"
    } else {
        name
    }
}

pub fn bundle_executable(bundle: &Path, name: &str) -> PathBuf {
    bundle.join("Contents").join("MacOS").join(safe_exec_name(name))
}

pub fn is_launchable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() || meta.len() == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return meta.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn xml_plist_string(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let rest = xml.get(xml.find(&needle)? + needle.len()..)?;
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    let value = rest[start..end].trim();
    if value.is_empty() { None } else { Some(value.to_string()) }
}

pub fn first_version_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_string()
}

pub fn user_desktop_app(home: &Path) -> PathBuf {
    home.join("Applications").join("Claude.app")
}

pub fn desktop_bundle_paths(home: &Path) -> [PathBuf; 2] {
    [PathBuf::from("/Applications/Claude.app"), user_desktop_app(home)]
}

pub fn cli_candidates(home: &Path) -> Vec<PathBuf> {
    let windows = cfg!(windows);
    let name = if windows { "claude.exe" } else { "claude" };
    let mut paths = vec![home.join(".local/bin").join(name)];
    if !windows { paths.extend([PathBuf::from("/opt/homebrew/bin/claude"), PathBuf::from("/usr/local/bin/claude"), PathBuf::from("/usr/bin/claude")]); }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if !paths.contains(&candidate) { paths.push(candidate); }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn install_urls_stay_on_official_hosts() {
        assert!(is_local_proxy_url("http://127.0.0.1:3128"));
        assert!(is_local_proxy_url("socks5://localhost:1080"));
        assert!(!is_local_proxy_url("http://127.0.0.1:3128/path"));
        assert!(!is_local_proxy_url("http://203.0.113.8:3128"));
        assert!(!is_local_proxy_url("http://127.0.0.1:0"));
        assert!(effective_url_allowed(DESKTOP_INSTALL_URL));
        assert!(effective_url_allowed(CLI_INSTALL_URL));
        assert!(effective_url_allowed("https://downloads.claude.ai/releases/darwin/universal/1/Claude.dmg"));
        assert!(effective_url_allowed("https://storage.googleapis.com/claude-desktop/Claude.dmg"));
        assert!(!effective_url_allowed("https://storage.googleapis.com/other/app.dmg"));
        assert!(!effective_url_allowed("http://downloads.claude.ai/a.dmg"));
        assert!(!effective_url_allowed("https://downloads.claude.ai.evil.com/a"));
        assert!(!effective_url_allowed("https://evil.com/claude.dmg"));
        assert_eq!(safe_exec_name("../Claude"), "Claude");
        assert_eq!(first_version_line("\n2.1.3 (Claude Code)\n"), "2.1.3 (Claude Code)");
    }

    #[test]
    fn only_an_executable_app_bundle_counts() {
        let root = std::env::temp_dir().join(format!("ck-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bundle = root.join("Claude.app");
        let exe = bundle_executable(&bundle, "Claude");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&exe).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&exe, perms).unwrap();
        }
        assert!(is_launchable_file(&exe));
        let support = root.join("Library").join("Application Support").join("Claude");
        fs::create_dir_all(&support).unwrap();
        assert!(!is_launchable_file(&support));
        fs::write(bundle.join("Contents").join("Info.plist"), r#"<?xml version="1.0"?><plist><dict><key>CFBundleIdentifier</key><string>com.anthropic.claudefordesktop</string><key>CFBundleShortVersionString</key><string>1.2.3</string></dict></plist>"#).unwrap();
        let xml = fs::read_to_string(bundle.join("Contents").join("Info.plist")).unwrap();
        assert_eq!(xml_plist_string(&xml, "CFBundleIdentifier").as_deref(), Some(DESKTOP_BUNDLE_ID));
        assert_eq!(xml_plist_string(&xml, "CFBundleShortVersionString").as_deref(), Some("1.2.3"));
        let _ = fs::remove_dir_all(root);
    }
}
