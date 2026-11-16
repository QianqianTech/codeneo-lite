use super::{add_usage, in_range, number, records, safe_model, scan_logs, Row, Tokens};
use crate::{agents, models, transaction::Client, usage_prices};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    time::{Duration, SystemTime},
};
use time::OffsetDateTime;

const OPENCODE: &str = "OpenCode 本机会话数据库，按请求时间统计。";
const QWEN: &str = "Qwen Code 本机 /stats 记录。订阅及未标明服务商的调用显示 Token。";
const CODEBUDDY: &str = "CodeBuddy Code 本机会话的模型请求。可在原客户端用 /cost 核对。";
const KIMI: &str = "Kimi Code 本机 usage.record 事件，包含独立子 Agent 的记录。";
const COPILOT: &str = "Copilot CLI 本机会话记录。GitHub 套餐与 BYOK 用量分别显示。";

pub(super) fn scan(
    client: Client,
    cutoff: SystemTime,
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
    store: Option<&models::Store>,
) -> (&'static str, &'static str) {
    use Client::*;
    let (result, detail) = match client {
        Opencode => (opencode(start, rows, store), OPENCODE),
        Qwen => (qwen(cutoff, start, rows), QWEN),
        Kimi => (kimi(cutoff, start, rows, store), KIMI),
        Copilot => (copilot(cutoff, start, rows, store), COPILOT),
        Codebuddy => (codebuddy(cutoff, start, rows), CODEBUDDY),
        Workbuddy => return ("unsupported", "WorkBuddy 暂无已验证的本机 Token 来源。"),
        _ => return ("unsupported", ""),
    };
    let state = match result {
        Ok(false) | Err(_) => "partial",
        Ok(true) if rows.is_empty() => "empty",
        Ok(true) => "available",
    };
    (state, detail)
}

fn known_provider(provider: Option<&str>, store: Option<&models::Store>) -> Option<String> {
    let provider = provider?;
    let Some(id) = provider.strip_prefix("codeneo-lite-") else {
        return Some(provider.to_owned());
    };
    let account = store?.accounts.iter().find(|account| account.id == id)?;
    let url = reqwest::Url::parse(&account.base_url).ok()?;
    url.host_str().map(str::to_owned)
}

fn row(
    rows: &mut BTreeMap<String, Row>,
    client: Client,
    model: Option<&str>,
    provider: Option<&str>,
    tokens: Tokens,
    per_request: bool,
) {
    let model = safe_model(model);
    let provider = provider
        .map(|id| safe_model(Some(id)))
        .filter(|id| *id != "未知模型");
    let estimate = provider
        .ok_or_else(usage_prices::Unpriced::provider_unknown)
        .and_then(|id| usage_prices::for_provider(id, model, tokens, per_request));
    add_usage(rows, client, model, provider, tokens, estimate);
}

fn timestamp_in_range(record: &Value, start: OffsetDateTime) -> bool {
    if let Some(value) = record["timestamp"]
        .as_i64()
        .or_else(|| record["time"].as_i64())
    {
        let ms = if value < 10_000_000_000 {
            value.saturating_mul(1000)
        } else {
            value
        };
        return i128::from(ms) * 1_000_000 >= start.unix_timestamp_nanos();
    }
    in_range(record, start)
}

fn sqlite(path: &Path) -> Result<Option<Connection>, String> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取本机用量数据库".into()),
    };
    if !metadata.is_file() {
        return Err("本机用量数据库路径无效".into());
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let db = Connection::open_with_flags(path, flags).map_err(|_| "无法打开本机用量数据库")?;
    db.busy_timeout(Duration::from_millis(200))
        .map_err(|_| "用量数据库暂时不可读")?;
    db.execute_batch("PRAGMA query_only = ON; PRAGMA trusted_schema = OFF")
        .map_err(|_| "无法安全读取用量数据库")?;
    Ok(Some(db))
}

