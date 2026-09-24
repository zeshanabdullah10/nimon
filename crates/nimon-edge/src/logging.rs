//! Logging setup for the edge binary
//!
//! `logging.level` is an [`EnvFilter`] directive (plain levels like `info`
//! work); `RUST_LOG` overrides it. `logging.file` enables a daily-rotated
//! file (7 files kept) written by a background thread so logging never
//! blocks the actor thread; `logging.stdout` controls console output.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

use crate::config::LoggingConfig;

/// Number of rotated log files kept
const MAX_LOG_FILES: usize = 7;

/// Build the level filter: `RUST_LOG` when set, else `logging.level`,
/// else `info`.
pub fn build_filter(level: &str, rust_log: Option<&str>) -> EnvFilter {
    if let Some(directive) = rust_log.filter(|s| !s.trim().is_empty()) {
        if let Ok(filter) = EnvFilter::try_new(directive) {
            return filter;
        }
    }
    EnvFilter::try_new(level.trim()).unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Install the global subscriber. Keep the returned guard alive for the
/// process lifetime (dropping it flushes and stops the file writer).
///
/// Errors (e.g. an unwritable log directory) are reported on stderr and
/// logging falls back to stdout.
pub fn init(config: &LoggingConfig) -> Option<WorkerGuard> {
    let rust_log = std::env::var("RUST_LOG").ok();
    let filter = build_filter(&config.level, rust_log.as_deref());

    let (file_layer, guard) = match open_file_appender(&config.file) {
        Ok(Some(appender)) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer().with_ansi(false).with_writer(writer).boxed();
            (Some(layer), Some(guard))
        }
        Ok(None) => (None, None),
        Err(e) => {
            eprintln!("nimon-edge: cannot open log file '{}': {e}", config.file);
            (None, None)
        }
    };
    let stdout_layer = (config.stdout || file_layer.is_none()).then(|| fmt::layer().boxed());

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stdout_layer)
        .try_init();
    guard
}

fn open_file_appender(file: &str) -> Result<Option<RollingFileAppender>, String> {
    let file = file.trim();
    if file.is_empty() {
        return Ok(None);
    }
    let path = Path::new(file);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid file name".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(name)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_uses_config_level() {
        assert_eq!(build_filter("debug", None).to_string(), "debug");
        let f = build_filter("warn,nimon_edge=debug", None).to_string();
        assert!(f.contains("warn") && f.contains("nimon_edge=debug"), "{f}");
    }

    #[test]
    fn test_rust_log_overrides() {
        assert_eq!(build_filter("info", Some("trace")).to_string(), "trace");
        // empty RUST_LOG falls back to the config
        assert_eq!(build_filter("warn", Some("  ")).to_string(), "warn");
    }

    #[test]
    fn test_invalid_level_falls_back_to_info() {
        assert_eq!(build_filter("not a [level", None).to_string(), "info");
    }

    #[test]
    fn test_file_appender_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("edge.log");
        let appender = open_file_appender(file.to_str().unwrap()).unwrap();
        assert!(appender.is_some());
        assert!(dir.path().join("sub").is_dir());
        assert!(open_file_appender("  ").unwrap().is_none());
    }
}
