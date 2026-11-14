//! Bounded, shared diagnostics available before the application state is opened.
use claudekeeper_net::redact;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_LOG_BYTES: u64 = 1_048_576;
pub const LOG_ARCHIVES: usize = 2;
pub const MAX_RECENT_LINES: usize = 1000;
const MAX_MESSAGE_BYTES: usize = 8192;
static GLOBAL: OnceLock<Arc<DiagnosticLog>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    #[serde(default)]
    pub timestamp_ms: u64,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub source: String,
    pub level: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<LogContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogContext {
    pub task_id: String,
    pub stage: String,
    pub code: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogInfo {
    pub directory: String,
    pub log_path: String,
    pub session_id: String,
    pub recent_lines: usize,
    pub max_file_bytes: u64,
    pub retained_files: usize,
    pub write_error: Option<String>,
}

struct Journal {
    lines: VecDeque<LogLine>,
    write_error: Option<String>,
}

pub struct DiagnosticLog {
    directory: PathBuf,
    session_id: String,
    journal: Mutex<Journal>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit { return text.to_string(); }
    let mut end = limit;
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}…[已截断]", &text[..end])
}

fn clean_context(mut context: LogContext) -> LogContext {
    context.task_id = bounded(&redact(&context.task_id, &[]), 100);
    context.stage = bounded(&redact(&context.stage, &[]), 240);
    context.code = context.code.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(80).collect();
    context
}

impl DiagnosticLog {
    pub fn open(directory: PathBuf) -> Arc<Self> {
        let error = fs::create_dir_all(&directory).err().map(|err| format!("无法创建日志目录：{err}"));
        let mut lines = VecDeque::new();
        for index in (0..=LOG_ARCHIVES).rev() {
            let name = if index == 0 { "logs.jsonl".into() } else { format!("logs.{index}.jsonl") };
            let Ok(file) = fs::File::open(directory.join(name)) else { continue };
            // Old or externally modified files must not cause an unbounded allocation on startup.
            let reader = BufReader::new(file.take(MAX_LOG_BYTES + 65_536));
            for line in reader.split(b'\n').map_while(Result::ok) {
                if line.len() > MAX_MESSAGE_BYTES * 4 { continue; }
                if let Ok(mut line) = serde_json::from_slice::<LogLine>(&line) {
                    line.message = bounded(&redact(&line.message, &[]), MAX_MESSAGE_BYTES);
                    line.context = line.context.map(clean_context);
                    lines.push_back(line);
                    if lines.len() > MAX_RECENT_LINES { lines.pop_front(); }
                }
            }
        }
        Arc::new(Self {
            directory,
            session_id: format!("{}-{}", now_ms(), std::process::id()),
            journal: Mutex::new(Journal { lines, write_error: error }),
        })
    }

    pub fn for_directory(directory: &Path) -> Arc<Self> {
        if let Some(log) = GLOBAL.get().filter(|log| log.directory == directory) { return log.clone(); }
        Self::open(directory.to_path_buf())
    }

    pub fn write(&self, level: &str, source: &str, message: &str) -> LogLine {
        self.write_context(level, source, message, None)
    }

    pub fn write_context(&self, level: &str, source: &str, message: &str, context: Option<LogContext>) -> LogLine {
        // Redact before clipping, including credentials which straddle the size boundary.
        let line = LogLine {
            timestamp_ms: now_ms(),
            session_id: self.session_id.clone(),
            source: bounded(source, 80),
            level: match level { "error" => "error", "warn" => "warn", "debug" => "debug", _ => "info" }.into(),
            message: bounded(&redact(message, &[]), MAX_MESSAGE_BYTES),
            context: context.map(clean_context),
        };
        let mut journal = self.journal.lock().unwrap_or_else(|err| err.into_inner());
        journal.lines.push_back(line.clone());
        if journal.lines.len() > MAX_RECENT_LINES { journal.lines.pop_front(); }
        // Serialize appending and rotation under one lock, including startup/process events.
        journal.write_error = self.append(&line).err();
        line
    }

    fn append(&self, line: &LogLine) -> Result<(), String> {
        let mut text = serde_json::to_vec(line).map_err(|err| err.to_string())?;
        text.push(b'\n');
        let path = self.directory.join("logs.jsonl");
        if fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) + text.len() as u64 > MAX_LOG_BYTES {
            let oldest = self.directory.join(format!("logs.{LOG_ARCHIVES}.jsonl"));
            if oldest.exists() { fs::remove_file(oldest).map_err(|err| err.to_string())?; }
            for index in (1..LOG_ARCHIVES).rev() {
                let from = self.directory.join(format!("logs.{index}.jsonl"));
                if from.exists() { fs::rename(from, self.directory.join(format!("logs.{}.jsonl", index + 1))).map_err(|err| err.to_string())?; }
            }
            if path.exists() { fs::rename(&path, self.directory.join("logs.1.jsonl")).map_err(|err| err.to_string())?; }
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(path).map_err(|err| err.to_string())?;
        file.write_all(&text).map_err(|err| err.to_string())
    }

    pub fn lines(&self) -> Vec<LogLine> {
        self.journal.lock().unwrap_or_else(|err| err.into_inner()).lines.iter().cloned().collect()
    }