fn table(db: &Connection, name: &str) -> bool {
    db.query_row(
        "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .is_ok()
}

fn opencode(
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
    store: Option<&models::Store>,
) -> Result<bool, String> {
    let data = agents::location("XDG_DATA_HOME", agents::home()?.join(".local/share"))?;
    let Some(db) = sqlite(&data.join("opencode/opencode.db"))? else {
        return Ok(true);
    };
    opencode_db(&db, start, rows, store)
}

fn opencode_db(
    db: &Connection,
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
    store: Option<&models::Store>,
) -> Result<bool, String> {
    let has_v1 = table(db, "message");
    let has_v2 = table(db, "session_message");
    if !has_v1 && !has_v2 {
        return Err("未识别 OpenCode 用量数据库版本".into());
    }
    let mut seen = HashSet::new();
    // A V2 migration may leave the legacy rows in the database. Visit the
    // projected messages first and count a message ID at most once.
    let queries = [
        (
            has_v2,
            "SELECT id, json_extract(data,'$.model.id'), json_extract(data,'$.model.providerID'), \
            json_extract(data,'$.tokens') FROM session_message WHERE time_created >= ?1 \
            AND type IN ('assistant','compaction') AND json_valid(data) LIMIT 50001",
        ),
        (
            has_v1,
            "SELECT id, json_extract(data,'$.modelID'), json_extract(data,'$.providerID'), \
            json_extract(data,'$.tokens') FROM message WHERE time_created >= ?1 \
            AND json_valid(data) AND json_extract(data,'$.role') = 'assistant' LIMIT 50001",
        ),
    ];
    let start = (start.unix_timestamp_nanos() / 1_000_000) as i64;
    let mut complete = true;
    for (_, query) in queries.into_iter().filter(|(exists, _)| *exists) {
        let mut stmt = db
            .prepare(query)
            .map_err(|_| "OpenCode 用量数据库结构无法读取")?;
        let mut found = stmt
            .query(params![start])
            .map_err(|_| "无法查询 OpenCode 用量")?;
        let mut count = 0;
        while let Some(item) = found.next().map_err(|_| "无法读取 OpenCode 用量")? {
            count += 1;
            if count > 50_000 {
                complete = false;
                break;
            }
            let (Ok(id), Ok(model), Ok(provider), Ok(Some(tokens))) = (
                item.get::<_, String>(0),
                item.get::<_, Option<String>>(1),
                item.get::<_, Option<String>>(2),
                item.get::<_, Option<String>>(3),
            ) else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let Ok(tokens) = serde_json::from_str::<Value>(&tokens) else {
                complete = false;
                continue;
            };
            let provider = known_provider(provider.as_deref(), store).or(provider);
            row(
                rows,
                Client::Opencode,
                model.as_deref(),
                provider.as_deref(),
                Tokens {
                    input: number(&tokens, "input"),
                    output: number(&tokens, "output").saturating_add(number(&tokens, "reasoning")),
                    cache_read: number(&tokens["cache"], "read"),
                    cache_write: number(&tokens["cache"], "write"),
                    ..Tokens::default()
                },
                true,
            );
        }
    }
    Ok(complete)
}

fn qwen(
    cutoff: SystemTime,
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
) -> Result<bool, String> {
    let home = agents::settings(Client::Qwen)?;
    let base = home.parent().ok_or("找不到 Qwen 用量目录")?;
    let runtime = agents::location("QWEN_RUNTIME_DIR", base.to_path_buf())?;
    let roots = [runtime.join("usage"), runtime.join("stats")];
    let mut seen = HashSet::new();
    Ok(scan_logs(&roots, cutoff, |path, size| {
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("token-usage-"))
        {
            return true;
        }
        records(path, size, |record| {
            if record["schemaVersion"] != 1 || !in_range(&record, start) {
                return;
            }
            let Some(id) = record["id"].as_str().filter(|id| !id.is_empty()) else {
                return;
            };
            if !seen.insert(id.to_owned()) {
                return;
            }
            let cached = number(&record, "cachedTokens");
            let tokens = Tokens {
                input: number(&record, "inputTokens").saturating_sub(cached),
                output: number(&record, "outputTokens")
                    .saturating_add(number(&record, "thoughtsTokens")),
                cache_read: cached,
                ..Tokens::default()
            };
            let account_type = match record["authType"].as_str() {
                Some("qwen-oauth" | "qwen") => Some("Qwen OAuth"),
                Some("openai") => Some("OpenAI 兼容"),
                Some("anthropic") => Some("Anthropic 兼容"),
                _ => None,
            };
            add_usage(
                rows,
                Client::Qwen,
                safe_model(record["model"].as_str()),
                account_type,
                tokens,
                Err(usage_prices::Unpriced::provider_unknown()),
            );
        })
    }))
}

