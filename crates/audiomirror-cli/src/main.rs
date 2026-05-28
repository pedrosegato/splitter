use clap::{Parser, Subcommand};

mod commands;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum Source {
    Mic,
    System,
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
        } => commands::send::run(&input, &addr, stream_id, bitrate).await,
        Cmd::Recv { output, bind } => commands::recv::run(&output, &bind).await,
        Cmd::Loop {
            input,
            output,
            bitrate,
            source,
        } => commands::loop_cmd::run(&input, &output, bitrate, source).await,
    }
}
