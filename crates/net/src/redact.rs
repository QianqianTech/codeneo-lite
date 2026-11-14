/// 日志和诊断里不出现完整令牌、密码或 Basic 凭证。
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if secret.len() >= 4 {
            out = out.replace(secret, "***");
        }
    }
    out = redact_marked(&out, "Token ");
    out = redact_marked(&out, "Basic ");
    out = redact_marked(&out, "Bearer ");
    out = redact_assignments(&out);
    out = redact_userinfo(&out);
    out
}

fn redact_marked(text: &str, marker: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(idx) = rest.to_ascii_lowercase().find(&marker.to_ascii_lowercase()) {
        out.push_str(&rest[..idx + marker.len()]);
        rest = &rest[idx + marker.len()..];
        let end = rest.find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ';' | '}' | ']' | '&' | '#')).unwrap_or(rest.len());
        if end > 0 {
            out.push_str("***");
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    key.contains("password") || key == "passwd" || key == "pwd" || key.contains("secret") ||
        key.contains("token") || key.contains("api_key") || key.contains("apikey") ||
        matches!(key.as_str(), "authorization" | "proxy_authorization" | "cookie" | "set_cookie")
}

fn redact_assignments(text: &str) -> String {
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() && bytes[i] != b'_' { i += 1; continue; }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'-')) { i += 1; }
        if !sensitive_key(&text[start..i]) { continue; }
        let mut pos = i;
        if pos < bytes.len() && matches!(bytes[pos], b'"' | b'\'') { pos += 1; }
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') { pos += 1; }
        if pos >= bytes.len() || !matches!(bytes[pos], b':' | b'=') { continue; }
        pos += 1;
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') { pos += 1; }
        let quote = bytes.get(pos).copied().filter(|b| matches!(b, b'"' | b'\''));
        if quote.is_some() { pos += 1; }
        let value_start = pos;
        let whole_header = quote.is_none() && matches!(text[start..i].to_ascii_lowercase().as_str(), "cookie" | "set-cookie");
        while pos < bytes.len() {
            if let Some(quote) = quote {
                if bytes[pos] == quote { break; }
                if bytes[pos] == b'\\' { pos = (pos + 2).min(bytes.len()); continue; }
            } else if whole_header {
                if matches!(bytes[pos], b'\r' | b'\n') { break; }
            } else if bytes[pos].is_ascii_whitespace() || matches!(bytes[pos], b',' | b';' | b'&' | b'#' | b'}' | b']' | b'"' | b'\'') { break; }
            pos += 1;
        }
        if pos > value_start {
            out.push_str(&text[copied..value_start]);
            out.push_str("***");
            copied = pos;
            i = pos;
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn redact_userinfo(text: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while let Some(scheme_end) = text[i..].find("://") {
        let authority_start = i + scheme_end + 3;
        out.push_str(&text[i..authority_start]);
        let end = text[authority_start..].find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>')).map(|end| authority_start + end).unwrap_or(text.len());
        let url = &text[authority_start..end];
        let authority_end = url.find(['/', '?', '#']).unwrap_or(url.len());
        let authority = &url[..authority_end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("***@");
            out.push_str(&authority[at + 1..]);
        } else { out.push_str(authority); }
        let tail = &url[authority_end..];
        if let Some(query) = tail.find(['?', '#']) {
            out.push_str(&tail[..query]);
            out.push_str("?[已隐藏参数]");
        } else { out.push_str(tail); }
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

pub fn last4(secret: &str) -> String {
    let trimmed = secret.trim();
    let count = trimmed.chars().count();
    if count < 8 {
        return "****".into();
    }
    trimmed.chars().skip(count - 4).collect()
}

pub fn username_tail(username: &str) -> String {
    let count = username.chars().count();
    if count <= 2 {
        return "**".into();
    }
    username.chars().skip(count - 2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_do_not_survive_logs() {
        let line = "Token abcdefghijk proxy socks5://alice:s3cret@p.webshare.io:1080 Basic QWxhZGRpbjpvcGVu";
        let cleaned = redact(line, &["s3cret", "abcdefghijk"]);
        assert!(!cleaned.contains("s3cret"));
        assert!(!cleaned.contains("abcdefghijk"));
        assert!(!cleaned.contains("QWxhZGRpbjpvcGVu"));
        assert!(!cleaned.contains("alice:s3cret"));
        assert_eq!(last4("abcdefghijk"), "hijk");
        assert_eq!(last4("short"), "****");
        assert_eq!(username_tail("qwxvzrtp"), "tp");
    }

    #[test]
    fn unknown_secrets_headers_json_and_url_parameters_are_redacted() {
        let text = r#"authorization: bearer private-auth {"password":"短密","api_key":"external-key","refresh_token":"refresh-secret"} ANTHROPIC_AUTH_TOKEN=env-secret x-api-key: external-header-key https://user@host/path?opaque=unregistered#fragment http://alice:pw@proxy:8080 a@b"#;
        let cleaned = redact(text, &[]);
        for secret in ["private-auth", "短密", "external-key", "external-header-key", "refresh-secret", "env-secret", "user@", "opaque=", "unregistered", "fragment", "alice", "pw@"] {
            assert!(!cleaned.contains(secret), "leaked {secret}: {cleaned}");
        }
        assert!(cleaned.contains("host/path"));
        assert!(cleaned.contains("proxy:8080"));
        assert!(cleaned.contains("a@b"));
        assert_eq!(redact("https://host/path user: elsewhere@example.test", &[]), "https://host/path user: elsewhere@example.test");
        assert_eq!(redact("Cookie: one=first-secret; two=second-secret\nnext", &[]), "Cookie: ***\nnext");
    }
}
