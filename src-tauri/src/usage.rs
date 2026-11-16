use crate::{
    clients, codex, desktop,
    transaction::Client,
    usage_prices::{self, Estimate, Unpriced},
};
use memchr::memmem::Finder;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[path = "usage_agents.rs"]
mod agents;
#[path = "usage_logs.rs"]
mod logs;

const MAX_LINE: u64 = 2 * 1024 * 1024;
const MAX_CACHED_FILES: usize = 1500;
const MAX_CACHED_RECORDS: usize = 50_000;
const RECORD_MARKERS: &[&[u8]] = &[
    b"\"usage\"",
    b"\"token_count\"",
    b"\"turn_context\"",
    b"\"session_meta\"",
    b"\"inputTokens\"",
    b"\"llm.request\"",
];

#[derive(Clone, Copy, Default, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_long: u64,
}

impl Tokens {
    pub fn input_total(self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
            .saturating_add(self.cache_write_long)
    }

    fn total(self) -> u64 {
        self.input_total().saturating_add(self.output)
    }

    fn add(&mut self, other: Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.cache_write_long = self.cache_write_long.saturating_add(other.cache_write_long);
    }
}

#[derive(Serialize)]
struct Row {
    client: Client,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    tokens: Tokens,
    estimate: Option<Estimate>,
    priced_tokens: u64,
    unpriced: Vec<UnpricedUsage>,
}

#[derive(Serialize)]
struct UnpricedUsage {
    #[serde(flatten)]
    issue: Unpriced,
    tokens: u64,
}

#[derive(Serialize)]
struct Coverage {
    client: Client,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'static str>,
}

#[derive(Serialize)]
pub struct Report {
    app_version: &'static str,
    rows: Vec<Row>,
    coverage: Vec<Coverage>,
    pricing_verified: &'static str,
    pricing_sources: Vec<usage_prices::SourceInfo>,
    exchange_rate_date: &'static str,
    usd_to_cny: f64,
    exchange_rate_source: &'static str,
}

