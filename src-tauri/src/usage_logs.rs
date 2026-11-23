use super::{number, read_records, safe_model, timestamp, LogRead, Tokens};
use memchr::memmem::Finder;
use serde::Deserialize;
use serde_json::Value;
use std::{path::Path, sync::Arc};
use time::OffsetDateTime;

pub(super) struct Parsed<T> {
    pub usage: Vec<T>,
    pub read: LogRead,
}

pub(super) struct ClaudeUsage {
    pub time: OffsetDateTime,
    pub id: String,
    pub model: String,
    pub tokens: Tokens,
}

#[derive(Deserialize)]
struct ClaudeRecord {
    #[serde(rename = "type")]
    kind: String,
    timestamp: String,
    message: ClaudeMessage,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: String,
    #[serde(default)]
    model: Value,
    #[serde(default)]
    usage: Value,
}

fn claude_usage(usage: &Value) -> Tokens {
    let total_writes = number(usage, "cache_creation_input_tokens");
    let long_writes =
        number(&usage["cache_creation"], "ephemeral_1h_input_tokens").min(total_writes);
    Tokens {
        input: number(usage, "input_tokens"),
        output: number(usage, "output_tokens"),
        cache_read: number(usage, "cache_read_input_tokens"),
        cache_write: total_writes - long_writes,
        cache_write_long: long_writes,
    }
}

fn claude_record(line: &[u8]) -> Option<ClaudeUsage> {
    let record: ClaudeRecord = serde_json::from_slice(line).ok()?;
    if record.kind != "assistant" || record.message.id.is_empty() {
        return None;
    }
    let tokens = claude_usage(&record.message.usage);
    if tokens.total() == 0 {
        return None;
    }
    Some(ClaudeUsage {
        time: timestamp(&record.timestamp)?,
        id: record.message.id,
        model: safe_model(record.message.model.as_str()).to_owned(),
        tokens,
    })
}

pub(super) fn claude(path: &Path, size: u64) -> Parsed<ClaudeUsage> {
    let marker = Finder::new(b"\"usage\"");
    let mut usage = Vec::new();
    let read = read_records(
        path,
        size,
        |_| false,
        |line| {
            if marker.find(line).is_some() {
                usage.extend(claude_record(line));
            }
        },
    );
    Parsed { usage, read }
}

pub(super) struct CodexUsage {
    pub time: Option<OffsetDateTime>,
    pub id: Option<String>,
    pub model: Arc<str>,
    pub tokens: Tokens,
}

const CODEX_MARKERS: &[&[u8]] = &[
    b"\"usage\"",
    b"\"token_usage_record\"",
    b"\"token_count\"",
    b"\"turn_context\"",
    b"\"session_meta\"",
];

