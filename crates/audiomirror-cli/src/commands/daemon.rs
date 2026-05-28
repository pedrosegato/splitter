use super::stream_repl;
use audiomirror_core::audio::devices::{list_devices, DeviceKind};
use audiomirror_core::net::device_watcher;
use audiomirror_core::net::discovery::{DiscoveredPeer, Discovery, DiscoveryEvent};
use audiomirror_core::net::signaling::{
    connect_to_peer, server::accept_pending, server::SignalingServer, CodecParams, Endpoint,
    PeerConnectionHandle, PeerEvent, SignalingMessage, StreamAction,
};
use audiomirror_core::net::stream_runtime::{
    dispatch_device_events, open_stream_as_sink, StreamRegistry,
};
use audiomirror_core::net::trust::TrustStore;
use audiomirror_core::observability::metrics::MetricsRegistry;
use audiomirror_core::settings::{settings_path, Settings};
use audiomirror_core::{log_dir, PeerIdentity, SessionManager, StreamRoute};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use uuid::Uuid;

fn short(u: &Uuid) -> String {
    u.to_string().chars().take(8).collect()
}

async fn peer_display_name(
    peer_id: &Uuid,
    discovered: &Arc<RwLock<HashMap<String, DiscoveredPeer>>>,
    trust: &Arc<RwLock<TrustStore>>,
) -> String {
    {
        let map = discovered.read().await;
        if let Some(p) = map.values().find(|p| p.peer_id == peer_id.to_string()) {
            return p.peer_name.clone();
        }
    }
    {
        let t = trust.read().await;
        if let Some(p) = t.peer_for(peer_id) {
            return p.peer_name.clone();
        }
    }
    short(peer_id)
}

fn pick_default_output_device_id() -> Option<String> {
    list_devices()
        .ok()?
        .into_iter()
        .find(|d| d.kind == DeviceKind::Output)
        .map(|d| d.id)
}

