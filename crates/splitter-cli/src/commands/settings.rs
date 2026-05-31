use splitter_core::{settings_path, FecMode, JitterMode, LogLevel, Settings};

#[allow(clippy::print_stdout)]
pub(crate) fn run_show() -> anyhow::Result<()> {
    let path = settings_path()?;
    let s = Settings::load_or_default(&path)?;
    let raw = toml::to_string_pretty(&s)?;
    print!("{raw}");
    Ok(())
}

#[allow(clippy::print_stdout)]
pub(crate) fn run_get(key: &str) -> anyhow::Result<()> {
    let s = Settings::load_or_default(&settings_path()?)?;
    let value = read_field(&s, key)?;
    println!("{value}");
    Ok(())
}

pub(crate) fn run_set(key: &str, value: &str) -> anyhow::Result<()> {
    let path = settings_path()?;
    let mut s = Settings::load_or_default(&path)?;
    write_field(&mut s, key, value)?;
    s.save_atomic(&path)?;
    Ok(())
}

fn read_field(s: &Settings, key: &str) -> anyhow::Result<String> {
    match key {
        "auto_accept_trusted" => Ok(s.auto_accept_trusted.to_string()),
        "auto_start_with_system" => Ok(s.auto_start_with_system.to_string()),
        "default_bitrate" => Ok(s.default_bitrate.to_string()),
        "fec_mode" => Ok(format!("{:?}", s.fec_mode).to_lowercase()),
        "fec_on_threshold_pct" => Ok(s.fec_on_threshold_pct.to_string()),
        "fec_off_threshold_pct" => Ok(s.fec_off_threshold_pct.to_string()),
        "fec_hysteresis_secs" => Ok(s.fec_hysteresis_secs.to_string()),
        "jitter_mode" => Ok(format_jitter_mode(s.jitter_mode)),
        "jitter_max_depth_ms" => Ok(s.jitter_max_depth_ms.to_string()),
        "log_level" => Ok(format!("{:?}", s.log_level).to_lowercase()),
        "metrics_enabled" => Ok(s.metrics_enabled.to_string()),
        "metrics_port" => Ok(s.metrics_port.to_string()),
        "signaling_port" => Ok(s.signaling_port.to_string()),
        other => anyhow::bail!("unknown settings key: {other}"),
    }
}

fn write_field(s: &mut Settings, key: &str, value: &str) -> anyhow::Result<()> {
    match key {
        "auto_accept_trusted" => s.auto_accept_trusted = value.parse()?,
        "auto_start_with_system" => s.auto_start_with_system = value.parse()?,
        "default_bitrate" => s.default_bitrate = value.parse()?,
        "fec_mode" => s.fec_mode = parse_fec_mode(value)?,
        "fec_on_threshold_pct" => s.fec_on_threshold_pct = value.parse()?,
        "fec_off_threshold_pct" => s.fec_off_threshold_pct = value.parse()?,
        "fec_hysteresis_secs" => s.fec_hysteresis_secs = value.parse()?,
        "jitter_mode" => s.jitter_mode = parse_jitter_mode(value)?,
        "jitter_max_depth_ms" => s.jitter_max_depth_ms = value.parse()?,
        "log_level" => s.log_level = parse_log_level(value)?,
        "metrics_enabled" => s.metrics_enabled = value.parse()?,
        "metrics_port" => s.metrics_port = value.parse()?,
        "signaling_port" => s.signaling_port = value.parse()?,
        other => anyhow::bail!("unknown settings key: {other}"),
    }
    Ok(())
}

fn format_jitter_mode(mode: JitterMode) -> String {
    match mode {
        JitterMode::Auto => "auto".into(),
        JitterMode::Min => "min".into(),
        JitterMode::Fixed(ms) => format!("fixed:{ms}"),
    }
}

fn parse_fec_mode(v: &str) -> anyhow::Result<FecMode> {
    Ok(match v {
        "auto" => FecMode::Auto,
        "always" => FecMode::Always,
        "never" => FecMode::Never,
        _ => anyhow::bail!("fec_mode must be auto|always|never"),
    })
}

fn parse_jitter_mode(v: &str) -> anyhow::Result<JitterMode> {
    if let Some(ms) = v.strip_prefix("fixed:") {
        return Ok(JitterMode::Fixed(ms.parse()?));
    }
    Ok(match v {
        "auto" => JitterMode::Auto,
        "min" => JitterMode::Min,
        _ => anyhow::bail!("jitter_mode must be auto|min|fixed:<ms>"),
    })
}

fn parse_log_level(v: &str) -> anyhow::Result<LogLevel> {
    Ok(match v {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => anyhow::bail!("log_level must be trace|debug|info|warn|error"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_fec_mode() {
        let mut s = Settings::default();
        write_field(&mut s, "fec_mode", "always").unwrap();
        assert_eq!(s.fec_mode, FecMode::Always);
        assert_eq!(read_field(&s, "fec_mode").unwrap(), "always");
    }

    #[test]
    fn unknown_key_errors() {
        let s = Settings::default();
        assert!(read_field(&s, "no_such_field").is_err());
    }

    #[test]
    fn jitter_mode_fixed_parses() {
        let mut s = Settings::default();
        write_field(&mut s, "jitter_mode", "fixed:40").unwrap();
        assert_eq!(s.jitter_mode, JitterMode::Fixed(40));
    }

    #[test]
    fn jitter_mode_fixed_round_trips() {
        let mut s = Settings::default();
        write_field(&mut s, "jitter_mode", "fixed:80").unwrap();
        assert_eq!(read_field(&s, "jitter_mode").unwrap(), "fixed:80");
    }

    #[test]
    fn log_level_round_trip() {
        let mut s = Settings::default();
        write_field(&mut s, "log_level", "debug").unwrap();
        assert_eq!(s.log_level, LogLevel::Debug);
        assert_eq!(read_field(&s, "log_level").unwrap(), "debug");
    }

    #[test]
    fn unknown_fec_mode_errors() {
        let mut s = Settings::default();
        assert!(write_field(&mut s, "fec_mode", "bogus").is_err());
    }

    #[test]
    fn unknown_jitter_mode_errors() {
        let mut s = Settings::default();
        assert!(write_field(&mut s, "jitter_mode", "bogus").is_err());
    }

    #[test]
    fn unknown_log_level_errors() {
        let mut s = Settings::default();
        assert!(write_field(&mut s, "log_level", "verbose").is_err());
    }

    #[test]
    fn signaling_port_round_trip() {
        let mut s = Settings::default();
        write_field(&mut s, "signaling_port", "7001").unwrap();
        assert_eq!(s.signaling_port, 7001);
        assert_eq!(read_field(&s, "signaling_port").unwrap(), "7001");
    }
}