#[derive(Deserialize)]
struct CodexRecord {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: Value,
    #[serde(default)]
    payload: Option<CodexPayload>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CodexPayload {
    #[serde(rename = "type")]
    kind: Value,
    parent_thread_id: Value,
    model: Value,
    response_id: Value,
    thread_token_usage: Value,
    usage: Value,
    info: Value,
}

fn codex_raw(usage: &Value) -> [u64; 4] {
    [
        number(usage, "input_tokens"),
        number(usage, "output_tokens"),
        number(usage, "cached_input_tokens"),
        number(usage, "cache_write_input_tokens"),
    ]
}

fn codex_usage([input, output, cache_read, cache_write]: [u64; 4]) -> Tokens {
    let cache_read = cache_read.min(input);
    let cache_write = cache_write.min(input - cache_read);
    Tokens {
        input: input - cache_read - cache_write,
        output,
        cache_read,
        cache_write,
        cache_write_long: 0,
    }
}

#[derive(Default)]
struct CodexLog {
    model: Arc<str>,
    previous: [u64; 4],
    seen_count: bool,
    detailed_since_count: bool,
    forked: bool,
}

impl CodexLog {
    fn record(&mut self, record: CodexRecord) -> Option<CodexUsage> {
        let payload = record.payload.unwrap_or_default();
        let (id, tokens) = match record.kind.as_str() {
            "session_meta" => {
                self.forked = !payload.parent_thread_id.is_null();
                return None;
            }
            "turn_context" => {
                self.model = Arc::from(safe_model(payload.model.as_str()));
                return None;
            }
            "token_usage_record" => {
                self.detailed_since_count = true;
                if payload.thread_token_usage.is_object() {
                    self.previous = codex_raw(&payload.thread_token_usage);
                    self.seen_count = true;
                }
                let id = match payload.response_id {
                    Value::String(id) if !id.is_empty() => Some(id),
                    _ => None,
                };
                (id, codex_usage(codex_raw(&payload.usage)))
            }
            "event_msg"
                if payload.kind == "token_count"
                    && payload.info["total_token_usage"].is_object() =>
            {
                let current = codex_raw(&payload.info["total_token_usage"]);
                let previous = std::mem::replace(&mut self.previous, current);
                let seen_count = std::mem::replace(&mut self.seen_count, true);
                let detailed = std::mem::take(&mut self.detailed_since_count);
                if detailed
                    || self.forked && !seen_count
                    || (0..4).any(|i| current[i] < previous[i])
                {
                    return None;
                }
                let tokens = codex_usage(std::array::from_fn(|i| current[i] - previous[i]));
                (None, tokens)
            }
            _ => return None,
        };
        if tokens.total() == 0 && id.is_none() {
            return None;
        }
        Some(CodexUsage {
            time: record.timestamp.as_str().and_then(timestamp),
            id,
            model: Arc::clone(&self.model),
            tokens,
        })
    }
}

pub(super) fn codex(path: &Path, size: u64) -> Parsed<CodexUsage> {
    let mut log = CodexLog {
        model: Arc::from(safe_model(None)),
        ..CodexLog::default()
    };
    let markers: Vec<_> = CODEX_MARKERS.iter().map(Finder::new).collect();
    let mut usage = Vec::new();
    let read = read_records(path, size, codex_non_usage, |line| {
        if codex_non_usage(line) || !markers.iter().any(|marker| marker.find(line).is_some()) {
            return;
        }
        if let Ok(record) = serde_json::from_slice(line) {
            usage.extend(log.record(record));
        }
    });
    Parsed { usage, read }
}

fn codex_kind_prefix(line: &[u8]) -> Option<&[u8]> {
    // Current rollouts start with the timestamp, optional ordinal, then type.
    // When that layout differs, let the marker search examine the whole line.
    let rest = line.strip_prefix(b"{\"timestamp\":\"")?;
    let end = memchr::memchr2(b'"', b'\\', rest.get(..40).unwrap_or(rest))?;
    if rest[end] != b'"' {
        return None;
    }
    let mut rest = &rest[end + 1..];
    if let Some(ordinal) = rest.strip_prefix(b",\"ordinal\":") {
        let end = memchr::memchr(b',', ordinal.get(..21).unwrap_or(ordinal))?;
        if !ordinal[..end].iter().all(u8::is_ascii_digit) {
            return None;
        }
        rest = &ordinal[end..];
    }
    rest.strip_prefix(b",\"type\":\"")
}

fn codex_non_usage(line: &[u8]) -> bool {
    let Some(kind) = codex_kind_prefix(line) else {
        return false;
    };
    if kind.starts_with(b"response_item\",\"payload\":")
        || kind.starts_with(b"compacted\",\"payload\":")
    {
        return true;
    }
    let Some(event) = kind.strip_prefix(b"event_msg\",\"payload\":{\"type\":\"") else {
        return false;
    };
    memchr::memchr2(b'"', b'\\', event.get(..48).unwrap_or(event))
        .is_some_and(|end| event[end] == b'"' && &event[..end] != b"token_count")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codex_record(value: Value) -> CodexRecord {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn codex_detailed_and_cumulative_records_do_not_double_count() {
        let mut log = CodexLog::default();
        let initial = json!({"input_tokens": 100, "output_tokens": 50, "cached_input_tokens": 30});
        let first = json!({"type":"event_msg", "payload":{"type":"token_count", "info":{"total_token_usage":initial}}});
        assert_eq!(
            log.record(codex_record(first.clone()))
                .unwrap()
                .tokens
                .total(),
            150
        );
        assert!(log.record(codex_record(first)).is_none());
        let total = json!({"input_tokens": 200, "output_tokens": 100, "cached_input_tokens": 60});
        let detail = codex_record(json!({"type":"token_usage_record", "payload":{
            "response_id":"fixture-response", "usage":initial, "thread_token_usage":total
        }}));
        let use_count = log.record(detail).unwrap().tokens;
        assert_eq!(use_count.total(), 150);
        assert_eq!(use_count.input, 70);
        let fallback = codex_record(
            json!({"type":"event_msg", "payload":{"type":"token_count", "info":{"total_token_usage":total}}}),
        );
        assert!(log.record(fallback).is_none());
    }

    #[test]
    fn codex_reads_current_and_legacy_usage_with_reordered_fields() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let lines = [
            r#"{"timestamp":"2026-11-13T00:00:00Z","ordinal":1,"type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#,
            r#"{"timestamp":"2026-11-13T00:00:01Z","ordinal":2,"type":"response_item","payload":{"text":"token_usage_record"}}"#,
            r#"{"timestamp":"2026-11-13T00:00:02Z","ordinal":3,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"cached_input_tokens":7}}}}"#,
            r#"{"type":"token_usage_record","payload":{"response_id":"new-response","usage":{"input_tokens":10,"output_tokens":2}},"timestamp":"2026-11-13T00:00:03Z"}"#,
        ].join("\n");
        std::fs::write(file.path(), &lines).unwrap();
        let parsed = codex(file.path(), lines.len() as u64);
        assert!(parsed.read.complete);
        assert_eq!(parsed.usage.len(), 2);
        assert_eq!(parsed.usage[0].tokens.cache_read, 7);
        assert_eq!(parsed.usage[0].tokens.input, 13);
        assert_eq!(parsed.usage[1].model.as_ref(), "gpt-6.1-sol");
        assert_eq!(parsed.usage[1].id.as_deref(), Some("new-response"));
        assert_eq!(parsed.usage[1].tokens.total(), 12);
    }

    #[test]
    fn codex_gpt_62_usage_keeps_both_models_priced_after_switching() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let lines = ["gpt-6.2-sol", "gpt-6.2-astra"].into_iter().flat_map(|model| [
            json!({"type":"turn_context", "payload":{"model":model}}).to_string(),
            json!({"type":"token_usage_record", "payload":{
                "response_id":format!("{model}-response"),
                "usage":{"input_tokens":1000,"cached_input_tokens":500,
                    "cache_write_input_tokens":100,"output_tokens":200}
            }}).to_string(),
        ]).collect::<Vec<_>>().join("\n");
        std::fs::write(file.path(), &lines).unwrap();
        let parsed = codex(file.path(), lines.len() as u64);
        assert!(parsed.read.complete);
        assert_eq!(parsed.usage.len(), 2);
        let mut rows = std::collections::BTreeMap::new();
        for record in parsed.usage {
            super::super::add(&mut rows, crate::transaction::Client::Codex,
                &record.model, record.tokens);
        }
        for (model, usd) in [("gpt-6.2-sol", 0.0031), ("gpt-6.2-astra", 0.0124)] {
            let row = &rows[&format!("{model}\n")];
            assert_eq!(row.priced_tokens, 1200);
            assert!(row.unpriced.is_empty());
            let price = row.estimate.unwrap();
            assert!((price.high_cny - usd * crate::usage_prices::USD_TO_CNY).abs() < 1e-9);
            assert_eq!(price.source, crate::usage_prices::Source::Openai);
        }
    }