fn kimi(
    cutoff: SystemTime,
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
    store: Option<&models::Store>,
) -> Result<bool, String> {
    let path = agents::settings(Client::Kimi)?;
    let base = path.parent().ok_or("找不到 Kimi 用量目录")?;
    let mut seen = HashSet::new();
    Ok(scan_logs(&[base.join("sessions")], cutoff, |path, size| {
        if path.file_name().and_then(|name| name.to_str()) != Some("wire.jsonl") {
            return true;
        }
        let mut provider = None;
        records(path, size, |record| {
            if record["type"] == "llm.request" {
                provider = record["provider"].as_str().map(str::to_owned);
            }
            if record["type"] != "usage.record" || !timestamp_in_range(&record, start) {
                return;
            }
            let usage = &record["usage"];
            let tokens = Tokens {
                input: number(usage, "inputOther"),
                output: number(usage, "output"),
                cache_read: number(usage, "inputCacheRead"),
                cache_write: number(usage, "inputCacheCreation"),
                ..Tokens::default()
            };
            let Some(session) = path.ancestors().nth(3).and_then(Path::file_name) else {
                return;
            };
            let Some(agent) = path.parent().and_then(Path::file_name) else {
                return;
            };
            let key = (
                session.to_owned(),
                agent.to_owned(),
                record["time"].as_i64(),
                record["model"].as_str().unwrap_or_default().to_owned(),
                tokens.total(),
            );
            if key.2.is_some() && !seen.insert(key) {
                return;
            }
            let recorded = record["provider"].as_str().or(provider.as_deref());
            let known = known_provider(recorded, store);
            row(
                rows,
                Client::Kimi,
                record["model"].as_str(),
                known.as_deref().or(recorded),
                tokens,
                true,
            );
        })
    }))
}

