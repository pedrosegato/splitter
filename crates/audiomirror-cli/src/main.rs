use clap::{Parser, Subcommand};

#[derive(Subcommand, Debug)]
enum AutostartAction {
    /// Register the daemon as a login/startup item for the current user.
    Enable,
    /// Remove the daemon from login/startup items.
    Disable,
    /// Show whether the autostart artifact is present.
    Status,
}

#[derive(Subcommand, Debug)]
enum LogsAction {
    /// Print the path of the current log file.
    Path,
    /// Tail the current log file (poll every 200 ms).
    Tail,
}

#[derive(Subcommand, Debug)]
enum SettingsAction {
    /// Print all settings as TOML.
    Show,
    /// Print the value of a single settings key.
    Get { key: String },
    /// Set the value of a single settings key.
    Set { key: String, value: String },
}

mod commands;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum Source {
    Mic,
    System,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum SendFecMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum CliJitterMode {
    Auto,
    Min,
}

impl From<CliJitterMode> for audiomirror_core::JitterMode {
    fn from(m: CliJitterMode) -> Self {
        match m {
            CliJitterMode::Auto => audiomirror_core::JitterMode::Auto,
            CliJitterMode::Min => audiomirror_core::JitterMode::Min,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "audiomirror-cli", version, about = "AudioMirror Phase 1 CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Devices,

    Send {
        #[arg(long)]
        input: String,
        #[arg(long)]
        addr: String,
        #[arg(long, default_value_t = 0)]
        stream_id: u8,
        #[arg(long, default_value_t = 64_000)]
        bitrate: i32,
        #[arg(long, value_enum, default_value_t = Source::Mic)]
        source: Source,
        #[arg(long, value_enum, default_value_t = SendFecMode::Auto)]
        fec_mode: SendFecMode,
        #[arg(long, default_value_t = 0)]
        simulated_loss_pct: u8,
    },

    Recv {
        #[arg(long)]
        output: String,
        #[arg(long)]
        bind: String,
        #[arg(long, value_enum, default_value_t = CliJitterMode::Auto)]
        jitter_mode: CliJitterMode,
        #[arg(long, default_value_t = 100)]
        jitter_max_depth_ms: u32,
    },

    Loop {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long, default_value_t = 64_000)]
        bitrate: i32,
        #[arg(long, value_enum, default_value_t = Source::Mic)]
        source: Source,
        #[arg(long, value_enum, default_value_t = SendFecMode::Auto)]
        fec_mode: SendFecMode,
        #[arg(long, default_value_t = 0)]
        simulated_loss_pct: u8,
    },

    Discover {
        #[arg(long, default_value_t = 5)]
        duration_secs: u64,
        #[arg(long, default_value_t = 7_000)]
        signaling_port: u16,
    },

    Daemon {
        #[arg(long, default_value_t = 7_000)]
        signaling_port: u16,
        #[arg(long)]
        peer_name: Option<String>,
    },

    /// Inspect application logs.
    Logs {
        #[command(subcommand)]
        action: LogsAction,
    },

    /// Read or write application settings.
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },

    /// Manage autostart (login item / systemd user service / Windows Run key).
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let logs_dir = audiomirror_core::log_dir()?;
    let _logs_guard =
        audiomirror_core::observability::logs::init(audiomirror_core::LogLevel::Info, &logs_dir)?;
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Devices => commands::devices::run(),
        Cmd::Send {
            input,
            addr,
            stream_id,
            bitrate,
            source,
            fec_mode,
            simulated_loss_pct,
        } => {
            commands::send::run(
                &input,
                &addr,
                stream_id,
                bitrate,
                source,
                fec_mode,
                simulated_loss_pct,
            )
            .await
        }
        Cmd::Recv {
            output,
            bind,
            jitter_mode,
            jitter_max_depth_ms,
        } => {
            commands::recv::run_with_settings(
                &output,
                &bind,
                jitter_mode.into(),
                jitter_max_depth_ms,
            )
            .await
        }
        Cmd::Loop {
            input,
            output,
            bitrate,
            source,
            fec_mode,
            simulated_loss_pct,
        } => {
            commands::loop_cmd::run(
                &input,
                &output,
                bitrate,
                source,
                fec_mode,
                simulated_loss_pct,
            )
            .await
        }
        Cmd::Discover {
            duration_secs,
            signaling_port,
        } => commands::discover::run(duration_secs, signaling_port).await,
        Cmd::Daemon {
            signaling_port,
            peer_name,
        } => commands::daemon::run(signaling_port, peer_name).await,
        Cmd::Logs { action } => match action {
            LogsAction::Path => commands::logs::run_path().await,
            LogsAction::Tail => commands::logs::run_tail().await,
        },
        Cmd::Settings { action } => match action {
            SettingsAction::Show => commands::settings::run_show(),
            SettingsAction::Get { key } => commands::settings::run_get(&key),
            SettingsAction::Set { key, value } => commands::settings::run_set(&key, &value),
        },
        Cmd::Autostart { action } => match action {
            AutostartAction::Enable => commands::autostart::run_enable(),
            AutostartAction::Disable => commands::autostart::run_disable(),
            AutostartAction::Status => commands::autostart::run_status(),
        },
    }
}