    #[test]
    fn oversized_codex_tool_and_compaction_records_do_not_hide_usage() {
        use std::io::Write;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        let text = "x".repeat(super::super::MAX_LINE as usize);
        for (ordinal, (kind, payload)) in [
            (
                "response_item",
                r#"{"type":"function_call_output","output":""#,
            ),
            ("compacted", r#"{"message":""#),
            ("event_msg", r#"{"type":"exec_command_end","stdout":""#),
        ]
        .into_iter()
        .enumerate()
        {
            writeln!(file, r#"{{"timestamp":"2026-11-13T00:00:00Z","ordinal":{ordinal},"type":"{kind}","payload":{payload}{text}"}}}}"#).unwrap();
        }
        writeln!(file, r#"{{"timestamp":"2026-11-13T00:00:01Z","type":"token_usage_record","payload":{{"response_id":"one","usage":{{"input_tokens":100,"cached_input_tokens":80,"output_tokens":12}}}}}}"#).unwrap();
        let parsed = codex(file.path(), file.as_file().metadata().unwrap().len());
        assert!(parsed.read.complete);
        assert_eq!(parsed.usage.len(), 1);
        assert_eq!(parsed.usage[0].tokens.total(), 112);
    }

    #[test]
    fn prices_cache_tiers_and_excludes_unknown_catalog_ids() {
        let usage = claude_usage(&json!({
            "input_tokens": 1000, "output_tokens": 200, "cache_read_input_tokens": 500,
            "cache_creation_input_tokens": 400, "cache_creation": {"ephemeral_1h_input_tokens": 300}
        }));
        assert_eq!(usage.total(), 2100);
        let sonnet = crate::usage_prices::estimate("claude-sonnet-5-5-20261023", usage).unwrap();
        let expected =
            (1000.0 * 2.0 + 200.0 * 10.0 + 500.0 * 0.1 + 100.0 * 2.5 + 300.0 * 4.0) / 1_000_000.0;
        assert!((sonnet.high_cny - expected * 6.7174).abs() < 1e-9);
        let deepseek = crate::usage_prices::estimate("deepseek-flash", usage).unwrap();
        assert_eq!(deepseek.low_cny * 2.0, deepseek.high_cny);
        let qwen = crate::usage_prices::estimate("qwen3.8-max", usage).unwrap();
        let expected = (1000.0 * 12.0 + 200.0 * 36.0 + 500.0 * 1.5 + 400.0 * 15.0) / 1_000_000.0;
        assert!((qwen.high_cny - expected).abs() < 1e-9);
        assert_eq!(
            crate::usage_prices::estimate("qwen4-max-preview", usage)
                .unwrap_err()
                .reason,
            crate::usage_prices::Reason::PlanOnly
        );
    }
}
