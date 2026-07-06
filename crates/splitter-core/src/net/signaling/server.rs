use crate::error::NetError;
use crate::net::identity::PeerIdentity;
use crate::net::manager::SessionManager;
use crate::net::signaling::connection::{spawn_peer_connection, PeerConnectionHandle, PeerEvent};
use crate::net::signaling::message::{Capabilities, SignalingMessage, PROTOCOL_VERSION};
use crate::net::trust::{TrustStore, TrustedPeer};
use crate::settings::SettingsHandle;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PendingPeer {
    pub peer_id: Uuid,
    pub peer_name: String,
    pub remote_addr: SocketAddr,
    pub proposed_token: String,
}

#[derive(Debug, Default)]
pub struct PendingPeers {
    inner: Mutex<Vec<PendingPeer>>,
}

impl PendingPeers {
    pub async fn list(&self) -> Vec<PendingPeer> {
        self.inner.lock().await.clone()
    }

    pub async fn take(&self, idx: usize) -> Option<PendingPeer> {
        let mut guard = self.inner.lock().await;
        if idx >= guard.len() {
            return None;
        }
        Some(guard.remove(idx))
    }

    pub async fn push(&self, p: PendingPeer) {
        self.inner.lock().await.push(p);
    }

    pub async fn remove_peer(&self, peer_id: &Uuid) -> bool {
        let mut guard = self.inner.lock().await;
        let before = guard.len();
        guard.retain(|p| &p.peer_id != peer_id);
        guard.len() != before
    }
}

#[derive(Debug)]
pub struct SignalingServerHandle {
    pub bind_addr: SocketAddr,
    pub pending: Arc<PendingPeers>,
    pub connections: Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    /// Fires once for every peer whose connection is fully established (both
    /// the auto-accept-trusted path and the manual `accept` path).
    pub connection_established_tx: broadcast::Sender<Uuid>,
}

pub struct SignalingServer;

impl SignalingServer {
    pub async fn start(
        bind: SocketAddr,
        identity: PeerIdentity,
        trust: Arc<RwLock<TrustStore>>,
        sessions: Arc<SessionManager>,
        settings: SettingsHandle,
    ) -> Result<SignalingServerHandle, NetError> {
        let listener = TcpListener::bind(bind).await.map_err(NetError::UdpIo)?;
        let bind_addr = listener.local_addr().map_err(NetError::UdpIo)?;
        let pending: Arc<PendingPeers> = Arc::new(PendingPeers::default());
        let connections: Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>> = Arc::default();
        let (conn_est_tx, _) = broadcast::channel::<Uuid>(32);

        let p_clone = pending.clone();
        let c_clone = connections.clone();
        let t_clone = trust.clone();
        let s_clone = settings;
        let id_clone = identity.clone();
        let _sessions = sessions;
        let conn_est_tx_task = conn_est_tx.clone();

        tokio::spawn(async move {
            loop {
                let (stream, addr) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("signaling accept: {e}");
                        continue;
                    }
                };
                let handle = match spawn_peer_connection(stream, None) {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::warn!(error = %e, "dropping accepted connection: peer_addr unavailable");
                        continue;
                    }
                };
                let mut events = handle.events.subscribe();
                let p_inner = p_clone.clone();
                let c_inner = c_clone.clone();
                let t_inner = t_clone.clone();
                let s_inner = s_clone.clone();
                let id_inner = id_clone.clone();
                let conn_est_tx_inner = conn_est_tx_task.clone();
                tokio::spawn(async move {
                    let first = match tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        events.recv(),
                    )
                    .await
                    {
                        Ok(Ok(PeerEvent::Message(m))) => m,
                        _ => {
                            tracing::warn!("no HELLO from {addr} within 5s");
                            return;
                        }
                    };
                    let SignalingMessage::Hello {
                        protocol_version,
                        peer_id,
                        peer_name,
                        auth_token,
                        ..
                    } = first
                    else {
                        let _ = handle
                            .tx
                            .send(SignalingMessage::HelloAck {
                                accepted: false,
                                reason: Some("first message must be hello".into()),
                                auth_token: None,
                                peer_id: None,
                            })
                            .await;
                        return;
                    };
                    if protocol_version != PROTOCOL_VERSION {
                        let _ = handle
                            .tx
                            .send(SignalingMessage::HelloAck {
                                accepted: false,
                                reason: Some(format!(
                                    "protocol_version mismatch: got {protocol_version}, expected {PROTOCOL_VERSION}"
                                )),
                                auth_token: None,
                                peer_id: None,
                            })
                            .await;
                        return;
                    }
                    let Ok(peer_uuid) = Uuid::parse_str(&peer_id) else {
                        let _ = handle
                            .tx
                            .send(SignalingMessage::HelloAck {
                                accepted: false,
                                reason: Some("invalid peer_id uuid".into()),
                                auth_token: None,
                                peer_id: None,
                            })
                            .await;
                        return;
                    };

                    let (known, token_valid) = {
                        let t = t_inner.read().await;
                        (t.contains(&peer_uuid), t.verify(&peer_uuid, &auth_token))
                    };
                    if known {
                        if !token_valid {
                            let _ = handle
                                .tx
                                .send(SignalingMessage::HelloAck {
                                    accepted: false,
                                    reason: Some("auth_token mismatch".into()),
                                    auth_token: None,
                                    peer_id: None,
                                })
                                .await;
                            return;
                        }
                        let auto_accept = s_inner.read().await.auto_accept_trusted;
                        if auto_accept {
                            let _ = handle
                                .tx
                                .send(SignalingMessage::HelloAck {
                                    accepted: true,
                                    reason: None,
                                    auth_token: Some(auth_token.clone()),
                                    peer_id: Some(id_inner.peer_id.to_string()),
                                })
                                .await;
                            c_inner.write().await.insert(peer_uuid, handle);
                            let _ = conn_est_tx_inner.send(peer_uuid);
                            return;
                        }
                        // auto_accept_trusted is false — fall through to pending queue
                    }

                    p_inner
                        .push(PendingPeer {
                            peer_id: peer_uuid,
                            peer_name: peer_name.clone(),
                            remote_addr: addr,
                            proposed_token: auth_token.clone(),
                        })
                        .await;
                    let mut pre_accept_events = handle.events.subscribe();
                    let pending_for_watch = p_inner.clone();
                    let conns_for_watch = c_inner.clone();
                    c_inner.write().await.insert(peer_uuid, handle);
                    tokio::spawn(async move {
                        loop {
                            match pre_accept_events.recv().await {
                                Ok(PeerEvent::Disconnected { .. })
                                | Err(broadcast::error::RecvError::Closed) => {
                                    // Once accepted, `accept_pending_as` has taken the peer out
                                    // of pending and the acceptor owns cleanup; only evict while
                                    // still pending.
                                    if pending_for_watch.remove_peer(&peer_uuid).await {
                                        conns_for_watch.write().await.remove(&peer_uuid);
                                    }
                                    break;
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Ok(_) => {}
                            }
                        }
                    });
                });
            }
        });

        Ok(SignalingServerHandle {
            bind_addr,
            pending,
            connections,
            connection_established_tx: conn_est_tx,
        })
    }
}