pub(crate) async fn run(
    signaling_port: u16,
    peer_name_override: Option<String>,
    identity_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let settings_handle = Arc::new(RwLock::new(Settings::load_or_default(&settings_path()?)?));

    let log_level = settings_handle.read().await.log_level;
    let _logs_guard = audiomirror_core::observability::logs::init(log_level, &log_dir()?)?;

    let base_dir = identity_dir.unwrap_or_else(|| {
        dirs::config_dir()
            .expect("no config_dir on this platform")
            .join("AudioMirror")
    });
    std::fs::create_dir_all(&base_dir)?;
    let id_path = base_dir.join("identity.toml");
    let trust_path = base_dir.join("trusted_peers.toml");

    let mut identity = PeerIdentity::load_or_create(&id_path)?;
    if let Some(name) = peer_name_override {
        identity.peer_name = name;
    }
    let trust = Arc::new(RwLock::new(TrustStore::load_or_create(&trust_path)?));

    let sessions = SessionManager::new();
    let stream_registry = StreamRegistry::new();

    let bind: SocketAddr = format!("0.0.0.0:{signaling_port}").parse()?;
    let server = SignalingServer::start(
        bind,
        identity.clone(),
        trust.clone(),
        sessions.clone(),
        settings_handle.clone(),
    )
    .await?;

    let outgoing_connections: Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>> = Arc::default();

    let watcher = device_watcher::start(Duration::from_secs(5));
    let dispatcher_rx = watcher.subscribe();
    tokio::spawn(dispatch_device_events(
        stream_registry.clone(),
        dispatcher_rx,
    ));

    // Keep Discovery in scope so graceful_shutdown can call .shutdown() on it.
    let mut discovery = Discovery::start(&identity, signaling_port)?;
    let discovered: Arc<RwLock<HashMap<String, audiomirror_core::net::discovery::DiscoveredPeer>>> =
        Arc::default();

    {
        let s = settings_handle.read().await;
        if s.metrics_enabled {
            let metrics = Arc::new(MetricsRegistry::new()?);
            let metrics_port = s.metrics_port;

            let metrics_tick = metrics.clone();
            let sessions_tick = sessions.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                loop {
                    interval.tick().await;
                    let snap = sessions_tick.snapshot().await;
                    metrics_tick.sessions_active.set(snap.len() as f64);
                    let unique_peers: std::collections::HashSet<_> =
                        snap.iter().map(|s| s.remote_peer_id).collect();
                    metrics_tick.peers_connected.set(unique_peers.len() as f64);
                }
            });

            tokio::spawn(async move {
                if let Err(e) =
                    audiomirror_core::observability::metrics::serve(metrics, metrics_port).await
                {
                    tracing::error!(?e, "metrics server exited");
                }
            });
        }
    }

    #[allow(clippy::print_stdout)]
    {
        println!("READY port={}", server.bind_addr.port());
    }
    tracing::info!(
        peer_id = %identity.peer_id,
        peer_name = %identity.peer_name,
        bind = %server.bind_addr,
        "daemon ready; type 'help' for commands"
    );

    // Spawn the stream-open acceptor for every newly established peer connection,
    // regardless of whether it came through the manual `accept` branch or the
    // auto-accept-trusted shortcut.
    {
        let mut conn_est_rx = server.connection_established_tx.subscribe();
        let conns = server.connections.clone();
        let registry = stream_registry.clone();
        let disc_clone = discovered.clone();
        let trust_clone = trust.clone();
        let sessions_clone = sessions.clone();
        tokio::spawn(async move {
            while let Ok(peer_id) = conn_est_rx.recv().await {
                let name = peer_display_name(&peer_id, &disc_clone, &trust_clone).await;
                #[allow(clippy::print_stdout)]
                {
                    println!(">> {name} connected (peer_id {})", short(&peer_id));
                }
                let guard = conns.read().await;
                if let Some(conn) = guard.get(&peer_id) {
                    spawn_stream_open_acceptor(
                        conn.tx.clone(),
                        conn.events.subscribe(),
                        registry.clone(),
                        peer_id,
                        disc_clone.clone(),
                        trust_clone.clone(),
                        sessions_clone.clone(),
                    );
                }
            }
        });
    }

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    // Install OS signal handlers before entering the REPL loop.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    #[cfg(unix)]
    let mut sigterm = {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::terminate()).expect("SIGTERM handler")
    };

    loop {
        #[cfg(unix)]
        let shutdown_reason: &str = tokio::select! {
            line_res = reader.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        let cmd = line.trim().to_string();
                        if cmd.is_empty() { continue; }
                        if let Err(e) = handle_line(
                            &cmd, &identity, &trust, &sessions,
                            &stream_registry, &server, &discovered, &outgoing_connections,
                        ).await {
                            tracing::error!("command failed: {e}");
                        }
                        if cmd == "quit" { "quit" } else { continue; }
                    }
                    _ => "stdin closed",
                }
            }
            _ = &mut ctrl_c => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
            disc_ev = discovery.next_event() => {
                match disc_ev {
                    Some(DiscoveryEvent::Found(p)) => {
                        discovered.write().await.insert(p.peer_id.clone(), p);
                    }
                    Some(DiscoveryEvent::Removed(name)) => {
                        tracing::info!("peer removed: {name}");
                    }
                    None => {}
                }
                continue;
            }
        };

        #[cfg(not(unix))]
        let shutdown_reason: &str = tokio::select! {
            line_res = reader.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        let cmd = line.trim().to_string();
                        if cmd.is_empty() { continue; }
                        if let Err(e) = handle_line(
                            &cmd, &identity, &trust, &sessions,
                            &stream_registry, &server, &discovered, &outgoing_connections,
                        ).await {
                            tracing::error!("command failed: {e}");
                        }
                        if cmd == "quit" { "quit" } else { continue; }
                    }
                    _ => "stdin closed",
                }
            }
            _ = &mut ctrl_c => "SIGINT",
            disc_ev = discovery.next_event() => {
                match disc_ev {
                    Some(DiscoveryEvent::Found(p)) => {
                        discovered.write().await.insert(p.peer_id.clone(), p);
                    }
                    Some(DiscoveryEvent::Removed(name)) => {
                        tracing::info!("peer removed: {name}");
                    }
                    None => {}
                }
                continue;
            }
        };

        tracing::info!("shutdown triggered: {shutdown_reason}");
        graceful_shutdown(
            &sessions,
            &stream_registry,
            Some(&server),
            &outgoing_connections,
        )
        .await;
        discovery.shutdown();
        // Drop server last: closes the TCP accept loop and all peer connections.
        drop(server);
        tracing::info!("daemon shutdown complete");
        break;
    }

    Ok(())
}

