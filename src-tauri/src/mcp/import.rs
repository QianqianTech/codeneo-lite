use super::{connection, entry, name_valid, options, transports, Input};
use crate::{files, jsonc, transaction::Client};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Serialize)]
pub struct Imported {
    #[serde(flatten)]
    input: Input,
    skipped: Vec<String>,
}

pub fn parse_import(client: Client, raw: &str) -> Result<Imported, String> {
    if raw.len() > files::LIMIT {
        return Err("MCP 配置片段过长".into());
    }
    let raw = raw.trim_start_matches('\u{feff}').trim();
    if raw.is_empty() {
        return Err("请粘贴服务提供的 JSON、JSONC 或 Codex TOML 配置".into());
    }
    let mut doc: Map<String, Value> =
        if raw.starts_with('{') || raw.starts_with("//") || raw.starts_with("/*") {
            jsonc::document(Some(raw.as_bytes()))?.decode()?
        } else {
            toml_edit::de::from_str(raw)
                .map_err(|_| "无法读取 MCP 配置，请粘贴有效的 JSON、JSONC 或 Codex TOML")?
        };
    // Interpret URL and header aliases according to the source schema first.
    let source = if doc.contains_key("mcp_servers") {
        Client::Codex
    } else if doc.contains_key("mcp") && !doc.contains_key("mcpServers") {
        Client::Opencode
    } else {
        client
    };
    let (name, server) = if let Some(map) = doc
        .remove("mcpServers")
        .or_else(|| doc.remove("mcp_servers"))
        .or_else(|| doc.remove("servers"))
        .or_else(|| {
            doc.remove("mcp").map(|mut value| {
                value
                    .as_object_mut()
                    .and_then(|obj| obj.remove("servers"))
                    .unwrap_or(value)
            })
        }) {
        one_server(map)?
    } else if ["command", "url", "httpUrl", "serverUrl"]
        .iter()
        .any(|key| doc.contains_key(*key))
    {
        let name = doc
            .remove("name")
            .and_then(|name| name.as_str().map(str::to_owned))
            .unwrap_or_default();
        (name, Value::Object(doc))
    } else {
        one_server(Value::Object(doc))?
    };
    if !name.is_empty() && !name_valid(&name) {
        return Err("配置里的服务名称过长或含控制字符".into());
    }
    let source = if server.get("httpUrl").is_some() {
        Client::Qwen
    } else if source == Client::Qwen && server["type"] == "remote" {
        Client::Opencode
    } else {
        source
    };
    let config = connection(source, &server)?;
    if !transports(client).contains(&config.transport) {
        return Err(
            "目标客户端不支持片段中的连接方式，请切换客户端或使用该服务提供的其他配置".into(),
        );
    }
    entry(client, &config)?;
    let advanced = options::read(client, config.transport, &server);
    let object = server.as_object().ok_or("MCP 服务必须是配置对象")?;
    let endpoint = super::existing_key(&server, config.transport.endpoint_keys(source));
    let option = super::existing_key(&server, config.transport.option_keys(source));
    let skipped = object
        .keys()
        .filter(|key| {
            ![endpoint, option, "type", "transport"].contains(&key.as_str())
                && !(key.as_str() == "args"
                    && config.transport == super::Transport::Stdio
                    && !server[endpoint].is_array())
                && !advanced.contains_key(*key)
        })
        .cloned()
        .collect();
    Ok(Imported {
        input: Input {
            name,
            connection: config,
            revision: None,
            advanced: advanced
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .collect(),
        },
        skipped,
    })
}

fn one_server(value: Value) -> Result<(String, Value), String> {
    match value {
        Value::Object(map) if map.len() == 1 => Ok(map.into_iter().next().expect("one server")),
        _ => Err("请一次粘贴一个 MCP 服务的配置".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imports_a_single_service_and_translates_known_client_fields() {
        let imported = parse_import(
            Client::Codex,
            r#"{
          // Qwen-style config
          "mcpServers": { "example": {
            "httpUrl": "https://example.invalid/mcp",
            "headers": {"X-Token": "fixture"},
            "trust": true,
          }}
        }"#,
        )
        .unwrap();
        assert_eq!(imported.input.name, "example");
        assert_eq!(imported.skipped, ["trust"]);
        let server = entry(Client::Codex, &imported.input.connection).unwrap();
        assert_eq!(server["http_headers"], json!({"X-Token": "fixture"}));
        assert!(server.get("trust").is_none());

        let imported = parse_import(
            Client::Codex,
            r#"
          [mcp_servers.example]
          url = "https://example.invalid/mcp"
          bearer_token_env_var = "EXAMPLE_TOKEN"
          startup_timeout_sec = 2.5
        "#,
        )
        .unwrap();
        assert!(imported.skipped.is_empty());
        assert_eq!(
            imported.input.advanced["startup_timeout_sec"],
            Some(json!(2.5))
        );
        assert_eq!(
            imported.input.advanced["bearer_token_env_var"],
            Some(json!("EXAMPLE_TOKEN"))
        );

        let imported = parse_import(
            Client::Codex,
            r#"{"mcpServers":{"vendor.tool":{"url":"https://example.invalid/mcp"}}}"#,
        )
        .unwrap();
        assert_eq!(imported.input.name, "vendor.tool");
    }

    #[test]
    fn rejects_unsupported_transport_and_whole_multi_service_configurations() {
        let remote =
            r#"{"mcpServers":{"example":{"type":"sse","url":"https://example.invalid/sse"}}}"#;
        assert!(parse_import(Client::Codex, remote).is_err());
        assert!(parse_import(Client::Code, remote).is_ok());
        assert!(parse_import(Client::Code, r#"{"mcpServers":{"a":{},"b":{}}}"#).is_err());
        assert!(parse_import(
            Client::Code,
            r#"{"mcpServers":{"a":{"type":"ws","url":"https://example.invalid"}}}"#
        )
        .is_err());
    }
}
