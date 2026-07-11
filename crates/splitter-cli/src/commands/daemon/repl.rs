use super::context::{short, DaemonContext};
use super::{graceful_shutdown, ui};
use crate::commands::stream_repl;
use splitter_core::net::discovery::{Discovery, DiscoveryEvent};
use splitter_core::net::signaling::client_ops::{find_conn, find_conn_tx, notify_remote_control};
use splitter_core::net::signaling::server::{accept_pending_as, SignalingServerHandle};
use splitter_core::net::signaling::{connect_to_peer, SignalingMessage, StreamAction};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use uuid::Uuid;

#[cfg(unix)]
type SigTerm = Option<tokio::signal::unix::Signal>;
#[cfg(not(unix))]
type SigTerm = Option<()>;

enum LineOutcome {
    Got(String),
    Closed,
}

enum ReplEvent {
    Line(LineOutcome),
    Shutdown(&'static str),
    Discovery(Option<DiscoveryEvent>),
}

pub(crate) async fn run_repl(
    ctx: &DaemonContext,
    server: SignalingServerHandle,
    mut discovery: Discovery,
) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    // Install OS signal handlers before entering the REPL loop.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    #[cfg(unix)]
    let mut sigterm: SigTerm = {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("SIGTERM handler registration failed, proceeding without it: {e}");
                None
            }
        }
    };
    #[cfg(not(unix))]
    let mut sigterm: SigTerm = None;

    loop {
        let event = tokio::select! {
            line_res = reader.next_line() => match line_res {
                Ok(Some(line)) => ReplEvent::Line(LineOutcome::Got(line)),
                _ => ReplEvent::Line(LineOutcome::Closed),
            },
            _ = &mut ctrl_c => ReplEvent::Shutdown("SIGINT"),
            _ = sigterm_future(sigterm.as_mut()) => ReplEvent::Shutdown("SIGTERM"),
            disc_ev = discovery.next_event() => ReplEvent::Discovery(disc_ev),
        };

        if let Some(reason) = process_repl_event(ctx, &server, event).await {
            tracing::info!("shutdown triggered: {reason}");
            graceful_shutdown(
                &ctx.sessions,
                &ctx.stream_registry,
                Some(&server),
                &ctx.outgoing_connections,
            )
            .await;
            discovery.shutdown();
            // Drop server last: closes the TCP accept loop and all peer connections.
            drop(server);
            tracing::info!("daemon shutdown complete");
            #[allow(clippy::print_stdout)]
            {
                println!(">> goodbye");
            }
            break;
        }
    }
    Ok(())
}

