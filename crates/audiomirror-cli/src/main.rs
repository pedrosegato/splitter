use clap::{Parser, Subcommand};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
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
        Cmd::Recv { output, bind } => commands::recv::run(&output, &bind).await,
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
    }
}
