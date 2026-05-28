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
enum MetricsAction {
    /// Enable the Prometheus /metrics endpoint (restart daemon to apply).
    Enable,
    /// Disable the Prometheus /metrics endpoint (restart daemon to apply).
    Disable,
    /// Show metrics_enabled setting and whether the endpoint is currently reachable.
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

#[derive(Subcommand, Debug)]
enum StreamAction {
    /// Open an audio stream to a remote peer device.
    ///
    /// Example:
    ///   audiomirror-cli stream open --from in:0 --to peer:out:0 --session <UUID>
    #[command(
        long_about = "Open a peer-to-peer audio stream within an existing session.\n\
        \n\
        Requires a running daemon (`audiomirror-cli daemon`).  The source peer encodes audio\n\
        from the --from device with Opus at the chosen bitrate and sends it over UDP to the\n\
        --to device on the remote peer.  Obtain the session UUID from the daemon REPL `sessions`\n\
        command after running `connect` + `accept` + `open`.\n\
        \n\
        Device syntax:\n\
          --from in:<index>              local input device by index\n\
          --to   <peer-name>:out:<index> remote output device on the named peer\n\
        \n\
        Examples:\n\
          # Inside the daemon REPL — stream mic to remote speaker\n\
          stream open --from in:0 --to bob:out:1 --session e5f6a7b8-...\n\
          \n\
          # Higher bitrate for music\n\
          stream open --from in:0 --to bob:out:1 --session e5f6a7b8-... --bitrate 96000"
    )]
    Open {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, default_value_t = 64_000)]
        bitrate: i32,
    },
}