fn roots(client: Client) -> Vec<PathBuf> {
    match client {
        Client::Desktop => desktop::locations()
            .map(|paths| {
                [paths.native, paths.gateway]
                    .into_iter()
                    .flat_map(|path| {
                        [
                            path.join("local-agent-mode-sessions"),
                            path.join("claude-code-sessions"),
                        ]
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Client::Code => clients::code_path()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.join("projects")))
            .into_iter()
            .collect(),
        Client::Codex => codex::path()
            .ok()
            .and_then(|path| {
                path.parent()
                    .map(|dir| vec![dir.join("sessions"), dir.join("archived_sessions")])
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct LogMetadata {
    modified: SystemTime,
    size: u64,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
    #[cfg(windows)]
    created: Option<SystemTime>,
}

impl LogMetadata {
    fn from_metadata(metadata: fs::Metadata) -> std::io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            modified: metadata.modified()?,
            size: metadata.len(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
            #[cfg(windows)]
            created: metadata.created().ok(),
        })
    }
}

fn collect_logs(
    dir: &Path,
    cutoff: SystemTime,
    files: &mut BTreeMap<PathBuf, LogMetadata>,
) -> bool {
    let mut directories = vec![dir.to_path_buf()];
    let mut complete = true;
    while let Some(dir) = directories.pop() {
        let listing = match fs::read_dir(dir) {
            Ok(listing) => listing,
            Err(error) => {
                complete &= error.kind() == std::io::ErrorKind::NotFound;
                continue;
            }
        };
        for item in listing {
            let Ok(item) = item else {
                complete = false;
                continue;
            };
            let Ok(kind) = item.file_type() else {
                complete = false;
                continue;
            };
            let path = item.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                let Ok(meta) = item.metadata().and_then(LogMetadata::from_metadata) else {
                    complete = false;
                    continue;
                };
                if meta.modified >= cutoff {
                    files.insert(path, meta);
                }
            }
        }
    }
    complete
}

fn scan_log_files(
    roots: &[PathBuf],
    cutoff: SystemTime,
    mut parse: impl FnMut(&Path, LogMetadata) -> bool,
) -> bool {
    let mut files = BTreeMap::new();
    let mut complete = true;
    for dir in roots {
        complete &= collect_logs(dir, cutoff, &mut files);
    }
    let mut files: Vec<_> = files.into_iter().collect();
    files.sort_unstable_by_key(|(_, meta)| std::cmp::Reverse(meta.modified));
    for (path, meta) in files {
        complete &= parse(&path, meta);
    }
    complete
}

fn scan_logs(
    roots: &[PathBuf],
    cutoff: SystemTime,
    mut parse: impl FnMut(&Path, u64) -> bool,
) -> bool {
    scan_log_files(roots, cutoff, |path, meta| parse(path, meta.size))
}

struct LogRead {
    complete: bool,
    metadata: Option<LogMetadata>,
}

fn read_records(
    path: &Path,
    size: u64,
    skip_large: impl Fn(&[u8]) -> bool,
    mut consume: impl FnMut(&[u8]),
) -> LogRead {
    let failed = LogRead {
        complete: false,
        metadata: None,
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(path) else {
        return failed;
    };
    let Ok(metadata) = file.metadata() else {
        return failed;
    };
    if !metadata.is_file() {
        return failed;
    }
    let metadata = LogMetadata::from_metadata(metadata).ok();
    let mut reader = BufReader::with_capacity(64 * 1024, file.take(size));
    let mut line = Vec::new();
    let mut complete = true;
    loop {
        line.clear();
        // Most records fit in the buffer, so inspect them without copying.
        let Ok(buffer) = reader.fill_buf() else {
            return failed;
        };
        if buffer.is_empty() {
            let file = reader.get_ref();
            complete &= file.limit() == 0;
            let metadata = metadata.filter(|metadata| {
                file.limit() == 0
                    && file
                        .get_ref()
                        .metadata()
                        .and_then(LogMetadata::from_metadata)
                        .is_ok_and(|current| current == *metadata)
            });
            return LogRead { complete, metadata };
        }
        if let Some(end) = memchr::memchr(b'\n', buffer) {
            let len = end + 1;
            consume(&buffer[..len]);
            reader.consume(len);
            continue;
        }
        if skip_large(buffer) {
            if reader.skip_until(b'\n').is_err() {
                return failed;
            }
            continue;
        }
        match (&mut reader)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(len) if len as u64 > MAX_LINE => {
                if line.last() != Some(&b'\n') && reader.skip_until(b'\n').is_err() {
                    return failed;
                }
                complete &= skip_large(&line);
            }
            Ok(_) => consume(&line),
            Err(_) => return failed,
        }
    }
}

fn records(path: &Path, size: u64, mut consume: impl FnMut(Value)) -> bool {
    let markers: Vec<_> = RECORD_MARKERS.iter().map(Finder::new).collect();
    read_records(
        path,
        size,
        |_| false,
        |line| {
            if markers.iter().any(|marker| marker.find(line).is_some()) {
                if let Ok(record) = serde_json::from_slice(line) {
                    consume(record);
                }
            }
        },
    )
    .complete
}

fn timestamp(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).ok()
}

fn in_range(record: &Value, cutoff: OffsetDateTime) -> bool {
    record["timestamp"]
        .as_str()
        .and_then(timestamp)
        .is_some_and(|time| time >= cutoff)
}

fn safe_model(model: Option<&str>) -> &str {
    model
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 180
                && id.bytes().all(|ch| {
                    ch.is_ascii_alphanumeric()
                        || matches!(ch, b'-' | b'_' | b'.' | b'/' | b':' | b'[' | b']' | b'~')
                })
        })
        .unwrap_or("未知模型")
}