pub fn generate_auth_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    B64.encode(buf)
}

pub async fn accept_pending(
    pending: &PendingPeers,
    trust: &Arc<RwLock<TrustStore>>,
    connections: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    conn_established_tx: &broadcast::Sender<Uuid>,
    idx: usize,
) -> Result<(Uuid, String), NetError> {
    accept_pending_as(pending, trust, connections, conn_established_tx, idx, None).await
}

pub async fn accept_pending_as(
    pending: &PendingPeers,
    trust: &Arc<RwLock<TrustStore>>,
    connections: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    conn_established_tx: &broadcast::Sender<Uuid>,
    idx: usize,
    server_peer_id: Option<Uuid>,
) -> Result<(Uuid, String), NetError> {
    let p = pending
        .take(idx)
        .await
        .ok_or_else(|| NetError::SignalingProtocol {
            reason: format!("no pending peer at index {idx}"),
        })?;
    let token = generate_auth_token();
    {
        let mut t = trust.write().await;
        t.add(TrustedPeer {
            peer_id: p.peer_id,
            peer_name: p.peer_name.clone(),
            auth_token: token.clone(),
        })?;
    }
    let conns = connections.read().await;
    if let Some(handle) = conns.get(&p.peer_id) {
        handle
            .tx
            .send(SignalingMessage::HelloAck {
                accepted: true,
                reason: None,
                auth_token: Some(token.clone()),
                peer_id: server_peer_id.map(|id| id.to_string()),
            })
            .await
            .map_err(|_| NetError::SignalingProtocol {
                reason: "peer disconnected before ack".into(),
            })?;
    }
    let _ = conn_established_tx.send(p.peer_id);
    Ok((p.peer_id, token))
}