#[derive(Parser, Debug)]
#[command(name = "audiomirror-cli", version, about = "AudioMirror CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List available audio input and output devices.
    #[command(
        long_about = "Enumerate all audio input and output devices available on the host\n\
        and print them to stdout with their index and display name.\n\
        \n\
        Use the reported indices or names as arguments to --input / --output / --from / --to\n\
        in other subcommands.\n\
        \n\
        Example:\n\
          audiomirror-cli devices"
    )]
    Devices,

    /// Capture audio from a local device and send it over UDP as Opus packets.
    ///
    /// Example:
    ///   audiomirror-cli send --input <device-id> --addr 192.168.1.50:5004
    #[command(
        long_about = "Capture audio from a local input device, encode with Opus, and\n\
        transmit UDP packets to the specified address. Runs until interrupted (Ctrl-C).\n\
        \n\
        Use --source system to capture desktop/loopback audio instead of a microphone:\n\
          macOS  — route system output through BlackHole 2ch, then pass BlackHole as --input\n\
          Windows — WASAPI loopback is selected automatically when --source system is set\n\
          Linux   — pass a .monitor PulseAudio/PipeWire source as --input\n\
        \n\
        Examples:\n\
          # Send microphone to a remote host\n\
          audiomirror-cli send --input \"Built-in Microphone\" --addr 192.168.1.50:5004\n\
          \n\
          # Send system audio with FEC always on\n\
          audiomirror-cli send --input \"BlackHole 2ch\" --addr 10.0.0.2:5004 \\\n\
              --source system --fec-mode always --bitrate 96000"
    )]
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

    /// Receive Opus packets over UDP and play them on a local output device.
    ///
    /// Example:
    ///   audiomirror-cli recv --output <device-id> --bind 0.0.0.0:5004
    #[command(
        long_about = "Listen for incoming UDP Opus packets on the given bind address and\n\
        play decoded audio on the specified output device. Runs until interrupted (Ctrl-C).\n\
        \n\
        An adaptive jitter buffer absorbs packet reordering and network jitter. Use\n\
        --jitter-mode min for lowest-latency monitoring, or leave it as auto (default) for\n\
        robust delivery over congested LANs.\n\
        \n\
        Examples:\n\
          audiomirror-cli recv --output \"Built-in Speakers\" --bind 0.0.0.0:5004\n\
          \n\
          # Minimum latency mode\n\
          audiomirror-cli recv --output \"Headphones\" --bind 0.0.0.0:5004 --jitter-mode min"
    )]
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

    /// Capture from a local mic and play back on a local output device (loopback test).
    ///
    /// Example:
    ///   audiomirror-cli loop --input <mic-id> --output <spk-id>
    #[command(
        long_about = "Single-process loopback test: capture from an input device, encode\n\
        with Opus, immediately decode, and play back on an output device with no network hop.\n\
        \n\
        Use this to verify the audio pipeline is working end-to-end, measure codec latency,\n\
        and confirm device IDs before setting up a two-machine session.\n\
        \n\
        Note: --source system is available on Windows (WASAPI loopback) and Linux (.monitor\n\
        sources). On macOS prefer BlackHole 2ch as --input instead.\n\
        \n\
        Examples:\n\
          audiomirror-cli loop \\\n\
              --input \"Built-in Microphone\" \\\n\
              --output \"Built-in Speakers\"\n\
          \n\
          # Windows system audio loopback test\n\
          audiomirror-cli loop --input ignored --output \"Speakers\" --source system"
    )]
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

    /// Discover AudioMirror peers on the local network via mDNS.
    ///
    /// Example:
    ///   audiomirror-cli discover --duration-secs 5
    #[command(
        long_about = "Browse for AudioMirror peers on the local network via mDNS\n\
        (_audiomirror._tcp.local.). Prints discovered peers with their peer ID, display name,\n\
        address, and version, then exits after the scan window.\n\
        \n\
        This is a one-shot query and does not require a running daemon. Use the `daemon` REPL\n\
        `peers` command for continuous discovery with automatic reconnection.\n\
        \n\
        Examples:\n\
          audiomirror-cli discover\n\
          audiomirror-cli discover --duration-secs 10"
    )]
    Discover {
        #[arg(long, default_value_t = 5)]
        duration_secs: u64,
        #[arg(long, default_value_t = 7_000)]
        signaling_port: u16,
    },

    /// Start the AudioMirror background daemon (signaling server + device watcher).
    ///
    /// Example:
    ///   audiomirror-cli daemon --signaling-port 5100
    #[command(
        long_about = "Start the AudioMirror background daemon. The daemon binds a TCP\n\
        signaling server, registers an mDNS service record (_audiomirror._tcp.local.),\n\
        starts a device hot-plug watcher, and optionally enables a Prometheus metrics endpoint.\n\
        \n\
        After startup it prints the machine-readable banner:\n\
          READY port=<N>\n\
        where <N> is the actual TCP port. Supervisor scripts must wait for this line before\n\
        issuing REPL commands via stdin.\n\
        \n\
        On first launch a UUID identity is written to the platform config directory:\n\
          macOS   — ~/Library/Application Support/AudioMirror/<identity>/\n\
          Linux   — ~/.config/AudioMirror/<identity>/\n\
          Windows — %APPDATA%\\AudioMirror\\<identity>\\\n\
        \n\
        REPL quick reference (full list: see CLI-REFERENCE.md):\n\
          peers                     list mDNS-discovered peers\n\
          connect <name|id>         open a TCP signaling link to a peer\n\
          accept <n>                trust a pending HELLO from peer at index <n>\n\
          open <name|id>            open a Session with a connected peer\n\
          sessions                  list active sessions and streams\n\
          stream open --from in:0 --to <peer>:out:0 --session <UUID>\n\
          stream stats              live per-stream statistics table\n\
          quit                      graceful shutdown\n\
        \n\
        Examples:\n\
          audiomirror-cli daemon\n\
          audiomirror-cli daemon --signaling-port 5100 --peer-name \"Studio Mac\"\n\
          audiomirror-cli daemon --identity-dir ~/.audiomirror/alice"
    )]
    Daemon {
        #[arg(long, default_value_t = 7_000)]
        signaling_port: u16,
        #[arg(long)]
        peer_name: Option<String>,
        #[arg(long)]
        identity_dir: Option<std::path::PathBuf>,
    },

    /// Open or manage peer-to-peer audio streams (requires a running daemon).
    ///
    /// Example:
    ///   audiomirror-cli stream open --from in:0 --to <peer>:out:0 --session <UUID>
    #[command(
        long_about = "Open or manage peer-to-peer audio streams within the running daemon.\n\
        \n\
        This subcommand is normally invoked from the daemon's interactive REPL rather than\n\
        as a standalone CLI call. Start the daemon first with `audiomirror-cli daemon`, then\n\
        type stream commands at its stdin prompt.\n\
        \n\
        Subcommands:\n\
          stream open  --from in:<idx> --to <peer>:out:<idx> --session <UUID> [--bitrate N]\n\
          stream close <session_id>:<stream_id>\n\
          stream volume <session_id>:<stream_id> <0-100>\n\
          stream mute / unmute <session_id>:<stream_id>\n\
          stream pause / resume <session_id>:<stream_id>\n\
          stream stats [<session_id>:<stream_id>]\n\
        \n\
        Example workflow:\n\
          1. audiomirror-cli daemon\n\
          2. > connect alice          # open signaling link\n\
          3. > accept 0               # (on alice's side) trust the HELLO\n\
          4. > open alice             # open a Session\n\
          5. > stream open --from in:0 --to alice:out:1 --session <UUID>"
    )]
    Stream {
        #[command(subcommand)]
        action: StreamAction,
    },

    /// Show real-time statistics for active streams.
    ///
    /// Examples:
    ///   audiomirror-cli stats
    ///   audiomirror-cli stats --stream-id 1
    #[command(
        long_about = "Display real-time statistics for all active streams managed by the\n\
        running daemon. Refreshes once per second. Press Ctrl-C to exit.\n\
        \n\
        Columns: session, stream, packets sent/recv/lost, bitrate kbps, RTT ms, bandwidth.\n\
        \n\
        Requires a running daemon (`audiomirror-cli daemon`).\n\
        \n\
        Examples:\n\
          audiomirror-cli stats\n\
          audiomirror-cli stats --stream-id 1"
    )]
    Stats {
        #[arg(long)]
        stream_id: Option<u8>,
    },

    /// Inspect application logs.
    #[command(
        long_about = "Inspect the structured application log file written by the daemon\n\
        and other subcommands. Logs rotate daily with 7-day retention.\n\
        \n\
        Subcommands:\n\
          path   Print the absolute path of the current log file\n\
          tail   Poll and print new log lines every 200 ms (Ctrl-C to stop)\n\
        \n\
        Examples:\n\
          audiomirror-cli logs path\n\
          audiomirror-cli logs tail"
    )]
    Logs {
        #[command(subcommand)]
        action: LogsAction,
    },

    /// Read or write application settings.
    #[command(
        long_about = "Read or write the persistent application settings stored as TOML\n\
        in the platform config directory. Changes take effect on the next daemon start.\n\
        \n\
        Subcommands:\n\
          show              print all settings as TOML\n\
          get <key>         print the value of a single key\n\
          set <key> <value> update a single key\n\
        \n\
        Common keys:\n\
          fec_mode          auto|always|never  (default: auto)\n\
          jitter_mode       auto|min           (default: auto)\n\
          jitter_max_depth_ms  u32             (default: 100)\n\
          default_bitrate   i32 bits/s         (default: 64000)\n\
          log_level         trace|debug|info|warn|error\n\
          metrics_enabled   bool               (default: false)\n\
          auto_accept_trusted bool             (default: false)\n\
        \n\
        Examples:\n\
          audiomirror-cli settings show\n\
          audiomirror-cli settings get log_level\n\
          audiomirror-cli settings set fec_mode always\n\
          audiomirror-cli settings set log_level debug"
    )]
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },

    /// Manage autostart (login item / systemd user service / Windows Run key).
    #[command(
        long_about = "Manage the platform-native autostart mechanism for the daemon.\n\
        \n\
        Platform mechanisms:\n\
          macOS   — LaunchAgent plist in ~/Library/LaunchAgents/\n\
          Linux   — systemd user service unit in ~/.config/systemd/user/\n\
          Windows — HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run registry key\n\
        \n\
        Subcommands:\n\
          enable   install the autostart artifact\n\
          disable  remove the autostart artifact\n\
          status   print whether the autostart artifact is present\n\
        \n\
        Examples:\n\
          audiomirror-cli autostart enable\n\
          audiomirror-cli autostart status\n\
          audiomirror-cli autostart disable"
    )]
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },

    /// Enable, disable, or check the Prometheus metrics endpoint.
    #[command(
        long_about = "Manage the optional Prometheus /metrics HTTP endpoint. The endpoint\n\
        is served by the running daemon; changes to metrics_enabled require a daemon restart.\n\
        \n\
        Subcommands:\n\
          enable   set metrics_enabled = true in settings\n\
          disable  set metrics_enabled = false in settings\n\
          status   print metrics_enabled, the configured port, and endpoint reachability\n\
        \n\
        Examples:\n\
          audiomirror-cli metrics enable\n\
          audiomirror-cli daemon &\n\
          curl http://localhost:9000/metrics\n\
          audiomirror-cli metrics status\n\
          # metrics_enabled: true  port: 9000  endpoint_live: true"
    )]
    Metrics {
        #[command(subcommand)]
        action: MetricsAction,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // The daemon subcommand initialises logging itself (using the persisted
    // log_level from settings).  Every other subcommand gets a sensible default
    // at Info so tracing macros work without extra setup.
    let _logs_guard = if !matches!(cli.cmd, Cmd::Daemon { .. }) {
        let logs_dir = audiomirror_core::log_dir()?;
        Some(audiomirror_core::observability::logs::init(
            audiomirror_core::LogLevel::Info,
            &logs_dir,
        )?)
    } else {
        None
    };

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
            identity_dir,
        } => commands::daemon::run(signaling_port, peer_name, identity_dir).await,
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
        Cmd::Metrics { action } => match action {
            MetricsAction::Enable => commands::metrics::run_enable(),
            MetricsAction::Disable => commands::metrics::run_disable(),
            MetricsAction::Status => commands::metrics::run_status().await,
        },
        Cmd::Stream { .. } => {
            anyhow::bail!(
                "stream subcommand requires a running daemon; start with `audiomirror-cli daemon`"
            )
        }
        Cmd::Stats { .. } => {
            anyhow::bail!(
                "stats subcommand requires a running daemon; start with `audiomirror-cli daemon`"
            )
        }
    }
}
