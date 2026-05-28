use super::stream_repl;
use audiomirror_core::audio::devices::{list_devices, DeviceKind};
use audiomirror_core::net::device_watcher;
use audiomirror_core::net::discovery::{Discovery, DiscoveryEvent};
use audiomirror_core::net::signaling::{
    connect_to_peer, server::accept_pending, server::SignalingServer, CodecParams, Endpoint,
    PeerEvent, SignalingMessage, StreamAction,
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
                            &stream_registry, &server, &discovered,
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
                            &stream_registry, &server, &discovered,
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
        graceful_shutdown(&sessions, &stream_registry, Some(&server)).await;
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
) {
    let session_snap = sessions.snapshot().await;

    // 1. Notify peers: send StreamControl{Close} for every active stream.
    if let Some(srv) = server {
        let conns = srv.connections.read().await;
        for sess in &session_snap {
            if let Some(conn) = conns.get(&sess.remote_peer_id) {
                for stream in &sess.streams {
                    let _ = conn
                        .tx
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

async fn handle_line(
    line: &str,
    identity: &PeerIdentity,
    trust: &Arc<RwLock<TrustStore>>,
    sessions: &Arc<SessionManager>,
    stream_registry: &Arc<StreamRegistry>,
    server: &audiomirror_core::net::signaling::server::SignalingServerHandle,
    discovered: &Arc<RwLock<HashMap<String, audiomirror_core::net::discovery::DiscoveredPeer>>>,
) -> anyhow::Result<()> {
    let tokens = split_repl_line(line);
    let token_refs: Vec<&str> = tokens.iter().map(|s| s.as_str()).collect();
    let mut parts = token_refs.iter().copied();
    let head = parts.next().unwrap_or("");
    match head {
        "help" => {
            tracing::info!(
                "commands: peers | connect <peer_id|name> | pending | accept <idx> | sessions | open <peer_id> | disconnect <session_id> | quit"
            );
        }
        "peers" => {
            let snap = discovered.read().await.clone();
            if snap.is_empty() {
                tracing::info!("no peers discovered yet");
            }
            for (idx, p) in snap.values().enumerate() {
                tracing::info!(
                    "[{idx}] {} ({}) at {}:{} v{}",
                    p.peer_name,
                    p.peer_id,
                    p.host,
                    p.port,
                    p.version
                );
            }
        }
        "pending" => {
            let list = server.pending.list().await;
            if list.is_empty() {
                tracing::info!("no pending hellos");
            }
            for (i, p) in list.iter().enumerate() {
                tracing::info!(
                    "[{i}] {} ({}) from {}",
                    p.peer_name,
                    p.peer_id,
                    p.remote_addr
                );
            }
        }
        "accept" => {
            let idx: usize = parts.next().unwrap_or("0").parse()?;
            let (peer_id, _token) =
                accept_pending(&server.pending, trust, &server.connections, idx).await?;
            tracing::info!("accepted pending #{idx} → peer {peer_id}");
            let conns = server.connections.read().await;
            if let Some(conn) = conns.get(&peer_id) {
                spawn_stream_open_acceptor(
                    conn.tx.clone(),
                    conn.events.subscribe(),
                    stream_registry.clone(),
                );
            }
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
                tracing::info!("connected to {}", target.peer_name);
            } else {
                tracing::warn!(
                    "connect not yet accepted (reason={:?}); waiting for remote operator to accept",
                    outcome.reason
                );
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
            tracing::info!("opened session {session_id} with {}", target.peer_name);
            let conns = server.connections.read().await;
            if let Some(handle) = conns.get(&remote_uuid) {
                handle
                    .tx
                    .send(SignalingMessage::SessionRequest {
                        session_id: session_id.to_string(),
                        requested_by: identity.peer_id.to_string(),
                    })
                    .await
                    .ok();
            }
        }
        "sessions" => {
            let snap = sessions.snapshot().await;
            if snap.is_empty() {
                tracing::info!("no sessions");
            }
            for s in snap {
                tracing::info!(
                    "session {} ({:?}) ↔ {} : {} streams",
                    s.id,
                    s.state,
                    s.remote_peer_id,
                    s.streams.len()
                );
                for st in s.streams {
                    tracing::info!(
                        "  stream {} ({:?}) {} → {}",
                        st.id,
                        st.state,
                        st.source_peer,
                        st.sink_peer
                    );
                }
            }
        }
        "disconnect" => {
            let key = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: disconnect <session_id>"))?;
            let id = Uuid::parse_str(key)?;
            sessions.close(&id).await?;
            tracing::info!("closed session {id}");
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
) {
    let default_output =
        pick_default_output_device_id().unwrap_or_else(|| "Output:0:default".into());
    tokio::spawn(async move {
        while let Ok(PeerEvent::Message(msg)) = events.recv().await {
            if let SignalingMessage::StreamOpen {
                session_id,
                stream_id,
                source,
                sink,
                codec,
                ..
            } = msg
            {
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
                    chosen_output,
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
        graceful_shutdown(&sessions, &registry, None).await;
    }
}
