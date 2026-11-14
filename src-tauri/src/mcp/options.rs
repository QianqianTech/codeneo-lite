use super::Transport;
use crate::transaction::Client;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Text,
    Variable,
    Timeout,
    List,
    Switch,
}

#[derive(Clone, Copy, Serialize)]
pub struct Field {
    key: &'static str,
    label: &'static str,
    kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<Transport>,
}

const fn field(key: &'static str, label: &'static str, kind: Kind) -> Field {
    Field {
        key,
        label,
        kind,
        transport: None,
    }
}

const CWD: Field = Field {
    transport: Some(Transport::Stdio),
    ..field("cwd", "工作目录", Kind::Text)
};
const fn bearer(key: &'static str) -> Field {
    Field {
        transport: Some(Transport::Http),
        ..field(key, "Bearer Token 的环境变量名", Kind::Variable)
    }
}
const TIMEOUT: Field = field("timeout", "超时（毫秒）", Kind::Timeout);

pub const fn fields(client: Client) -> &'static [Field] {
    use Client::*;
    use Kind::*;
    match client {
        Codex => {
            const {
                &[
                    CWD,
                    bearer("bearer_token_env_var"),
                    field("startup_timeout_sec", "启动超时（秒）", Timeout),
                    field("tool_timeout_sec", "工具调用超时（秒）", Timeout),
                    field("enabled_tools", "允许的工具", List),
                    field("disabled_tools", "禁用的工具", List),
                ]
            }
        }
        Opencode => &[CWD, TIMEOUT],
        Qwen => {
            const {
                &[
                    CWD,
                    TIMEOUT,
                    field("includeTools", "允许的工具", List),
                    field("excludeTools", "禁用的工具", List),
                ]
            }
        }
        Codebuddy => const { &[field("defer_loading", "按需加载工具", Switch)] },
        Kimi => {
            const {
                &[
                    CWD,
                    bearer("bearerTokenEnvVar"),
                    field("startupTimeoutMs", "启动超时（毫秒）", Timeout),
                    field("toolTimeoutMs", "工具调用超时（毫秒）", Timeout),
                    field("enabledTools", "允许的工具", List),
                    field("disabledTools", "禁用的工具", List),
                ]
            }
        }
        Copilot => const { &[TIMEOUT, field("tools", "允许的工具", List)] },
        _ => &[],
    }
}

fn applies(field: Field, transport: Transport) -> bool {
    match field.transport {
        Some(Transport::Http) => transport != Transport::Stdio,
        Some(required) => required == transport,
        None => true,
    }
}

fn valid(field: Field, value: &Value) -> bool {
    match field.kind {
        Kind::Text => value.as_str().is_some_and(|v| {
            !v.trim().is_empty() && v.len() <= 4096 && !v.chars().any(char::is_control)
        }),
        Kind::Variable => value.as_str().is_some_and(variable_name),
        Kind::Timeout => value.as_f64().is_some_and(|v| {
            v > 0.0 && v <= i32::MAX as f64 && (field.key.ends_with("_sec") || v.fract() == 0.0)
        }),
        Kind::List => value.as_array().is_some_and(|v| {
            v.len() <= 512
                && v.iter().all(|item| {
                    item.as_str().is_some_and(|name| {
                        !name.trim().is_empty()
                            && name.len() <= 256
                            && !name.chars().any(char::is_control)
                    })
                })
        }),
        Kind::Switch => value.is_boolean(),
    }
}

pub(super) fn variable_name(value: &str) -> bool {
    let mut chars = value.bytes();
    value.len() <= 128
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

pub fn read(client: Client, transport: Transport, server: &Value) -> BTreeMap<String, Value> {
    fields(client)
        .iter()
        .filter_map(|&field| {
            let value = server.get(field.key)?;
            (applies(field, transport) && valid(field, value))
                .then(|| (field.key.into(), value.clone()))
        })
        .collect()
}

pub fn apply(
    client: Client,
    before: Option<Transport>,
    after: Transport,
    changes: &BTreeMap<String, Option<Value>>,
    server: &mut Map<String, Value>,
) -> Result<(), String> {
    if before.is_some_and(|old| old != after) {
        for &field in fields(client) {
            if !applies(field, after) {
                server.remove(field.key);
            }
        }
    }
    for (key, change) in changes {
        let field = fields(client)
            .iter()
            .copied()
            .find(|&field| field.key == key && applies(field, after))
            .ok_or("此客户端或连接方式不支持其中一项高级设置")?;
        if let Some(value) = change {
            if !valid(field, value) {
                return Err(format!("「{}」格式无效", field.label));
            }
            if client == Client::Codex && field.key == "startup_timeout_sec" {
                server.remove("startup_timeout_ms");
            }
            server.insert(key.clone(), value.clone());
        } else if client == Client::Copilot && field.key == "tools" {
            server.insert(key.clone(), serde_json::json!(["*"]));
        } else {
            server.remove(key);
            if client == Client::Codex && field.key == "startup_timeout_sec" {
                server.remove("startup_timeout_ms");
            }
        }
    }
    Ok(())
}
