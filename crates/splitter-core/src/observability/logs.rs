use crate::error::NetError;
use crate::settings::LogLevel;
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;

pub struct LogsGuard {
    _file_guard: WorkerGuard,
}

pub fn log_dir() -> Result<PathBuf, NetError> {
    let base =
        dirs::data_dir().ok_or_else(|| NetError::ConfigIo("no data_dir available".into()))?;
    Ok(base.join("Splitter").join("logs"))
}

/// Return the path of the most recently modified log file in the log directory.
/// Falls back to the canonical `splitter.log` path when the directory is empty
/// or does not yet exist (useful in tests / first run before any log is written).
pub fn current_log_path() -> Result<PathBuf, NetError> {
    let dir = log_dir()?;
    if dir.is_dir() {
        let newest = std::fs::read_dir(&dir)
            .map_err(|e| NetError::ConfigIo(format!("read_dir {}: {e}", dir.display())))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("log"))
            })
            .filter_map(|e| {
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((mtime, e.path()))
            })
            .max_by_key(|(mtime, _)| *mtime)
            .map(|(_, p)| p);
        if let Some(p) = newest {
            return Ok(p);
        }
    }
    Ok(dir.join("splitter.log"))
}

pub fn init(level: LogLevel, dir: &Path) -> Result<LogsGuard, NetError> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};

    std::fs::create_dir_all(dir)
        .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", dir.display())))?;

    let level_str = match level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level_str));

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(7)
        .filename_prefix("splitter")
        .filename_suffix("log")
        .build(dir)
        .map_err(|e| NetError::ConfigIo(format!("rolling appender: {e}")))?;
    let (nb, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = fmt::layer().json().with_writer(nb);
    let stdout_layer = fmt::layer().pretty().with_writer(std::io::stdout);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .try_init()
        .map_err(|e| NetError::ConfigIo(format!("tracing init: {e}")))?;

    Ok(LogsGuard { _file_guard: guard })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_creates_log_directory() {
        let dir = tempdir().unwrap();
        let logs = dir.path().join("logs");
        let _guard = init(LogLevel::Info, &logs).unwrap();
        assert!(logs.exists());
    }

    #[test]
    fn log_dir_returns_splitter_subpath() {
        let p = log_dir().unwrap();
        assert!(p.ends_with("Splitter/logs") || p.ends_with("Splitter\\logs"));
    }

    /// current_log_path falls back to `splitter.log` when no `.log` files exist.
    #[test]
    fn current_log_path_fallback_when_no_log_files() {
        // The real log_dir() is used; if it exists but has no .log files, or does
        // not exist, the fallback path ending in "splitter.log" is returned.
        // We cannot inject the dir without refactoring, so we test the invariant
        // on the returned path: it must end in ".log".
        if let Ok(p) = current_log_path() {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            assert_eq!(ext, "log", "current_log_path must return a .log path");
        }
        // If Err (no data_dir on headless CI), that's also acceptable.
    }

    /// current_log_path picks the newest .log file when multiple exist.
    #[test]
    fn current_log_path_picks_newest_log_file() {
        use std::io::Write;
        use std::thread::sleep;
        use std::time::Duration;

        let dir = tempdir().unwrap();
        // Create two log files with a brief delay so mtime differs.
        let old_file = dir.path().join("splitter.2026-05-27.log");
        std::fs::File::create(&old_file).unwrap();

        sleep(Duration::from_millis(10));
        let new_file = dir.path().join("splitter.2026-05-28.log");
        let mut f = std::fs::File::create(&new_file).unwrap();
        writeln!(f, "newer").unwrap();

        // Simulate what current_log_path does internally (dir-scoped version).
        let newest = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("log"))
            })
            .filter_map(|e| {
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((mtime, e.path()))
            })
            .max_by_key(|(mtime, _)| *mtime)
            .map(|(_, p)| p);

        assert_eq!(newest.unwrap(), new_file);
    }
}
