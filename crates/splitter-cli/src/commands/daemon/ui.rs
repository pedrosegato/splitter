use super::context::DaemonContext;
use splitter_core::net::signaling::server::SignalingServerHandle;

pub(crate) const BAR: &str = "═══════════════════════════════════════════════════════════════════";

#[allow(clippy::print_stdout)]
pub(crate) fn print_help() {
    println!("{BAR}");
    println!("  SPLITTER DAEMON — COMMANDS");
    println!("{BAR}");
    println!("  {:<44}  list discovered peers", "peers");
    println!("  {:<44}  list peers waiting for accept", "pending");
    println!("  {:<44}  accept a pending peer (TOFU)", "accept <idx>");
    println!(
        "  {:<44}  open signaling link (name, peer_id, or host:port)",
        "connect <peer_id|name|host:port>"
    );
    println!(
        "  {:<44}  open a session with a connected peer",
        "open <peer_id|name>"
    );
    println!("  {:<44}  list active sessions", "sessions");
    println!(
        "  {:<44}  open a stream (see help below)",
        "stream open ..."
    );
    println!(
        "  {:<44}  show stream stats once",
        "stream stats [sid:stream]"
    );
    println!(
        "  {:<44}  close one stream",
        "stream close <xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx:N>"
    );
    println!(
        "  {:<44}  set volume (100 = unity)",
        "stream volume <sid:stream> <0-200>"
    );
    println!("  {:<44}  mute", "stream mute <sid:stream>");
    println!("  {:<44}  unmute", "stream unmute <sid:stream>");
    println!("  {:<44}  pause", "stream pause <sid:stream>");
    println!("  {:<44}  resume", "stream resume <sid:stream>");
    println!(
        "  {:<44}  close session and all streams",
        "disconnect <session_id>"
    );
    println!(
        "  {:<44}  runtime settings",
        "settings show | get <k> | set <k> <v>"
    );
    println!("  {:<44}  graceful shutdown", "quit");
    println!("{BAR}");
}

#[allow(clippy::print_stdout)]
pub(crate) async fn print_peers(ctx: &DaemonContext) {
    let snap = ctx.discovered.read().await.clone();
    println!("{BAR}");
    println!("  PEERS DISCOVERED");
    println!("{BAR}");
    if snap.is_empty() {
        println!("  (none)");
    } else {
        println!(
            "  {:<5}  {:<14}  {:<36}  {:<21}  VERSION",
            "IDX", "NAME", "PEER_ID", "ADDR"
        );
        println!(
            "  {:<5}  {:<14}  {:<36}  {:<21}  ───────",
            "───", "────", "───────", "────"
        );
        for (idx, p) in snap.values().enumerate() {
            let addr = format!("{}:{}", p.host, p.port);
            let ver = format!("v{}", p.version);
            println!(
                "  {:<5}  {:<14}  {:<36}  {:<21}  {}",
                format!("[{idx}]"),
                p.peer_name,
                p.peer_id,
                addr,
                ver
            );
        }
    }
    println!("{BAR}");
}

#[allow(clippy::print_stdout)]
pub(crate) async fn print_pending(server: &SignalingServerHandle) {
    let list = server.pending.list().await;
    println!("{BAR}");
    println!("  PENDING HELLOS");
    println!("{BAR}");
    if list.is_empty() {
        println!("  (none)");
    } else {
        println!("  {:<5}  {:<14}  {:<36}  ADDR", "IDX", "NAME", "PEER_ID");
        println!("  {:<5}  {:<14}  {:<36}  ────", "───", "────", "───────");
        for (i, p) in list.iter().enumerate() {
            println!(
                "  {:<5}  {:<14}  {:<36}  {}",
                format!("[{i}]"),
                p.peer_name,
                p.peer_id,
                p.remote_addr
            );
        }
    }
    println!("{BAR}");
}

#[allow(clippy::print_stdout)]
pub(crate) async fn print_sessions(ctx: &DaemonContext) {
    let snap = ctx.sessions.snapshot().await;
    println!("{BAR}");
    println!("  ACTIVE SESSIONS");
    println!("{BAR}");
    if snap.is_empty() {
        println!("  (none)");
    } else {
        println!(
            "  {:<38}  {:<8}  {:<36}  STREAMS",
            "SESSION ID", "STATE", "REMOTE PEER ID"
        );
        println!(
            "  {:<38}  {:<8}  {:<36}  ───────",
            "──────────", "─────", "──────────────"
        );
        for s in snap {
            println!(
                "  {:<38}  {:<8}  {:<36}  {}",
                s.id,
                format!("{:?}", s.state),
                s.remote_peer_id,
                s.streams.len()
            );
        }
    }
    println!("{BAR}");
}