// Yields a never-resolving future when SIGTERM is unavailable so the select! arm is inert.
#[cfg(unix)]
async fn sigterm_future(sigterm: Option<&mut tokio::signal::unix::Signal>) {
    match sigterm {
        Some(s) => {
            s.recv().await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(not(unix))]
async fn sigterm_future(_sigterm: Option<&mut ()>) {
    std::future::pending().await
}

async fn process_repl_event(
    ctx: &DaemonContext,
    server: &SignalingServerHandle,
    event: ReplEvent,
) -> Option<&'static str> {
    match event {
        ReplEvent::Line(LineOutcome::Got(line)) => {
            let cmd = line.trim().to_string();
            if cmd.is_empty() {
                return None;
            }
            if let Err(e) = process_command(ctx, server, &cmd).await {
                tracing::error!("command failed: {e}");
                #[allow(clippy::print_stdout)]
                {
                    println!(">> error: {e}");
                }
            }
            if cmd == "quit" {
                Some("quit")
            } else {
                None
            }
        }
        ReplEvent::Line(LineOutcome::Closed) => Some("stdin closed"),
        ReplEvent::Shutdown(reason) => Some(reason),
        ReplEvent::Discovery(disc_ev) => {
            match disc_ev {
                Some(DiscoveryEvent::Found(p)) => {
                    ctx.discovered.write().await.insert(p.peer_id.clone(), p);
                }
                Some(DiscoveryEvent::Removed(name)) => {
                    tracing::debug!("peer removed: {name}");
                }
                None => {}
            }
            None
        }
    }
}

fn is_uuid(s: &str) -> bool {
    Uuid::try_parse(s).is_ok()
}

fn split_repl_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut started = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

async fn process_command(
    ctx: &DaemonContext,
    server: &SignalingServerHandle,
    line: &str,
) -> anyhow::Result<()> {
    let tokens = split_repl_line(line);
    let token_refs: Vec<&str> = tokens.iter().map(|s| s.as_str()).collect();
    let mut parts = token_refs.iter().copied();
    let head = parts.next().unwrap_or("");
    match head {
        "help" => ui::print_help(),
        "peers" => ui::print_peers(ctx).await,
        "pending" => ui::print_pending(server).await,
        "sessions" => ui::print_sessions(ctx).await,
        "accept" => cmd_accept(ctx, server, &mut parts).await?,
        "connect" => cmd_connect(ctx, &mut parts).await?,
        "open" => cmd_open(ctx, server, &mut parts).await?,
        "disconnect" => cmd_disconnect(ctx, server, &mut parts).await?,
        "quit" => {
            // handled in run_repl() via the select! arm — nothing more to do here
        }
        "stream" => {
            stream_repl::handle(
                parts.collect::<Vec<_>>().as_slice(),
                &ctx.identity,
                &ctx.sessions,
                &ctx.stream_registry,
                server,
                &ctx.outgoing_connections,
            )
            .await?;
        }
        other => {
            tracing::warn!("unknown command: {other}");
        }
    }
    Ok(())
}

async fn cmd_accept<'a>(
    ctx: &DaemonContext,
    server: &SignalingServerHandle,
    parts: &mut impl Iterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let idx: usize = parts.next().unwrap_or("0").parse()?;
    let (peer_id, _token) = accept_pending_as(
        &server.pending,
        &ctx.trust,
        &server.connections,
        &server.connection_established_tx,
        idx,
        Some(ctx.local_peer_id),
    )
    .await?;
    #[allow(clippy::print_stdout)]
    {
        println!(">> accepted pending #{idx} -> peer {peer_id}");
    }
    Ok(())
}

async fn cmd_connect<'a>(
    ctx: &DaemonContext,
    parts: &mut impl Iterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let key = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: connect <peer_id|name|host:port>"))?;

    let (host_port, remote_uuid) = if key.contains(':') && !is_uuid(key) {
        let addr: SocketAddr = key.parse().map_err(|_| {
            anyhow::anyhow!("invalid address: {key}; expected host:port or a known peer name/id")
        })?;
        (addr, None)
    } else {
        let target = {
            let map = ctx.discovered.read().await;
            map.values()
                .find(|p| p.peer_id == key || p.peer_name == key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown peer {key}"))?
        };
        let addr: SocketAddr = format!("{}:{}", target.host, target.port).parse()?;
        let uuid = Uuid::parse_str(&target.peer_id).ok();
        (addr, uuid)
    };

    let outcome = connect_to_peer(
        host_port,
        &ctx.identity,
        ctx.trust.clone(),
        remote_uuid,
        Duration::from_secs(5),
    )
    .await?;

    let display_name = if let Some(id) = outcome.remote_peer_id {
        ctx.peer_display_name(&id).await
    } else {
        host_port.to_string()
    };

    #[allow(clippy::print_stdout)]
    if outcome.accepted {
        println!(">> connected to {display_name}");
    } else {
        tracing::warn!(
            "connect not yet accepted (reason={:?}); waiting for remote operator to accept",
            outcome.reason
        );
        println!(">> hello sent to {display_name} — waiting for remote operator to accept");
    }
    if let Some(peer_uuid) = outcome.remote_peer_id {
        ctx.register_outgoing_connection(peer_uuid, outcome.handle)
            .await;
    }
    Ok(())
}