    pub fn info(&self) -> LogInfo {
        let journal = self.journal.lock().unwrap_or_else(|err| err.into_inner());
        LogInfo {
            directory: self.directory.display().to_string(),
            log_path: self.directory.join("logs.jsonl").display().to_string(),
            session_id: self.session_id.clone(),
            recent_lines: journal.lines.len(),
            max_file_bytes: MAX_LOG_BYTES,
            retained_files: LOG_ARCHIVES + 1,
            write_error: journal.write_error.clone(),
        }
    }
}

pub fn initialize(directory: PathBuf) -> Arc<DiagnosticLog> {
    GLOBAL.get_or_init(|| {
        let log = DiagnosticLog::open(directory);
        if log.info().write_error.is_some() {
            let fallback = DiagnosticLog::open(std::env::temp_dir().join("ClaudeKeeper-diagnostics"));
            fallback.write("warn", "startup", "应用数据目录无法写入，启动日志改存临时目录");
            fallback
        } else { log }
    }).clone()
}

pub fn record(level: &str, source: &str, message: &str) {
    if let Some(log) = GLOBAL.get() { log.write(level, source, message); }
}

pub fn record_context(level: &str, source: &str, message: &str, context: LogContext) {
    if let Some(log) = GLOBAL.get() { log.write_context(level, source, message, Some(context)); }
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Payloads can contain configuration/credentials. Keep location and stack, never payload.
        let message = format!("线程异常：{}\n{}", info.location().map(|loc| loc.to_string()).unwrap_or_default(), std::backtrace::Backtrace::force_capture());
        record("error", "panic", &message);
        eprintln!("ClaudeKeeper 发生异常，位置已写入运行日志");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ck-journal-{name}-{}-{}", std::process::id(), now_ms()))
    }

    #[test]
    fn legacy_lines_and_startup_events_survive_restart_with_context() {
        let dir = temp("restart");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("logs.jsonl"), "{\"level\":\"warn\",\"message\":\"旧版记录\"}\npartial\n").unwrap();
        let log = DiagnosticLog::open(dir.clone());
        let line = log.write("error", "startup", "启动失败 Authorization: Bearer secret-token");
        assert!(line.timestamp_ms > 0);
        assert!(!line.session_id.is_empty());
        assert!(!line.message.contains("secret-token"));
        let again = DiagnosticLog::open(dir.clone());
        assert_eq!(again.lines().len(), 2);
        assert_eq!(again.lines()[0].timestamp_ms, 0);
        assert_eq!(again.lines()[1], line);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn structured_failure_context_survives_restart_without_credentials() {
        let dir = temp("context"); let log = DiagnosticLog::open(dir.clone());
        let line = log.write_context("error", "network", "连接失败 password=hidden-password", Some(LogContext { task_id:"op-1-2".into(), stage:"阶段 https://host.invalid/?token=private-value".into(), code:"NET_INGRESS_PROBE".into(), elapsed_ms:1250 }));
        let reopened = DiagnosticLog::open(dir.clone());
        assert_eq!(reopened.lines().last(), Some(&line));
        let data = fs::read_to_string(dir.join("logs.jsonl")).unwrap();
        assert!(data.contains("NET_INGRESS_PROBE")); assert!(data.contains("1250"));
        assert!(!data.contains("hidden-password")); assert!(!data.contains("private-value"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_writes_rotate_and_remain_valid_bounded_json() {
        let dir = temp("rotation");
        let log = DiagnosticLog::open(dir.clone());
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let log = log.clone();
                scope.spawn(move || { for n in 0..130 { log.write("info", "test", &format!("{worker}:{n} {}", "中".repeat(2500))); } });
            }
        });
        let mut total = 0;
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.metadata().unwrap().len() <= MAX_LOG_BYTES);
            for line in fs::read_to_string(entry.path()).unwrap().lines() {
                let _: LogLine = serde_json::from_str(line).unwrap();
                total += 1;
            }
        }
        assert!(dir.join("logs.2.jsonl").exists());
        assert!(total > 0 && total < 520);
        assert!(DiagnosticLog::open(dir.clone()).lines().len() <= MAX_RECENT_LINES);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn disk_failures_are_visible_and_do_not_drop_memory_events() {
        let dir = temp("failure");
        fs::write(&dir, "not a directory").unwrap();
        let log = DiagnosticLog::open(dir.clone());
        log.write("error", "test", "still visible");
        assert!(log.info().write_error.is_some());
        assert_eq!(log.lines().len(), 1);
        fs::remove_file(dir).unwrap();
    }

    #[test]
    fn panic_is_persisted_without_logging_the_payload() {
        const ENV: &str = "CK_DIAGNOSTICS_PANIC_TEST";
        if let Some(directory) = std::env::var_os(ENV) {
            initialize(PathBuf::from(directory));
            install_panic_hook();
            panic!("private-panic-payload-should-never-be-logged");
        }
        let dir = temp("panic");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "diagnostics::tests::panic_is_persisted_without_logging_the_payload", "--nocapture"])
            .env(ENV, &dir).output().unwrap();
        assert!(!output.status.success());
        let log = DiagnosticLog::open(dir.clone());
        assert!(log.lines().iter().any(|line| line.source == "panic" && line.level == "error"));
        for text in [fs::read_to_string(dir.join("logs.jsonl")).unwrap(), String::from_utf8_lossy(&output.stdout).to_string(), String::from_utf8_lossy(&output.stderr).to_string()] {
            assert!(!text.contains("private-panic-payload-should-never-be-logged"));
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
