mod context;
mod peer_event_loop;
mod repl;
mod ui;

use context::DaemonContext;
use peer_event_loop::CliControlPlane;
use splitter_core::net::device_watcher;
use splitter_core::net::discovery::Discovery;
use splitter_core::net::signaling::client_ops::{find_conn_tx, notify_remote_control};
use splitter_core::net::signaling::{
    server::SignalingServer, PeerConnectionHandle, SignalingMessage, StreamAction,
};
use splitter_core::net::stream_runtime::{dispatch_device_events, StreamRegistry};
use splitter_core::net::trust::TrustStore;
use splitter_core::observability::metrics::MetricsRegistry;
use splitter_core::settings::{settings_path, Settings};
use splitter_core::{log_dir, PeerIdentity, SessionManager};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

pub(crate) async fn run(
    signaling_port: u16,
    peer_name_override: Option<String>,
    identity_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let settings_path_buf = settings_path()?;
    let settings_handle = Arc::new(RwLock::new(Settings::load_or_default(&settings_path_buf)?));

    let log_level = settings_handle.read().await.log_level;
    let _logs_guard = splitter_core::observability::logs::init(log_level, &log_dir()?)?;

    let base_dir = match identity_dir {
        Some(d) => d,
        None => dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("no config_dir available on this platform"))?
            .join("Splitter"),
    };
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

    // mDNS is best-effort: networks that block multicast (CI sandboxes, restricted
    // LANs, some containers) can't start it. Signaling still works over direct and
    // loopback connections, so a discovery failure must not down the daemon.
    let discovery = match Discovery::start(&identity, signaling_port) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!("mDNS discovery unavailable ({e}); continuing without LAN discovery");
            None
        }
    };
    let discovered = Arc::default();

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
                    splitter_core::observability::metrics::serve(metrics, metrics_port).await
                {
                    tracing::error!(?e, "metrics server exited");
                }
            });
        }
    }

    {
        let sh = settings_handle.clone();
        let sp = settings_path_buf.clone();
        tokio::spawn(async move {
            let mut last_mtime: Option<std::time::SystemTime> =
                std::fs::metadata(&sp).ok().and_then(|m| m.modified().ok());
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.tick().await; // consume the immediate first tick
            loop {
                interval.tick().await;
                let current = std::fs::metadata(&sp).ok().and_then(|m| m.modified().ok());
                if current != last_mtime && current.is_some() {
                    last_mtime = current;
                    match Settings::load_or_default(&sp) {
                        Ok(new_settings) => {
                            let old = sh.read().await.clone();
                            *sh.write().await = new_settings.clone();
                            #[allow(clippy::print_stdout)]
                            {
                                println!(">> settings reloaded");
                            }
                            // Warn about keys that require restart.
                            if old.log_level != new_settings.log_level {
                                #[allow(clippy::print_stdout)]
                                {
                                    println!(
                                        ">> setting 'log_level' changed; restart required to apply"
                                    );
                                }
                            }
                            if old.metrics_enabled != new_settings.metrics_enabled
                                || old.metrics_port != new_settings.metrics_port
                            {
                                #[allow(clippy::print_stdout)]
                                {
                                    println!(
                                        ">> setting 'metrics_enabled/metrics_port' changed; restart required to apply"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("settings reload failed: {e}");
                        }
                    }
                }
            }
        });
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

    let local_peer_id = identity.peer_id;
    let ctx = DaemonContext {
        identity,
        trust,
        sessions,
        stream_registry,
        discovered,
        outgoing_connections,
        local_peer_id,
    };

    // Spawn the shared control-plane loop for every newly established peer connection,
    // regardless of whether it came through the manual `accept` branch or the
    // auto-accept-trusted shortcut.
    splitter_core::net::signaling::spawn_connection_supervisor(
        server.connection_established_tx.subscribe(),
        server.connections.clone(),
        Arc::new(CliControlPlane { ctx: ctx.clone() }),
    );

    repl::run_repl(&ctx, server, discovery).await
}

// Ordered teardown sequence (non-obvious ordering rationale):
// 1. Send StreamControl{Close} + SessionResponse{accepted:false} to peers so the remote
//    side tears down cleanly before we drop the TCP connection.
// 2. Then close local stream runtimes (aborts pump tasks).
// 3. Sleep 150 ms: gives TCP framing time to flush the outgoing messages.
// 4. Close SessionManager entries (bookkeeping only at this point).
async fn graceful_shutdown(
    sessions: &Arc<SessionManager>,
    stream_registry: &Arc<StreamRegistry>,
    server: Option<&splitter_core::net::signaling::server::SignalingServerHandle>,
    outgoing: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
) {
    let session_snap = sessions.snapshot().await;

    // 1. Notify peers: close streams then close sessions.
    if let Some(srv) = server {
        for sess in &session_snap {
            let tx = find_conn_tx(&srv.connections, outgoing, sess.remote_peer_id).await;
            if let Some(tx) = tx {
                for stream in &sess.streams {
                    notify_remote_control(&tx, stream.id.get(), StreamAction::Close).await;
                }
                let _ = tx
                    .send(SignalingMessage::SessionResponse {
                        session_id: sess.id.to_string(),
                        accepted: false,
                    })
                    .await;
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

#[cfg(test)]
mod tests {
    use super::context::pick_default_output_device_id;
    use super::*;
    use splitter_core::settings::LogLevel;

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

    #[tokio::test]
    async fn graceful_shutdown_on_empty_state_does_not_panic() {
        let sessions = splitter_core::SessionManager::new();
        let registry = splitter_core::StreamRegistry::new();
        let outgoing = Arc::new(RwLock::new(HashMap::new()));
        graceful_shutdown(&sessions, &registry, None, &outgoing).await;
    }

    #[tokio::test]
    async fn acceptor_loop_shape_survives_lagged_receiver() {
        use tokio::sync::broadcast::{self, error::RecvError};

        let (tx, mut rx) = broadcast::channel::<Uuid>(2);
        for _ in 0..5 {
            let _ = tx.send(Uuid::new_v4());
        }
        let wanted = Uuid::new_v4();
        tx.send(wanted).unwrap();

        let mut delivered = None;
        loop {
            match rx.recv().await {
                Ok(id) => {
                    delivered = Some(id);
                    if id == wanted {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
        assert_eq!(
            delivered,
            Some(wanted),
            "loop must continue past a Lagged error and still deliver later values"
        );
    }

    #[tokio::test]
    async fn open_dedupe_returns_existing_session() {
        use splitter_core::net::session::SessionState;
        let sessions = splitter_core::SessionManager::new();
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let sid = sessions.open_outgoing(local, remote).await;
        sessions.accept(&sid).await.unwrap();

        let snap = sessions.snapshot().await;
        let existing = snap
            .into_iter()
            .find(|s| s.remote_peer_id == remote && s.state == SessionState::Active);
        assert!(existing.is_some(), "should find existing active session");
        assert_eq!(existing.unwrap().id, sid);
    }

    #[test]
    fn missing_config_dir_returns_err_via_ok_or_else() {
        let result: anyhow::Result<std::path::PathBuf> = None::<std::path::PathBuf>
            .ok_or_else(|| anyhow::anyhow!("no config_dir available on this platform"))
            .map(|d| d.join("Splitter"));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("no config_dir"),
            "error message must describe the missing config_dir"
        );
    }
}
