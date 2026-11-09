use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB rotation threshold
const MAX_MEMORY_ENTRIES: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub component: String,
    pub message: String,
    pub context: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct StructuredLogger {
    log_file: PathBuf,
    rotated_file: PathBuf,
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl StructuredLogger {
    pub fn new(log_file: PathBuf) -> Self {
        let rotated_file = log_file.with_extension("1.log");
        if let Some(parent) = log_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        Self {
            log_file,
            rotated_file,
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_MEMORY_ENTRIES))),
        }
    }

    pub fn log(
        &self,
        level: LogLevel,
        component: &str,
        message: impl Into<String>,
        context: Option<serde_json::Value>,
    ) {
        let entry = LogEntry {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level,
            component: component.to_string(),
            message: message.into(),
            context,
        };

        let mut guard = self.inner.lock();
        if guard.len() >= MAX_MEMORY_ENTRIES {
            guard.pop_front();
        }
        guard.push_back(entry.clone());

        if let Ok(meta) = fs::metadata(&self.log_file) {
            if meta.len() >= MAX_LOG_FILE_BYTES {
                let _ = fs::rename(&self.log_file, &self.rotated_file);
            }
        }

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
        {
            if let Ok(serialized) = serde_json::to_string(&entry) {
                let _ = writeln!(file, "{serialized}");
            }
        }
    }

    pub fn info(&self, component: &str, message: impl Into<String>) {
        self.log(LogLevel::Info, component, message, None);
    }

    pub fn info_ctx(&self, component: &str, message: impl Into<String>, ctx: serde_json::Value) {
        self.log(LogLevel::Info, component, message, Some(ctx));
    }

    pub fn warn(&self, component: &str, message: impl Into<String>) {
        self.log(LogLevel::Warn, component, message, None);
    }

    pub fn warn_ctx(&self, component: &str, message: impl Into<String>, ctx: serde_json::Value) {
        self.log(LogLevel::Warn, component, message, Some(ctx));
    }

    pub fn error(&self, component: &str, message: impl Into<String>) {
        self.log(LogLevel::Error, component, message, None);
    }

    pub fn error_ctx(&self, component: &str, message: impl Into<String>, ctx: serde_json::Value) {
        self.log(LogLevel::Error, component, message, Some(ctx));
    }

    pub fn recent_entries(&self, limit: usize) -> Vec<LogEntry> {
        let guard = self.inner.lock();
        let take_n = limit.min(guard.len());
        guard.iter().rev().take(take_n).cloned().collect()
    }
}
