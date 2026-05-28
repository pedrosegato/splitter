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
    Ok(base.join("AudioMirror").join("logs"))
}

pub fn current_log_path() -> Result<PathBuf, NetError> {
    Ok(log_dir()?.join("audiomirror.log"))
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
        .filename_prefix("audiomirror")
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
    fn log_dir_returns_audiomirror_subpath() {
        let p = log_dir().unwrap();
        assert!(p.ends_with("AudioMirror/logs") || p.ends_with("AudioMirror\\logs"));
    }
}