async fn cmd_open<'a>(
    ctx: &DaemonContext,
    server: &SignalingServerHandle,
    parts: &mut impl Iterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let key = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: open <peer_id|name>"))?;
    let target = {
        let map = ctx.discovered.read().await;
        map.values()
            .find(|p| p.peer_id == key || p.peer_name == key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown peer {key}"))?
    };
    let remote_uuid = Uuid::parse_str(&target.peer_id)?;

    let existing = ctx.sessions.snapshot().await.into_iter().find(|s| {
        s.remote_peer_id == remote_uuid
            && s.state == splitter_core::net::session::SessionState::Active
    });
    if let Some(sess) = existing {
        #[allow(clippy::print_stdout)]
        {
            println!(
                ">> existing active session with {} (session_id: {})",
                target.peer_name, sess.id
            );
        }
        return Ok(());
    }

    let session_id = ctx
        .sessions
        .open_outgoing(ctx.identity.peer_id, remote_uuid)
        .await;
    ctx.sessions.accept(&session_id).await?;
    #[allow(clippy::print_stdout)]
    {
        println!(">> opened session {session_id} with {}", target.peer_name);
    }
    let conn = find_conn(&server.connections, &ctx.outgoing_connections, remote_uuid).await;
    if let Some(conn) = conn {
        ctx.sessions
            .set_session_owner(&session_id, conn.connection_id)
            .await;
        conn.tx
            .send(SignalingMessage::SessionRequest {
                session_id: session_id.to_string(),
                requested_by: ctx.identity.peer_id.to_string(),
            })
            .await
            .ok();
    }
    Ok(())
}

async fn cmd_disconnect<'a>(
    ctx: &DaemonContext,
    server: &SignalingServerHandle,
    parts: &mut impl Iterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let key = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: disconnect <session_id>"))?;
    let id = splitter_core::SessionId(Uuid::parse_str(key)?);
    // Close all local stream runtimes for this session and notify the remote peer.
    let snap = ctx.sessions.snapshot().await;
    let remote = if let Some(sess) = snap.iter().find(|s| s.id == id) {
        let conn_tx = find_conn_tx(
            &server.connections,
            &ctx.outgoing_connections,
            sess.remote_peer_id,
        )
        .await;
        for stream in &sess.streams {
            let _ = ctx.stream_registry.close(&id, stream.id).await;
            if let Some(ref tx) = conn_tx {
                notify_remote_control(tx, stream.id.get(), StreamAction::Close).await;
            }
        }
        Some(sess.remote_peer_id)
    } else {
        None
    };
    ctx.sessions.remove(&id).await;
    // shutdown() aborts the connection task so the socket closes even though the
    // stream-open acceptor still holds a tx clone; the abort fires no Disconnected
    // event, so no reconnect is scheduled.
    if let Some(remote) = remote {
        if let Some(handle) = server.connections.write().await.remove(&remote) {
            handle.shutdown();
        }
        if let Some(handle) = ctx.outgoing_connections.write().await.remove(&remote) {
            handle.shutdown();
        }
    }
    #[allow(clippy::print_stdout)]
    {
        println!(">> session {} closed", short(&id.get()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_uuid_recognizes_valid_uuid() {
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn is_uuid_rejects_host_port() {
        assert!(!is_uuid("192.168.1.10:7777"));
        assert!(!is_uuid("localhost:7777"));
    }

    #[test]
    fn split_repl_line_respects_double_quotes() {
        let v = split_repl_line(
            r#"stream open --from "BlackHole 2ch" --to bob:"Alto-falantes (MCHOSE V9 PRO)""#,
        );
        assert_eq!(
            v,
            vec![
                "stream",
                "open",
                "--from",
                "BlackHole 2ch",
                "--to",
                "bob:Alto-falantes (MCHOSE V9 PRO)",
            ]
        );
    }

    #[test]
    fn split_repl_line_plain_words() {
        let v = split_repl_line("connect bob");
        assert_eq!(v, vec!["connect", "bob"]);
    }
}
