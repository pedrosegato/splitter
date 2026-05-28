use audiomirror_core::{settings_path, Settings};

#[allow(clippy::print_stdout)]
pub(crate) fn run_enable() -> anyhow::Result<()> {
    flip(true)?;
    println!("metrics enabled (restart daemon to apply)");
    Ok(())
}

#[allow(clippy::print_stdout)]
pub(crate) fn run_disable() -> anyhow::Result<()> {
    flip(false)?;
    println!("metrics disabled (restart daemon to apply)");
    Ok(())
}

#[allow(clippy::print_stdout)]
pub(crate) async fn run_status() -> anyhow::Result<()> {
    let s = Settings::load_or_default(&settings_path()?)?;
    let reach = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        tokio::net::TcpStream::connect(("127.0.0.1", s.metrics_port)),
    )
    .await;
    let live = matches!(reach, Ok(Ok(_)));
    println!(
        "metrics_enabled: {}  port: {}  endpoint_live: {}",
        s.metrics_enabled, s.metrics_port, live
    );
    Ok(())
}

fn flip(enabled: bool) -> anyhow::Result<()> {
    let path = settings_path()?;
    let mut s = Settings::load_or_default(&path)?;
    s.metrics_enabled = enabled;
    s.save_atomic(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Create a unique temp path for settings that does NOT exist yet.
    fn temp_settings_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("am_metrics_test_{tag}_{}.toml", std::process::id()))
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn flip_enable_persists_to_toml() {
        let path = temp_settings_path("enable");
        let s = Settings {
            metrics_enabled: true,
            ..Settings::default()
        };
        assert!(
            !Settings::default().metrics_enabled,
            "default should be disabled"
        );
        s.save_atomic(&path).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert!(loaded.metrics_enabled);
        cleanup(&path);
    }

    #[test]
    fn flip_disable_persists_to_toml() {
        let path = temp_settings_path("disable");
        let s = Settings {
            metrics_enabled: true,
            ..Settings::default()
        };
        s.save_atomic(&path).unwrap();

        let mut s2 = Settings::load_or_default(&path).unwrap();
        s2.metrics_enabled = false;
        s2.save_atomic(&path).unwrap();

        let loaded = Settings::load_or_default(&path).unwrap();
        assert!(!loaded.metrics_enabled);
        cleanup(&path);
    }

    #[test]
    fn flip_enable_disable_toggle() {
        let path = temp_settings_path("toggle");
        // start disabled
        let s = Settings::default();
        s.save_atomic(&path).unwrap();

        // enable
        let mut s = Settings::load_or_default(&path).unwrap();
        s.metrics_enabled = true;
        s.save_atomic(&path).unwrap();
        assert!(Settings::load_or_default(&path).unwrap().metrics_enabled);

        // disable
        let mut s = Settings::load_or_default(&path).unwrap();
        s.metrics_enabled = false;
        s.save_atomic(&path).unwrap();
        assert!(!Settings::load_or_default(&path).unwrap().metrics_enabled);
        cleanup(&path);
    }

    #[tokio::test]
    async fn status_endpoint_live_false_when_nothing_running() {
        // Connect to a port where nothing is listening — endpoint_live must be false.
        let port: u16 = 19877; // unlikely to be in use during tests
        let reach = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await;
        let live = matches!(reach, Ok(Ok(_)));
        assert!(!live, "port {port} should not be live");
    }
}