pub fn local_capabilities() -> Capabilities {
    Capabilities {
        codecs: vec!["opus".into()],
        max_streams: 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::signaling::message::PROTOCOL_VERSION;
    use crate::net::trust::MIN_AUTH_TOKEN_LEN;
    use crate::settings::Settings;
    use tempfile::tempdir;
    use tokio::net::TcpStream;

    async fn setup() -> (
        SignalingServerHandle,
        PeerIdentity,
        Arc<RwLock<TrustStore>>,
        Arc<SessionManager>,
        tempfile::TempDir,
    ) {
        setup_with_settings(Settings::default()).await
    }

    async fn setup_with_settings(
        settings: Settings,
    ) -> (
        SignalingServerHandle,
        PeerIdentity,
        Arc<RwLock<TrustStore>>,
        Arc<SessionManager>,
        tempfile::TempDir,
    ) {
        let dir = tempdir().unwrap();
        let identity = PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: "server".into(),
        };
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        let settings_handle = Arc::new(RwLock::new(settings));
        let handle = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            identity.clone(),
            trust.clone(),
            sessions.clone(),
            settings_handle,
        )
        .await
        .unwrap();
        (handle, identity, trust, sessions, dir)
    }

    #[tokio::test]
    async fn server_queues_unknown_peer_hello() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        let client_peer_id = Uuid::new_v4();
        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: client_peer_id.to_string(),
                peer_name: "client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: "proposed-tok".into(),
            })
            .await
            .unwrap();

        let mut ok = false;
        for _ in 0..50 {
            if !server.pending.list().await.is_empty() {
                ok = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ok, "server never queued the pending peer");
        let pending = server.pending.list().await;
        assert_eq!(pending[0].peer_id, client_peer_id);
    }

    #[tokio::test]
    async fn server_rejects_protocol_mismatch() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        let mut events = client.events.subscribe();
        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: 99,
                peer_id: Uuid::new_v4().to_string(),
                peer_name: "client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: "tok".into(),
            })
            .await
            .unwrap();
        let ack = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(PeerEvent::Message(SignalingMessage::HelloAck {
                    accepted, reason, ..
                })) = events.recv().await
                {
                    return (accepted, reason);
                }
            }
        })
        .await
        .unwrap();
        assert!(!ack.0);
        assert!(ack.1.unwrap().contains("protocol_version"));
    }

    #[tokio::test]
    async fn accept_pending_promotes_and_acks() {
        let (server, _identity, trust, _sessions, _dir) = setup().await;
        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        let mut events = client.events.subscribe();
        let peer_id = Uuid::new_v4();
        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: peer_id.to_string(),
                peer_name: "client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: "proposed".into(),
            })
            .await
            .unwrap();
        for _ in 0..50 {
            if !server.pending.list().await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let (accepted_id, token) = accept_pending(
            &server.pending,
            &trust,
            &server.connections,
            &server.connection_established_tx,
            0,
        )
        .await
        .unwrap();
        assert_eq!(accepted_id, peer_id);
        assert!(!token.is_empty(), "minted token must not be empty");
        assert!(token.len() >= MIN_AUTH_TOKEN_LEN);
        assert_ne!(
            token, "proposed",
            "server must mint its own token, not echo the client-proposed value"
        );
        let ack = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(PeerEvent::Message(SignalingMessage::HelloAck { accepted, .. })) =
                    events.recv().await
                {
                    return accepted;
                }
            }
        })
        .await
        .unwrap();
        assert!(ack);
    }

    #[tokio::test]
    async fn hello_from_trusted_peer_with_auto_accept_skips_pending_queue() {
        let settings = Settings {
            auto_accept_trusted: true,
            ..Settings::default()
        };
        let (server, _identity, trust, _sessions, _dir) = setup_with_settings(settings).await;

        let peer_id = Uuid::new_v4();
        let token = generate_auth_token();
        trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id,
                peer_name: "trusted-client".into(),
                auth_token: token.clone(),
            })
            .unwrap();

        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        let mut events = client.events.subscribe();

        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: peer_id.to_string(),
                peer_name: "trusted-client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: token.clone(),
            })
            .await
            .unwrap();

        // Should receive HelloAck{accepted:true} directly — no manual accept step
        let ack = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(PeerEvent::Message(SignalingMessage::HelloAck { accepted, .. })) =
                    events.recv().await
                {
                    return accepted;
                }
            }
        })
        .await
        .expect("timed out waiting for HelloAck");
        assert!(
            ack,
            "trusted peer with auto_accept_trusted=true must be auto-accepted"
        );

        // Pending queue must remain empty
        assert!(
            server.pending.list().await.is_empty(),
            "trusted auto-accepted peer must not appear in pending queue"
        );
    }

    #[tokio::test]
    async fn hello_from_trusted_peer_without_auto_accept_goes_to_pending() {
        let off = Settings {
            auto_accept_trusted: false,
            ..Settings::default()
        };
        let (server, _identity, trust, _sessions, _dir) = setup_with_settings(off).await;

        let peer_id = Uuid::new_v4();
        let token = generate_auth_token();
        trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id,
                peer_name: "trusted-client".into(),
                auth_token: token.clone(),
            })
            .unwrap();

        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();

        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: peer_id.to_string(),
                peer_name: "trusted-client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: token.clone(),
            })
            .await
            .unwrap();

        // With auto_accept_trusted=false, trusted peer should land in pending
        let mut queued = false;
        for _ in 0..50 {
            if !server.pending.list().await.is_empty() {
                queued = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            queued,
            "trusted peer with auto_accept_trusted=false must still appear in pending queue"
        );
        let pending = server.pending.list().await;
        assert_eq!(pending[0].peer_id, peer_id);
    }

    #[tokio::test]
    async fn trusted_peer_bad_token_rejected_not_queued() {
        let settings = Settings {
            auto_accept_trusted: true,
            ..Settings::default()
        };
        let (server, _identity, trust, _sessions, _dir) = setup_with_settings(settings).await;

        let peer_id = Uuid::new_v4();
        trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id,
                peer_name: "trusted-client".into(),
                auth_token: generate_auth_token(),
            })
            .unwrap();

        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        let mut events = client.events.subscribe();

        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: peer_id.to_string(),
                peer_name: "trusted-client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: "wrong-tok".into(),
            })
            .await
            .unwrap();

        let ack = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(PeerEvent::Message(SignalingMessage::HelloAck {
                    accepted, reason, ..
                })) = events.recv().await
                {
                    return (accepted, reason);
                }
            }
        })
        .await
        .expect("timed out waiting for HelloAck");

        assert!(!ack.0, "known peer with wrong token must be rejected");
        assert!(
            ack.1
                .as_deref()
                .unwrap_or("")
                .contains("auth_token mismatch"),
            "rejection reason must mention auth_token mismatch, got: {:?}",
            ack.1
        );
        assert!(
            server.pending.list().await.is_empty(),
            "rejected peer must not appear in pending queue"
        );
    }

    async fn queue_hello_with_empty_token(
        server: &SignalingServerHandle,
        peer_id: Uuid,
    ) -> PeerConnectionHandle {
        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: peer_id.to_string(),
                peer_name: "client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: String::new(),
            })
            .await
            .unwrap();
        for _ in 0..50 {
            if !server.pending.list().await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        client
    }

    #[tokio::test]
    async fn accept_mints_non_empty_high_entropy_token() {
        let (server, _identity, trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let _client = queue_hello_with_empty_token(&server, peer_id).await;

        let (accepted_id, token) = accept_pending(
            &server.pending,
            &trust,
            &server.connections,
            &server.connection_established_tx,
            0,
        )
        .await
        .unwrap();
        assert_eq!(accepted_id, peer_id);
        assert!(!token.is_empty(), "minted token must not be empty");
        assert!(token.len() >= MIN_AUTH_TOKEN_LEN);
    }

    #[tokio::test]
    async fn empty_proposed_token_is_not_the_stored_credential() {
        let (server, _identity, trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let _client = queue_hello_with_empty_token(&server, peer_id).await;

        let (_accepted_id, token) = accept_pending(
            &server.pending,
            &trust,
            &server.connections,
            &server.connection_established_tx,
            0,
        )
        .await
        .unwrap();

        let t = trust.read().await;
        assert!(
            !t.verify(&peer_id, ""),
            "empty client-proposed token must not become the stored credential"
        );
        assert!(t.verify(&peer_id, &token), "the minted token must verify");
    }

    #[tokio::test]
    async fn pending_peers_remove_peer_removes_matching() {
        let pending = PendingPeers::default();
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        pending
            .push(PendingPeer {
                peer_id: p1,
                peer_name: "a".into(),
                remote_addr: addr,
                proposed_token: String::new(),
            })
            .await;
        pending
            .push(PendingPeer {
                peer_id: p2,
                peer_name: "b".into(),
                remote_addr: addr,
                proposed_token: String::new(),
            })
            .await;

        assert!(
            pending.remove_peer(&p1).await,
            "removing p1 must report true"
        );
        assert!(
            !pending.remove_peer(&p1).await,
            "removing an absent peer must report false"
        );
        let list = pending.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].peer_id, p2, "the other peer must survive");
    }

    #[tokio::test]
    async fn pre_accept_drop_evicts_pending_and_connections() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream, None).unwrap();
        let client_peer_id = Uuid::new_v4();
        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                peer_id: client_peer_id.to_string(),
                peer_name: "client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                auth_token: String::new(),
            })
            .await
            .unwrap();

        let mut queued = false;
        for _ in 0..50 {
            if !server.pending.list().await.is_empty()
                && server
                    .connections
                    .read()
                    .await
                    .contains_key(&client_peer_id)
            {
                queued = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(queued, "peer must be queued in pending and connections");

        drop(client);

        let mut evicted = false;
        for _ in 0..50 {
            if server.pending.list().await.is_empty()
                && !server
                    .connections
                    .read()
                    .await
                    .contains_key(&client_peer_id)
            {
                evicted = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            evicted,
            "dropped pre-accept connection must be evicted from pending and connections"
        );
    }
}