// Ordered teardown sequence (non-obvious ordering rationale):
// 1. Send StreamControl{Close} to peers first so the remote side tears down cleanly.
// 2. Then close local stream runtimes (which also sends Close + aborts the pump task).
// 3. Sleep 150 ms: gives TCP framing time to flush the Close messages.
// 4. Close SessionManager entries (bookkeeping only at this point).
async fn graceful_shutdown(
    sessions: &Arc<SessionManager>,
    stream_registry: &Arc<StreamRegistry>,
    server: Option<&audiomirror_core::net::signaling::server::SignalingServerHandle>,
    outgoing: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
) {
    let session_snap = sessions.snapshot().await;

    // 1. Notify peers: send StreamControl{Close} for every active stream.
    let outgoing_guard = outgoing.read().await;
    if let Some(srv) = server {
        let conns = srv.connections.read().await;
        for sess in &session_snap {
            let tx = conns
                .get(&sess.remote_peer_id)
                .map(|c| &c.tx)
                .or_else(|| outgoing_guard.get(&sess.remote_peer_id).map(|c| &c.tx));
            if let Some(tx) = tx {
                for stream in &sess.streams {
                    let _ = tx
                        .send(SignalingMessage::StreamControl {
                            stream_id: stream.id,
                            action: StreamAction::Close,
                            volume: None,
                        })
                        .await;
                }
            }
        }
    }
    drop(outgoing_guard);

    // 2. Close all local StreamRuntime pump tasks via the public registry API.
    let summaries = stream_registry.list().await;
    for summary in summaries {
        let _ = stream_registry
            .close(&summary.session_id, summary.stream_id)
            .await;
    }

    // 3. Drain window: give in-flight TCP/UDP packets time to leave the kernel buffers.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 4. Close sessions in the SessionManager.
    for sess in &session_snap {
        let _ = sessions.close(&sess.id).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_line(
    line: &str,
    identity: &PeerIdentity,
    trust: &Arc<RwLock<TrustStore>>,
    sessions: &Arc<SessionManager>,
    stream_registry: &Arc<StreamRegistry>,
    server: &audiomirror_core::net::signaling::server::SignalingServerHandle,
    discovered: &Arc<RwLock<HashMap<String, audiomirror_core::net::discovery::DiscoveredPeer>>>,
    outgoing_connections: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
) -> anyhow::Result<()> {
    let tokens = split_repl_line(line);
    let token_refs: Vec<&str> = tokens.iter().map(|s| s.as_str()).collect();
    let mut parts = token_refs.iter().copied();
    let head = parts.next().unwrap_or("");
    #[allow(clippy::print_stdout)]
    match head {
        "help" => {
            const BAR: &str = "═══════════════════════════════════════════════════════════════════";
            println!("{BAR}");
            println!("  AUDIOMIRROR DAEMON — COMMANDS");
            println!("{BAR}");
            println!("  {:<44}  list discovered peers", "peers");
            println!("  {:<44}  list peers waiting for accept", "pending");
            println!("  {:<44}  accept a pending peer (TOFU)", "accept <idx>");
            println!("  {:<44}  open signaling link", "connect <peer_id|name>");
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
            println!("  {:<44}  close one stream", "stream close <sid:stream>");
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
        "peers" => {
            const BAR: &str = "═══════════════════════════════════════════════════════════════════";
            let snap = discovered.read().await.clone();
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
        "pending" => {
            const BAR: &str = "═══════════════════════════════════════════════════════════════════";
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
        "accept" => {
            let idx: usize = parts.next().unwrap_or("0").parse()?;
            let (peer_id, _token) = accept_pending(
                &server.pending,
                trust,
                &server.connections,
                &server.connection_established_tx,
                idx,
            )
            .await?;
            println!(">> accepted pending #{idx} -> peer {peer_id}");
        }
        "connect" => {
            let key = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: connect <peer_id|name>"))?;
            let target = {
                let map = discovered.read().await;
                map.values()
                    .find(|p| p.peer_id == key || p.peer_name == key)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown peer {key}"))?
            };
            let host_port: SocketAddr = format!("{}:{}", target.host, target.port).parse()?;
            let remote_uuid = Uuid::parse_str(&target.peer_id).ok();
            let outcome = connect_to_peer(
                host_port,
                identity,
                trust.clone(),
                remote_uuid,
                Duration::from_secs(5),
            )
            .await?;
            if outcome.accepted {
                println!(">> connected to {}", target.peer_name);
            } else {
                tracing::warn!(
                    "connect not yet accepted (reason={:?}); waiting for remote operator to accept",
                    outcome.reason
                );
            }
            if let Some(peer_uuid) = outcome.remote_peer_id {
                spawn_stream_open_acceptor(
                    outcome.handle.tx.clone(),
                    outcome.handle.events.subscribe(),
                    stream_registry.clone(),
                    peer_uuid,
                    discovered.clone(),
                    trust.clone(),
                    sessions.clone(),
                );
                let mut events_rx = outcome.handle.events.subscribe();
                outgoing_connections
                    .write()
                    .await
                    .insert(peer_uuid, outcome.handle);
                let map = outgoing_connections.clone();
                tokio::spawn(async move {
                    loop {
                        match events_rx.recv().await {
                            Ok(PeerEvent::Disconnected { .. }) | Err(_) => {
                                map.write().await.remove(&peer_uuid);
                                break;
                            }
                            Ok(_) => {}
                        }
                    }
                });
            }
        }
        "open" => {
            let key = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: open <peer_id|name>"))?;
            let target = {
                let map = discovered.read().await;
                map.values()
                    .find(|p| p.peer_id == key || p.peer_name == key)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown peer {key}"))?
            };
            let remote_uuid = Uuid::parse_str(&target.peer_id)?;
            let session_id = sessions.open_outgoing(identity.peer_id, remote_uuid).await;
            sessions.accept(&session_id).await?;
            println!(">> opened session {session_id} with {}", target.peer_name);
            let conn_tx = {
                let inbound = server.connections.read().await;
                if let Some(h) = inbound.get(&remote_uuid) {
                    Some(h.tx.clone())
                } else {
                    let outbound = outgoing_connections.read().await;
                    outbound.get(&remote_uuid).map(|h| h.tx.clone())
                }
            };
            if let Some(tx) = conn_tx {
                tx.send(SignalingMessage::SessionRequest {
                    session_id: session_id.to_string(),
                    requested_by: identity.peer_id.to_string(),
                })
                .await
                .ok();
            }
        }
        "sessions" => {
            const BAR: &str = "═══════════════════════════════════════════════════════════════════";
            let snap = sessions.snapshot().await;
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
        "disconnect" => {
            let key = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: disconnect <session_id>"))?;
            let id = Uuid::parse_str(key)?;
            sessions.close(&id).await?;
            println!(">> disconnected session {id}");
        }
        "quit" => {
            // handled in run() via the select! arm — nothing more to do here
        }
        "stream" => {
            stream_repl::handle(
                parts.collect::<Vec<_>>().as_slice(),
                identity,
                sessions,
                stream_registry,
                server,
                outgoing_connections,
            )
            .await?;
        }
        other => {
            tracing::warn!("unknown command: {other}");
        }
    }
    Ok(())
}

fn spawn_stream_open_acceptor(
    conn_tx: tokio::sync::mpsc::Sender<SignalingMessage>,
    mut events: tokio::sync::broadcast::Receiver<PeerEvent>,
    registry: Arc<StreamRegistry>,
    peer_id: Uuid,
    discovered: Arc<RwLock<HashMap<String, DiscoveredPeer>>>,
    trust: Arc<RwLock<TrustStore>>,
    sessions: Arc<SessionManager>,
) {
    let default_output =
        pick_default_output_device_id().unwrap_or_else(|| "Output:0:default".into());
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(msg)) => match msg {
                    SignalingMessage::SessionRequest {
                        session_id,
                        requested_by,
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id) else {
                            continue;
                        };
                        let Ok(requester_uuid) = Uuid::parse_str(&requested_by) else {
                            continue;
                        };
                        let _ = sessions
                            .register_incoming(sid_uuid, peer_id, requester_uuid)
                            .await;
                        let _ = sessions.accept(&sid_uuid).await;
                        let name = peer_display_name(&peer_id, &discovered, &trust).await;
                        #[allow(clippy::print_stdout)]
                        {
                            println!(">> {name} opened session {}", short(&sid_uuid));
                        }
                    }
                    SignalingMessage::StreamOpen {
                        session_id,
                        stream_id,
                        source,
                        sink,
                        codec,
                        ..
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id) else {
                            continue;
                        };
                        let route = StreamRoute {
                            source: Endpoint {
                                peer_id: source.peer_id.clone(),
                                device_id: source.device_id.clone(),
                            },
                            sink: Endpoint {
                                peer_id: sink.peer_id.clone(),
                                device_id: sink.device_id.clone(),
                            },
                            codec: CodecParams {
                                name: codec.name.clone(),
                                bitrate: codec.bitrate,
                                frame_ms: codec.frame_ms,
                            },
                            volume: 1.0,
                        };
                        let chosen_output = if sink.device_id == "default" {
                            default_output.clone()
                        } else {
                            sink.device_id.clone()
                        };
                        match open_stream_as_sink(
                            registry.clone(),
                            sid_uuid,
                            stream_id,
                            route,
                            chosen_output.clone(),
                        )
                        .await
                        {
                            Ok(port) => {
                                let _ = conn_tx
                                    .send(SignalingMessage::StreamOpenAck {
                                        stream_id,
                                        accepted: true,
                                        udp_port: Some(port),
                                    })
                                    .await;
                                let name = peer_display_name(&peer_id, &discovered, &trust).await;
                                #[allow(clippy::print_stdout)]
                                {
                                    println!(
                                            ">> {name} opened stream {stream_id} from {} \u{2192} local {chosen_output}",
                                            source.device_id
                                        );
                                }
                            }
                            Err(e) => {
                                tracing::warn!("stream_open accept failed: {e}");
                                let _ = conn_tx
                                    .send(SignalingMessage::StreamOpenAck {
                                        stream_id,
                                        accepted: false,
                                        udp_port: None,
                                    })
                                    .await;
                            }
                        }
                    }
                    SignalingMessage::StreamControl {
                        stream_id,
                        action,
                        volume,
                    } => {
                        let name = peer_display_name(&peer_id, &discovered, &trust).await;
                        #[allow(clippy::print_stdout)]
                        {
                            match action {
                                StreamAction::Close => {
                                    println!(">> {name} closed stream {stream_id}");
                                }
                                StreamAction::Pause => {
                                    println!(">> {name} paused stream {stream_id}");
                                }
                                StreamAction::Resume => {
                                    println!(">> {name} resumed stream {stream_id}");
                                }
                                StreamAction::SetVolume => {
                                    let pct =
                                        volume.map(|v| (v * 100.0).round() as u32).unwrap_or(100);
                                    println!(">> {name} set stream {stream_id} volume to {pct}%");
                                }
                            }
                        }
                        if matches!(action, StreamAction::Close) {
                            let session_ids: Vec<Uuid> = sessions
                                .snapshot()
                                .await
                                .into_iter()
                                .filter(|s| s.remote_peer_id == peer_id)
                                .map(|s| s.id)
                                .collect();
                            for sid in session_ids {
                                let _ = registry.close(&sid, stream_id).await;
                            }
                        }
                    }
                    _ => {}
                },
                Ok(PeerEvent::Disconnected { reason }) => {
                    let name = peer_display_name(&peer_id, &discovered, &trust).await;
                    #[allow(clippy::print_stdout)]
                    {
                        println!(">> {name} disconnected (reason: {reason})");
                    }
                    break;
                }
                Ok(PeerEvent::Connected { .. }) => {}
                Err(_) => break,
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use audiomirror_core::settings::LogLevel;

    #[test]
    fn pick_default_output_device_id_returns_output_kind_or_none() {
        if let Some(id) = pick_default_output_device_id() {
            assert!(
                id.starts_with("Output:"),
                "expected Output: prefix, got {id}"
            );
        }
    }

    #[test]
    fn boot_settings_load_or_default_returns_info_level() {
        let missing = std::path::PathBuf::from("/tmp/am_daemon_boot_test_no_such_file.toml");
        let _ = std::fs::remove_file(&missing);
        let s = Settings::load_or_default(&missing).expect("load_or_default should not fail");
        assert_eq!(
            s.log_level,
            LogLevel::Info,
            "default log_level must be Info"
        );
        assert!(!s.metrics_enabled, "metrics must be off by default");
        assert_eq!(s.metrics_port, 9000, "default metrics port must be 9000");
    }

    #[test]
    fn boot_metrics_flag_read_from_settings() {
        let s_off = Settings {
            metrics_enabled: false,
            ..Settings::default()
        };
        let s_on = Settings {
            metrics_enabled: true,
            ..Settings::default()
        };
        assert!(!s_off.metrics_enabled);
        assert!(s_on.metrics_enabled);
    }

    #[test]
    fn split_repl_line_respects_double_quotes() {
        let v = super::split_repl_line(
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
        let v = super::split_repl_line("connect bob");
        assert_eq!(v, vec!["connect", "bob"]);
    }

    #[tokio::test]
    async fn graceful_shutdown_on_empty_state_does_not_panic() {
        let sessions = audiomirror_core::SessionManager::new();
        let registry = audiomirror_core::StreamRegistry::new();
        let outgoing = Arc::new(RwLock::new(HashMap::new()));
        graceful_shutdown(&sessions, &registry, None, &outgoing).await;
    }
}