fn number(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

#[derive(Default)]
pub struct UsageCache {
    codex: Mutex<LogCache<logs::CodexUsage>>,
    claude: Mutex<LogCache<logs::ClaudeUsage>>,
}

type LogCache<T> = BTreeMap<PathBuf, CachedLog<T>>;

struct CachedLog<T> {
    metadata: LogMetadata,
    parsed: logs::Parsed<T>,
}

fn scan_cached<T>(
    cache: &Mutex<LogCache<T>>,
    dirs: &[PathBuf],
    cutoff: SystemTime,
    parse: fn(&Path, u64) -> logs::Parsed<T>,
    mut consume: impl FnMut(&[T]),
) -> bool {
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut previous = std::mem::take(&mut *cache);
    let mut remaining = MAX_CACHED_RECORDS;
    let mut keep = |path: PathBuf, entry: CachedLog<T>| {
        let records = entry.parsed.usage.len().max(1);
        if records <= remaining && cache.len() < MAX_CACHED_FILES {
            remaining -= records;
            cache.insert(path, entry);
        }
    };
    // The cache budget limits retained records, never the files included in the report.
    let complete = scan_log_files(dirs, cutoff, |path, metadata| {
        if let Some((path, entry)) = previous
            .remove_entry(path)
            .filter(|(_, entry)| entry.metadata == metadata)
        {
            consume(&entry.parsed.usage);
            let complete = entry.parsed.read.complete;
            keep(path, entry);
            return complete;
        }

        let parsed = parse(path, metadata.size);
        consume(&parsed.usage);
        let complete = parsed.read.complete;
        if parsed.read.metadata == Some(metadata)
            && path.symlink_metadata().is_ok_and(|current| {
                current.is_file()
                    && LogMetadata::from_metadata(current).is_ok_and(|current| current == metadata)
            })
        {
            keep(path.to_path_buf(), CachedLog { metadata, parsed });
        }
        complete
    });
    // Keep other ranges warm within a fixed record budget.
    let mut entries: Vec<_> = previous.into_iter().collect();
    entries.sort_unstable_by_key(|(_, entry)| std::cmp::Reverse(entry.metadata.modified));
    for (path, entry) in entries {
        keep(path, entry);
    }
    complete
}

fn add(rows: &mut BTreeMap<String, Row>, client: Client, model: &str, tokens: Tokens) {
    add_usage(
        rows,
        client,
        model,
        None,
        tokens,
        usage_prices::estimate(model, tokens),
    );
}

fn add_usage(
    rows: &mut BTreeMap<String, Row>,
    client: Client,
    model: &str,
    provider: Option<&str>,
    tokens: Tokens,
    estimate: Result<Estimate, Unpriced>,
) {
    if tokens.total() == 0 {
        return;
    }
    let row = rows
        .entry(format!("{model}\n{}", provider.unwrap_or_default()))
        .or_insert_with(|| Row {
            client,
            model: model.to_owned(),
            provider: provider.map(str::to_owned),
            tokens: Tokens::default(),
            estimate: None,
            priced_tokens: 0,
            unpriced: Vec::new(),
        });
    row.tokens.add(tokens);
    if let Ok(cost) = estimate {
        row.priced_tokens = row.priced_tokens.saturating_add(tokens.total());
        let total = row.estimate.get_or_insert(Estimate {
            low_cny: 0.0,
            high_cny: 0.0,
            ..cost
        });
        total.low_cny += cost.low_cny;
        total.high_cny += cost.high_cny;
    } else if let Err(issue) = estimate {
        if let Some(existing) = row.unpriced.iter_mut().find(|item| item.issue == issue) {
            existing.tokens = existing.tokens.saturating_add(tokens.total());
        } else {
            row.unpriced.push(UnpricedUsage {
                issue,
                tokens: tokens.total(),
            });
        }
    }
}

fn scan_codex(
    cache: &UsageCache,
    dirs: &[PathBuf],
    cutoff: SystemTime,
    rows: &mut BTreeMap<String, Row>,
) -> bool {
    let cutoff_time = OffsetDateTime::from(cutoff);
    let mut seen = HashSet::new();
    scan_cached(&cache.codex, dirs, cutoff, logs::codex, |usage| {
        for record in usage {
            if let Some(id) = &record.id {
                if !seen.insert(id.clone()) {
                    continue;
                }
            }
            if record.time.is_some_and(|time| time >= cutoff_time) {
                add(rows, Client::Codex, &record.model, record.tokens);
            }
        }
    })
}

#[cfg(test)]
pub(crate) fn codex_usage_fixture(dir: &Path) -> Vec<(String, Tokens)> {
    let mut rows = BTreeMap::new();
    assert!(scan_codex(
        &UsageCache::default(),
        &[dir.to_path_buf()],
        SystemTime::UNIX_EPOCH,
        &mut rows,
    ));
    rows.into_values()
        .map(|row| (row.model, row.tokens))
        .collect()
}

pub fn summarize(
    days: u8,
    visible_agents: &[Client],
    cache: &UsageCache,
) -> Result<Report, String> {
    if !matches!(days, 1 | 7 | 30) {
        return Err("请选择最近 24 小时、7 天或 30 天".into());
    }
    let duration = Duration::from_secs(u64::from(days) * 86400);
    let cutoff = SystemTime::now() - duration;
    let cutoff_time = OffsetDateTime::from(cutoff);
    let mut rows = Vec::new();
    let mut coverage = Vec::new();
    let mut seen_claude = HashSet::new();
    let store = crate::secrets::root()
        .ok()
        .and_then(|root| crate::models::read(&root).ok().map(|(_, store)| store));
    for client in Client::ALL {
        if crate::agents::is_added(client) && !visible_agents.contains(&client) {
            continue;
        }
        let mut client_rows = BTreeMap::new();
        if crate::agents::is_added(client) || client == Client::Workbuddy {
            let (state, detail) = agents::scan(
                client,
                cutoff,
                cutoff_time,
                &mut client_rows,
                store.as_ref(),
            );
            coverage.push(Coverage {
                client,
                state,
                detail: Some(detail),
            });
            rows.extend(client_rows.into_values());
            continue;
        }
        let dirs = roots(client);
        let complete = if client == Client::Codex {
            scan_codex(cache, &dirs, cutoff, &mut client_rows)
        } else {
            let mut by_message = HashMap::<String, (String, Tokens)>::new();
            let complete = scan_cached(&cache.claude, &dirs, cutoff, logs::claude, |usage| {
                for record in usage {
                    if record.time >= cutoff_time && !seen_claude.contains(&record.id) {
                        let item = by_message
                            .entry(record.id.clone())
                            .or_insert_with(|| (record.model.clone(), record.tokens));
                        if record.tokens.total() > item.1.total() {
                            *item = (record.model.clone(), record.tokens);
                        }
                    }
                }
            });
            for (id, (model, tokens)) in by_message {
                seen_claude.insert(id);
                add(&mut client_rows, client, &model, tokens);
            }
            complete
        };
        let state = if !complete {
            "partial"
        } else if client_rows.is_empty() {
            "empty"
        } else {
            "available"
        };
        coverage.push(Coverage {
            client,
            state,
            detail: None,
        });
        rows.extend(client_rows.into_values());
    }
    rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.tokens.total()));
    let pricing_sources = rows
        .iter()
        .flat_map(|row| {
            row.estimate
                .map(|price| price.source)
                .into_iter()
                .chain(row.unpriced.iter().filter_map(|item| item.issue.source))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|source| source.info())
        .collect();
    Ok(Report {
        app_version: env!("CARGO_PKG_VERSION"),
        rows,
        coverage,
        pricing_verified: usage_prices::VERIFIED,
        pricing_sources,
        exchange_rate_date: usage_prices::EXCHANGE_RATE_DATE,
        usd_to_cny: usage_prices::USD_TO_CNY,
        exchange_rate_source: usage_prices::EXCHANGE_RATE_SOURCE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pricing_coverage_preserves_unpriced_usage_even_in_a_partly_priced_row() {
        let mut rows = BTreeMap::new();
        let tokens = Tokens {
            input: 1000,
            output: 100,
            ..Tokens::default()
        };
        let price = usage_prices::estimate("glm-5.3", tokens);
        for quote in [
            price,
            Err(Unpriced::provider_unknown()),
            Err(Unpriced::provider_unknown()),
        ] {
            add_usage(&mut rows, Client::Desktop, "glm-5.3", None, tokens, quote);
        }
        let row = &rows["glm-5.3\n"];
        assert_eq!(row.tokens.total(), 3300);
        assert_eq!(row.priced_tokens, 1100);
        assert_eq!(row.unpriced.len(), 1);
        assert_eq!(row.unpriced[0].tokens, 2200);
        assert!((row.estimate.unwrap().high_cny - 0.0108).abs() < 1e-9);

        for model in ["qwen4.0-max-preview", "unreleased-model"] {
            add(&mut rows, Client::Desktop, model, tokens);
            let row = &rows[&format!("{model}\n")];
            assert_eq!(row.tokens.total(), 1100);
            assert_eq!(row.priced_tokens, 0);
            assert!(row.estimate.is_none());
            assert_eq!(row.unpriced[0].tokens, 1100);
        }
        assert_eq!(
            rows["qwen4.0-max-preview\n"].unpriced[0].issue.reason,
            usage_prices::Reason::PlanOnly
        );
        assert_eq!(
            rows["unreleased-model\n"].unpriced[0].issue.reason,
            usage_prices::Reason::MissingPrice
        );
    }

    fn read_test_log(path: &Path, size: u64) -> logs::Parsed<Vec<u8>> {
        let mut usage = Vec::new();
        let read = read_records(path, size, |_| false, |line| usage.push(line.to_vec()));
        logs::Parsed { usage, read }
    }

    #[test]
    fn scans_deep_logs_and_all_files_even_with_large_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut nested = root.to_path_buf();
        for _ in 0..9 {
            nested.push("subagents");
        }
        fs::create_dir_all(&nested).unwrap();
        let deep = nested.join("nested-session.jsonl");
        fs::write(&deep, b"usage\n").unwrap();
        let large = root.join("large-session.jsonl");
        fs::File::create(&large)
            .unwrap()
            .set_len(256 * 1024 * 1024 + 1)
            .unwrap();
        for index in 0..1_500 {
            fs::File::create(root.join(format!("session-{index}.jsonl"))).unwrap();
        }

        let mut scanned = HashSet::new();
        assert!(scan_logs(
            &[root.to_path_buf(), nested],
            SystemTime::UNIX_EPOCH,
            |path, _| {
                assert!(scanned.insert(path.to_path_buf()));
                true
            }
        ));
        assert_eq!(scanned.len(), 1_502);
        assert!(scanned.contains(&deep));
        assert!(scanned.contains(&large));
    }

    #[test]
    fn cached_log_tracks_file_changes_and_time_ranges() {
        let temp = tempfile::tempdir().unwrap();
        let roots = [temp.path().to_path_buf()];
        let path = temp.path().join("session.jsonl");
        fs::write(&path, b"first\n").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let cache = Mutex::new(LogCache::new());
        let scan = |cutoff| {
            let mut usage = Vec::new();
            assert!(scan_cached(
                &cache,
                &roots,
                cutoff,
                read_test_log,
                |records| usage.extend_from_slice(records)
            ));
            usage
        };

        assert_eq!(scan(SystemTime::UNIX_EPOCH), [b"first\n".to_vec()]);
        assert!(scan(modified + Duration::from_secs(1)).is_empty());
        assert_eq!(cache.lock().unwrap().len(), 1);
        assert_eq!(scan(SystemTime::UNIX_EPOCH), [b"first\n".to_vec()]);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        assert_eq!(
            scan(SystemTime::UNIX_EPOCH),
            [b"first\n".to_vec(), b"second\n".to_vec()]
        );

        let replacement = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
        fs::write(replacement.path(), b"other\nrecord\n").unwrap();
        replacement
            .as_file()
            .set_modified(fs::metadata(&path).unwrap().modified().unwrap())
            .unwrap();
        replacement.persist(&path).unwrap();
        assert_eq!(
            scan(SystemTime::UNIX_EPOCH),
            [b"other\n".to_vec(), b"record\n".to_vec()]
        );
        fs::remove_file(&path).unwrap();
        assert!(scan(SystemTime::UNIX_EPOCH).is_empty());
    }

    #[test]
    fn oversized_log_lines_keep_partial_status_when_cached() {
        let temp = tempfile::tempdir().unwrap();
        let roots = [temp.path().to_path_buf()];
        let path = temp.path().join("session.jsonl");
        let mut bytes = vec![b'x'; MAX_LINE as usize];
        bytes.extend_from_slice(b"\nnext\nlast");
        fs::write(&path, bytes).unwrap();
        let cache = Mutex::new(LogCache::new());

        for _ in 0..2 {
            let mut usage = Vec::new();
            let complete = scan_cached(
                &cache,
                &roots,
                SystemTime::UNIX_EPOCH,
                read_test_log,
                |records| usage.extend_from_slice(records),
            );
            assert!(!complete);
            assert_eq!(usage, [b"next\n".to_vec(), b"last".to_vec()]);
            assert_eq!(cache.lock().unwrap().len(), 1);
        }
    }
}