fn codebuddy(
    cutoff: SystemTime,
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
) -> Result<bool, String> {
    let roots = [agents::codebuddy_home()?.join("projects")];
    let mut messages = BTreeMap::new();
    let complete = scan_logs(&roots, cutoff, |path, size| {
        records(path, size, |record| {
            if record["type"] != "message"
                || record["role"] != "assistant"
                || !timestamp_in_range(&record, start)
            {
                return;
            }
            let meta = &record["providerData"];
            let Some(id) = record["id"].as_str().or_else(|| meta["messageId"].as_str()) else {
                return;
            };
            let usage = &record["message"]["usage"];
            let raw = &meta["rawUsage"];
            let cached = raw["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .or_else(|| raw["cache_read_input_tokens"].as_u64())
                .or_else(|| usage["cache_read_input_tokens"].as_u64())
                .or_else(|| {
                    meta["usage"]["inputTokensDetails"]
                        .as_array()
                        .and_then(|items| {
                            items.iter().find_map(|item| item["cached_tokens"].as_u64())
                        })
                })
                .unwrap_or(0);
            let input = raw["prompt_tokens"]
                .as_u64()
                .or_else(|| raw["input_tokens"].as_u64())
                .or_else(|| usage["input_tokens"].as_u64())
                .unwrap_or(0);
            let cache_is_separate = raw["prompt_tokens"].is_null()
                && (raw["cache_read_input_tokens"].is_number()
                    || raw["cache_creation_input_tokens"].is_number());
            let tokens = Tokens {
                input: if cache_is_separate {
                    input
                } else {
                    input.saturating_sub(cached)
                },
                output: raw["completion_tokens"]
                    .as_u64()
                    .unwrap_or_else(|| number(usage, "output_tokens")),
                cache_read: cached,
                cache_write: raw["cache_creation_input_tokens"]
                    .as_u64()
                    .unwrap_or_else(|| number(usage, "cache_creation_input_tokens")),
                ..Tokens::default()
            };
            let model = safe_model(
                meta["model"]
                    .as_str()
                    .or_else(|| meta["requestModelName"].as_str()),
            );
            let row = messages
                .entry(id.to_owned())
                .or_insert_with(|| (model.to_owned(), tokens));
            if row.1.total() < tokens.total() {
                *row = (model.to_owned(), tokens);
            }
        })
    });
    for (_, (model, tokens)) in messages {
        add_usage(
            rows,
            Client::Codebuddy,
            &model,
            None,
            tokens,
            Err(usage_prices::Unpriced::provider_unknown()),
        );
    }
    Ok(complete)
}

fn copilot(
    cutoff: SystemTime,
    start: OffsetDateTime,
    rows: &mut BTreeMap<String, Row>,
    store: Option<&models::Store>,
) -> Result<bool, String> {
    let mut seen = HashSet::new();
    Ok(scan_logs(
        &[agents::copilot_home()?.join("session-state")],
        cutoff,
        |path, size| {
            if path.file_name().and_then(|name| name.to_str()) != Some("events.jsonl") {
                return true;
            }
            records(path, size, |record| {
                if record["type"] != "session.usage_record" || !timestamp_in_range(&record, start) {
                    return;
                }
                let data = &record["data"]["usage"];
                let accounting = &data["accounting"];
                if let (Some(session), Some(sequence)) = (
                    accounting["sourceSessionId"].as_str(),
                    accounting["sequence"].as_u64(),
                ) {
                    if !seen.insert((session.to_owned(), sequence)) {
                        return;
                    }
                }
                let read = number(data, "cacheReadTokens");
                let write = number(data, "cacheWriteTokens");
                // OpenAI counts cached input in inputTokens. Anthropic reports its
                // cache fields outside inputTokens.
                let input = if data["providerType"] == "anthropic" {
                    number(data, "inputTokens")
                } else {
                    number(data, "inputTokens")
                        .saturating_sub(read)
                        .saturating_sub(write)
                };
                let byok = data["isByok"] == true;
                let model = safe_model(
                    data["responseModel"]
                        .as_str()
                        .or_else(|| data["model"].as_str()),
                );
                let tokens = Tokens {
                    input,
                    output: number(data, "outputTokens"),
                    cache_read: read,
                    cache_write: write,
                    ..Tokens::default()
                };
                let provider = if byok {
                    let managed = data["modelDisplayName"]
                        .as_str()
                        .and_then(|name| name.split_once('/'))
                        .map(|(provider, _)| provider)
                        .filter(|provider| provider.starts_with("codeneo-lite-"));
                    known_provider(managed, store)
                } else {
                    None
                };
                let estimate = provider
                    .as_deref()
                    .ok_or_else(usage_prices::Unpriced::provider_unknown)
                    .and_then(|provider| usage_prices::for_provider(provider, model, tokens, true));
                add_usage(
                    rows,
                    Client::Copilot,
                    model,
                    provider
                        .as_deref()
                        .or(Some(if byok { "BYOK" } else { "GitHub Copilot" })),
                    tokens,
                    estimate,
                );
            })
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_v1_and_v2_usage_once_and_prices_each_provider_separately() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(r#"
            CREATE TABLE message (id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE session_message (id TEXT, time_created INTEGER, type TEXT, data TEXT);
            INSERT INTO message VALUES
              ('shared', 1, '{"role":"assistant","modelID":"gpt-5.4-mini","providerID":"openai","tokens":{"input":40}}'),
              ('other', 1, '{"role":"assistant","modelID":"gpt-5.4-mini","providerID":"router","tokens":{"input":10}}');
            INSERT INTO session_message VALUES
              ('shared', 1, 'assistant', '{"model":{"id":"gpt-5.4-mini","providerID":"openai"},"tokens":{"input":100,"output":3,"reasoning":2,"cache":{"read":4,"write":1}}}');
        "#).unwrap();
        let mut rows = BTreeMap::new();
        assert!(opencode_db(&db, OffsetDateTime::UNIX_EPOCH, &mut rows, None).unwrap());
        assert_eq!(rows.len(), 2);
        let official = &rows["gpt-5.4-mini\nopenai"];
        assert_eq!(official.tokens.total(), 110);
        assert!(official.estimate.is_some());
        assert!(rows["gpt-5.4-mini\nrouter"].estimate.is_none());
        assert_eq!(
            usage_prices::for_provider("openai", "gpt-5.4", official.tokens, false)
                .unwrap_err()
                .reason,
            usage_prices::Reason::RequestDetailRequired
        );
    }
}
