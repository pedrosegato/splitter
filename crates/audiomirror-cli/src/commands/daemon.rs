use super::stream_repl;
use audiomirror_core::audio::devices::{list_devices, DeviceKind};
use audiomirror_core::config::identity_path;
use audiomirror_core::net::device_watcher;
use audiomirror_core::net::discovery::{Discovery, DiscoveryEvent};
use audiomirror_core::net::signaling::{
    connect_to_peer, server::accept_pending, server::SignalingServer, CodecParams, Endpoint,
    PeerEvent, SignalingMessage,
};
use audiomirror_core::net::stream_runtime::{
    dispatch_device_events, open_stream_as_sink, StreamRegistry,
};
use audiomirror_core::net::trust::{trust_store_path, TrustStore};
use audiomirror_core::observability::metrics::MetricsRegistry;
use audiomirror_core::settings::{settings_path, Settings};
use audiomirror_core::{PeerIdentity, SessionManager, StreamRoute};
use std::collections::HashMap;
use std::net::SocketAddr;
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
) -> anyhow::Result<()> {
    let id_path = identity_path()?;
    let mut identity = PeerIdentity::load_or_create(&id_path)?;
    if let Some(name) = peer_name_override {
        identity.peer_name = name;
    }
    let trust = Arc::new(RwLock::new(TrustStore::load_or_create(
        &trust_store_path()?
    )?));
    let settings_handle = Arc::new(RwLock::new(Settings::load_or_default(&settings_path()?)?));
    let settings = settings_handle.clone();
    let sessions = SessionManager::new();
    let stream_registry = StreamRegistry::new();

    // Opt-in Prometheus metrics endpoint.
    {
        let s = settings_handle.read().await;
        if s.metrics_enabled {
            let metrics = Arc::new(MetricsRegistry::new()?);
            let metrics_port = s.metrics_port;

            // 1 s interval task: update peers_connected + sessions_active.
            let metrics_tick = metrics.clone();
            let sessions_tick = sessions.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                loop {
                    interval.tick().await;
                    let snap = sessions_tick.snapshot().await;
                    metrics_tick.sessions_active.set(snap.len() as f64);
                    // peers_connected = distinct remote peer IDs across all sessions
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

    let bind: SocketAddr = format!("0.0.0.0:{signaling_port}").parse()?;
    let server = SignalingServer::start(
        bind,
        identity.clone(),
        trust.clone(),
        sessions.clone(),
        settings,
    )
    .await?;

    let watcher = device_watcher::start(Duration::from_secs(5));
    let dispatcher_rx = watcher.subscribe();
    tokio::spawn(dispatch_device_events(
        stream_registry.clone(),
        dispatcher_rx,
    ));

    let mut discovery = Discovery::start(&identity, signaling_port)?;
    let discovered: Arc<RwLock<HashMap<String, audiomirror_core::net::discovery::DiscoveredPeer>>> =
        Arc::default();
    let discovered_clone = discovered.clone();
    tokio::spawn(async move {
        loop {
            match discovery.next_event().await {
                Some(DiscoveryEvent::Found(p)) => {
                    discovered_clone.write().await.insert(p.peer_id.clone(), p);
                }
                Some(DiscoveryEvent::Removed(name)) => {
                    tracing::info!("peer removed: {name}");
                }
                None => break,
            }
        }
    });

    tracing::info!(
        peer_id = %identity.peer_id,
        peer_name = %identity.peer_name,
        bind = %server.bind_addr,
        "daemon ready; type 'help' for commands"
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let cmd = line.trim().to_string();
        if cmd.is_empty() {
            continue;
        }
        if let Err(e) = handle_line(
            &cmd,
            &identity,
            &trust,
            &sessions,
            &stream_registry,
            &server,
            &discovered,
        )
        .await
        {
            tracing::error!("command failed: {e}");
        }
        if cmd == "quit" {
            break;
        }
    }
    Ok(())
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
    let mut parts = line.split_whitespace();
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
            tracing::info!("shutting down");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_default_output_device_id_returns_output_kind_or_none() {
        if let Some(id) = pick_default_output_device_id() {
            assert!(
                id.starts_with("Output:"),
                "expected Output: prefix, got {id}"
            );
        }
    }
}
