use crate::error::NetError;
use crate::net::crypto::{self, KeyExchange, Transcript};
use crate::net::identity::PeerIdentity;
use crate::net::manager::SessionManager;
use crate::net::signaling::connection::{
    spawn_peer_connection, PeerConnectionHandle, PeerEvent, Role,
};
use crate::net::signaling::message::{Capabilities, SignalingMessage, PROTOCOL_VERSION};
use crate::net::trust::{TrustStore, TrustedPeer};
use crate::settings::SettingsHandle;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex, Notify, RwLock, Semaphore};
use uuid::Uuid;

// Bounds what unauthenticated LAN hosts can hold open while waiting for the user.
pub const MAX_PENDING_PEERS: usize = 16;
const MAX_CONCURRENT_HANDSHAKES: usize = 64;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct PendingPeer {
    pub peer_id: Uuid,
    pub peer_name: String,
    pub remote_addr: SocketAddr,
    /// Lets a reconnect replace the request without the old connection's teardown evicting it.
    pub connection_id: Uuid,
    /// Shown on both machines for a first-time pairing; `None` when the peer proved an
    /// existing pairing and only needs to be let in.
    pub pairing_code: Option<String>,
    /// Stored in the trust store on accept.
    pub token: String,
}

impl std::fmt::Debug for PendingPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingPeer")
            .field("peer_id", &self.peer_id)
            .field("peer_name", &self.peer_name)
            .field("remote_addr", &self.remote_addr)
            .field("connection_id", &self.connection_id)
            .field("pairing_code", &self.pairing_code)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct PendingPeers {
    inner: Mutex<Vec<PendingPeer>>,
    changed: Notify,
}

impl PendingPeers {
    pub async fn list(&self) -> Vec<PendingPeer> {
        self.inner.lock().await.clone()
    }

    /// Resolves after the next change; changes made while nobody waits are coalesced.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }

    /// By peer, not list position, so an accept can't land on whoever shifted into a slot.
    pub async fn take(&self, peer_id: &Uuid) -> Option<PendingPeer> {
        let mut guard = self.inner.lock().await;
        let idx = guard.iter().position(|p| &p.peer_id == peer_id)?;
        self.changed.notify_one();
        Some(guard.remove(idx))
    }

    /// Returns false when the queue is full.
    pub async fn upsert(&self, p: PendingPeer) -> bool {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.iter_mut().find(|e| e.peer_id == p.peer_id) {
            *existing = p;
        } else if guard.len() >= MAX_PENDING_PEERS {
            return false;
        } else {
            guard.push(p);
        }
        self.changed.notify_one();
        true
    }

    /// Drops the request only if it still belongs to `connection_id`.
    pub async fn remove_connection(&self, peer_id: &Uuid, connection_id: Uuid) -> bool {
        self.remove_where(|p| &p.peer_id == peer_id && p.connection_id == connection_id)
            .await
    }

    pub async fn remove_peer(&self, peer_id: &Uuid) -> bool {
        self.remove_where(|p| &p.peer_id == peer_id).await
    }

    async fn remove_where(&self, matches: impl Fn(&PendingPeer) -> bool) -> bool {
        let mut guard = self.inner.lock().await;
        let before = guard.len();
        guard.retain(|p| !matches(p));
        let removed = guard.len() != before;
        if removed {
            self.changed.notify_one();
        }
        removed
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

/// Skips heartbeats: the dialer's connection task can send one before its Hello.
async fn next_handshake_message(
    events: &mut broadcast::Receiver<PeerEvent>,
) -> Option<SignalingMessage> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(SignalingMessage::Heartbeat { .. }))
                | Ok(PeerEvent::Connected { .. })
                | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Ok(PeerEvent::Message(m)) => return Some(m),
                Ok(PeerEvent::Disconnected { .. }) | Err(broadcast::error::RecvError::Closed) => {
                    return None
                }
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn reject(handle: PeerConnectionHandle, reason: &str) {
    handle.disarm();
    let _ = handle
        .tx
        .send(SignalingMessage::HelloAck {
            accepted: false,
            reason: Some(reason.into()),
        })
        .await;
}

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
        let handshakes = Arc::new(Semaphore::new(MAX_CONCURRENT_HANDSHAKES));

        tokio::spawn(async move {
            loop {
                let (stream, addr) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("signaling accept: {e}");
                        // Persistent failures (fd exhaustion) return instantly.
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(permit) = handshakes.clone().try_acquire_owned() else {
                    tracing::warn!(
                        "too many concurrent handshakes; dropping connection from {addr}"
                    );
                    continue;
                };
                let handle = match spawn_peer_connection(stream) {
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
                    let _permit = permit;
                    let Some(first) = next_handshake_message(&mut events).await else {
                        tracing::warn!("no HELLO from {addr} within {HANDSHAKE_TIMEOUT:?}");
                        return;
                    };
                    if let SignalingMessage::Probe {} = first {
                        handle.disarm();
                        let _ = handle
                            .tx
                            .send(SignalingMessage::ProbeAck {
                                peer_id: id_inner.peer_id.to_string(),
                                peer_name: id_inner.peer_name.clone(),
                                app_version: env!("CARGO_PKG_VERSION").into(),
                            })
                            .await;
                        return;
                    }
                    let SignalingMessage::Hello {
                        protocol_version,
                        peer_id,
                        peer_name,
                        client_nonce,
                        client_key,
                        ..
                    } = first
                    else {
                        reject(handle, "first message must be hello").await;
                        return;
                    };
                    if protocol_version != PROTOCOL_VERSION {
                        reject(
                            handle,
                            &format!(
                                "protocol_version mismatch: got {protocol_version}, expected {PROTOCOL_VERSION}"
                            ),
                        )
                        .await;
                        return;
                    }
                    let Ok(peer_uuid) = Uuid::parse_str(&peer_id) else {
                        reject(handle, "invalid peer_id uuid").await;
                        return;
                    };

                    let exchange = KeyExchange::new();
                    let server_key = exchange.public_key().to_string();
                    let server_nonce = crypto::new_nonce();
                    let server_peer_id = id_inner.peer_id.to_string();
                    let transcript = Transcript {
                        client_peer_id: &peer_id,
                        server_peer_id: &server_peer_id,
                        client_nonce: &client_nonce,
                        server_nonce: &server_nonce,
                        client_key: &client_key,
                        server_key: &server_key,
                    };
                    let Some(shared) = exchange.agree(&client_key) else {
                        reject(handle, "invalid key exchange").await;
                        return;
                    };
                    let stored_token = t_inner.read().await.token_for(Some(peer_uuid));
                    let (token, pairing_code, offer) = match stored_token {
                        Some(token) => {
                            let server_proof = crypto::server_proof(&token, &transcript);
                            let challenge = SignalingMessage::Challenge {
                                server_peer_id: server_peer_id.clone(),
                                server_nonce: server_nonce.clone(),
                                server_key: server_key.clone(),
                                server_proof,
                            };
                            (token, None, challenge)
                        }
                        None => (
                            crypto::pairing_token(&shared, &transcript),
                            Some(crypto::pairing_code(&shared, &transcript)),
                            SignalingMessage::PairingOffer {
                                server_peer_id: server_peer_id.clone(),
                                server_nonce: server_nonce.clone(),
                                server_key: server_key.clone(),
                            },
                        ),
                    };
                    let expected = crypto::client_proof(&token, &transcript);
                    handle.install_session(
                        crypto::session_keys(&shared, &token, &transcript),
                        Role::Acceptor,
                    );
                    let _ = handle.tx.send(offer).await;
                    let proven = matches!(
                        next_handshake_message(&mut events).await,
                        Some(SignalingMessage::ChallengeResponse { client_proof })
                            if crypto::proofs_match(&expected, &client_proof)
                    );
                    if !proven {
                        tracing::warn!(peer = %peer_uuid, %addr, "handshake proof failed");
                        reject(handle, "authentication failed").await;
                        return;
                    }
                    handle.start_encrypting();

                    if pairing_code.is_none() && s_inner.read().await.auto_accept_trusted {
                        let _ = handle
                            .tx
                            .send(SignalingMessage::HelloAck {
                                accepted: true,
                                reason: None,
                            })
                            .await;
                        c_inner.write().await.insert(peer_uuid, handle);
                        let _ = conn_est_tx_inner.send(peer_uuid);
                        return;
                    }

                    let connection_id = handle.connection_id;
                    let queued = p_inner
                        .upsert(PendingPeer {
                            peer_id: peer_uuid,
                            peer_name: peer_name.clone(),
                            remote_addr: addr,
                            connection_id,
                            pairing_code,
                            token,
                        })
                        .await;
                    if !queued {
                        reject(handle, "too many pending pairing requests").await;
                        return;
                    }
                    let mut pre_accept_events = handle.events.subscribe();
                    let pending_for_watch = p_inner.clone();
                    let conns_for_watch = c_inner.clone();
                    // Replaces (and aborts) an older connection from the same pending peer.
                    c_inner.write().await.insert(peer_uuid, handle);
                    tokio::spawn(async move {
                        loop {
                            match pre_accept_events.recv().await {
                                Ok(PeerEvent::Disconnected { .. })
                                | Err(broadcast::error::RecvError::Closed) => {
                                    // Once accepted, `accept_pending_as` has taken the peer out
                                    // of pending and the acceptor owns cleanup; only evict while
                                    // this connection's request is still pending.
                                    if pending_for_watch
                                        .remove_connection(&peer_uuid, connection_id)
                                        .await
                                    {
                                        let mut conns = conns_for_watch.write().await;
                                        if conns
                                            .get(&peer_uuid)
                                            .is_some_and(|h| h.connection_id == connection_id)
                                        {
                                            conns.remove(&peer_uuid);
                                        }
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

pub async fn accept_pending(
    pending: &PendingPeers,
    trust: &Arc<RwLock<TrustStore>>,
    connections: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    conn_established_tx: &broadcast::Sender<Uuid>,
    peer_id: Uuid,
) -> Result<(), NetError> {
    let p = pending
        .take(&peer_id)
        .await
        .ok_or_else(|| NetError::SignalingProtocol {
            reason: format!("no pending request from peer {peer_id}"),
        })?;
    let disconnected = || NetError::SignalingProtocol {
        reason: "peer disconnected before ack".into(),
    };
    let tx = connections
        .read()
        .await
        .get(&p.peer_id)
        .filter(|h| h.connection_id == p.connection_id)
        .map(|h| h.tx.clone())
        .ok_or_else(disconnected)?;
    trust.write().await.add(TrustedPeer {
        peer_id: p.peer_id,
        peer_name: p.peer_name.clone(),
        auth_token: p.token.clone(),
    })?;
    tx.send(SignalingMessage::HelloAck {
        accepted: true,
        reason: None,
    })
    .await
    .map_err(|_| disconnected())?;
    let _ = conn_established_tx.send(p.peer_id);
    Ok(())
}

/// Tells the dialer right away instead of leaving it waiting out the pairing timeout.
pub async fn reject_pending(
    pending: &PendingPeers,
    connections: &Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    peer_id: Uuid,
) -> bool {
    let Some(p) = pending.take(&peer_id).await else {
        return false;
    };
    let handle = {
        let mut conns = connections.write().await;
        if conns
            .get(&peer_id)
            .is_some_and(|h| h.connection_id == p.connection_id)
        {
            conns.remove(&peer_id)
        } else {
            None
        }
    };
    if let Some(handle) = handle {
        reject(handle, "pairing rejected").await;
    }
    true
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
    use base64::Engine;
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

    fn hello(peer_id: Uuid, client_key: &str) -> SignalingMessage {
        SignalingMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            peer_id: peer_id.to_string(),
            peer_name: "client".into(),
            app_version: "0".into(),
            capabilities: local_capabilities(),
            client_nonce: "cn".into(),
            client_key: client_key.into(),
        }
    }

    struct Dialer {
        _conn: PeerConnectionHandle,
        events: broadcast::Receiver<PeerEvent>,
        server_peer_id: String,
        token: String,
        code: Option<String>,
    }

    /// Plays the dialer: answers a Challenge with `known_token`, or a PairingOffer with
    /// the token derived from the exchange.
    async fn handshake(
        server: &SignalingServerHandle,
        peer_id: Uuid,
        known_token: Option<&str>,
        forge_proof: bool,
    ) -> Dialer {
        let (conn, mut events) = dial(server).await;
        let exchange = KeyExchange::new();
        let client_key = exchange.public_key().to_string();
        conn.tx.send(hello(peer_id, &client_key)).await.unwrap();
        let offer = next_msg(&mut events, |m| {
            matches!(
                m,
                SignalingMessage::Challenge { .. } | SignalingMessage::PairingOffer { .. }
            )
        })
        .await;
        let (server_peer_id, server_nonce, server_key, server_proof) = match offer {
            SignalingMessage::Challenge {
                server_peer_id,
                server_nonce,
                server_key,
                server_proof,
            } => (server_peer_id, server_nonce, server_key, Some(server_proof)),
            SignalingMessage::PairingOffer {
                server_peer_id,
                server_nonce,
                server_key,
            } => (server_peer_id, server_nonce, server_key, None),
            _ => unreachable!(),
        };
        let my_id = peer_id.to_string();
        let transcript = Transcript {
            client_peer_id: &my_id,
            server_peer_id: &server_peer_id,
            client_nonce: "cn",
            server_nonce: &server_nonce,
            client_key: &client_key,
            server_key: &server_key,
        };
        let shared = exchange.agree(&server_key).unwrap();
        let (token, code) = match server_proof {
            Some(proof) => {
                let token = known_token.expect("Challenge sent to an unknown peer");
                assert!(
                    crypto::proofs_match(&crypto::server_proof(token, &transcript), &proof),
                    "server must prove it holds the pairing token"
                );
                (token.to_string(), None)
            }
            None => (
                crypto::pairing_token(&shared, &transcript),
                Some(crypto::pairing_code(&shared, &transcript)),
            ),
        };
        conn.install_session(
            crypto::session_keys(&shared, &token, &transcript),
            Role::Dialer,
        );
        conn.start_encrypting();
        let client_proof = if forge_proof {
            "forged".into()
        } else {
            crypto::client_proof(&token, &transcript)
        };
        conn.tx
            .send(SignalingMessage::ChallengeResponse { client_proof })
            .await
            .unwrap();
        Dialer {
            _conn: conn,
            events,
            server_peer_id,
            token,
            code,
        }
    }

    async fn pair(server: &SignalingServerHandle, peer_id: Uuid) -> Dialer {
        handshake(server, peer_id, None, false).await
    }

    async fn hello_ack(events: &mut broadcast::Receiver<PeerEvent>) -> (bool, Option<String>) {
        let SignalingMessage::HelloAck { accepted, reason } = next_msg(events, is_hello_ack).await
        else {
            unreachable!()
        };
        (accepted, reason)
    }

    async fn dial(
        server: &SignalingServerHandle,
    ) -> (PeerConnectionHandle, broadcast::Receiver<PeerEvent>) {
        let stream = TcpStream::connect(server.bind_addr).await.unwrap();
        let client = spawn_peer_connection(stream).unwrap();
        let events = client.events.subscribe();
        (client, events)
    }

    async fn next_msg(
        events: &mut broadcast::Receiver<PeerEvent>,
        wanted: impl Fn(&SignalingMessage) -> bool,
    ) -> SignalingMessage {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(PeerEvent::Message(m)) = events.recv().await {
                    if wanted(&m) {
                        return m;
                    }
                }
            }
        })
        .await
        .expect("expected message not received within 2s")
    }

    fn is_hello_ack(m: &SignalingMessage) -> bool {
        matches!(m, SignalingMessage::HelloAck { .. })
    }

    async fn wait_pending(server: &SignalingServerHandle, n: usize) -> Vec<PendingPeer> {
        for _ in 0..50 {
            let list = server.pending.list().await;
            if list.len() >= n {
                return list;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("server never queued {n} pending peer(s)");
    }

    async fn trust_peer(trust: &Arc<RwLock<TrustStore>>, peer_id: Uuid) -> String {
        let token = "t".repeat(43);
        trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id,
                peer_name: "trusted-client".into(),
                auth_token: token.clone(),
            })
            .unwrap();
        token
    }

    #[tokio::test]
    async fn unknown_peer_gets_a_pairing_offer_and_both_sides_see_the_same_code() {
        let (server, identity, _trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let dialer = pair(&server, peer_id).await;

        let pending = wait_pending(&server, 1).await;
        assert_eq!(pending[0].peer_id, peer_id);
        assert_eq!(dialer.server_peer_id, identity.peer_id.to_string());
        let code = pending[0]
            .pairing_code
            .clone()
            .expect("first pairing shows a code");
        assert_eq!(Some(code), dialer.code);
        assert_eq!(pending[0].token, dialer.token);
    }

    #[tokio::test]
    async fn unknown_peer_with_a_wrong_pairing_proof_is_not_queued() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let mut dialer = handshake(&server, Uuid::new_v4(), None, true).await;

        let (accepted, reason) = hello_ack(&mut dialer.events).await;
        assert!(!accepted);
        assert!(reason.unwrap().contains("authentication failed"));
        assert!(server.pending.list().await.is_empty());
    }

    #[tokio::test]
    async fn a_low_order_client_key_is_rejected() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let (client, mut events) = dial(&server).await;
        let zero_key = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        client
            .tx
            .send(hello(Uuid::new_v4(), &zero_key))
            .await
            .unwrap();

        let (accepted, reason) = hello_ack(&mut events).await;
        assert!(!accepted);
        assert!(reason.unwrap().contains("key exchange"));
    }

    #[tokio::test]
    async fn probe_gets_ack_without_creating_pending() {
        let (server, identity, _trust, _sessions, _dir) = setup().await;
        let (client, mut events) = dial(&server).await;
        client.tx.send(SignalingMessage::Probe {}).await.unwrap();

        let SignalingMessage::ProbeAck {
            peer_id,
            peer_name,
            app_version,
        } = next_msg(&mut events, |m| {
            matches!(m, SignalingMessage::ProbeAck { .. })
        })
        .await
        else {
            unreachable!()
        };
        assert_eq!(peer_id, identity.peer_id.to_string());
        assert_eq!(peer_name, "server");
        assert_eq!(app_version, env!("CARGO_PKG_VERSION"));

        assert!(
            server.pending.list().await.is_empty(),
            "a probe must not create a pending peer"
        );
        assert!(
            server.connections.read().await.is_empty(),
            "a probe must not register a connection"
        );
    }

    #[tokio::test]
    async fn probe_is_answered_even_when_a_heartbeat_arrives_first() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let (client, mut events) = dial(&server).await;
        client
            .tx
            .send(SignalingMessage::Heartbeat {
                timestamp_ms: 0,
                streams_stats: Vec::new(),
            })
            .await
            .unwrap();
        client.tx.send(SignalingMessage::Probe {}).await.unwrap();

        next_msg(&mut events, |m| {
            matches!(m, SignalingMessage::ProbeAck { .. })
        })
        .await;
    }

    #[tokio::test]
    async fn server_rejects_protocol_mismatch() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let (client, mut events) = dial(&server).await;
        client
            .tx
            .send(SignalingMessage::Hello {
                protocol_version: 99,
                peer_id: Uuid::new_v4().to_string(),
                peer_name: "client".into(),
                app_version: "0".into(),
                capabilities: local_capabilities(),
                client_nonce: "cn".into(),
                client_key: KeyExchange::new().public_key().into(),
            })
            .await
            .unwrap();
        let (accepted, reason) = hello_ack(&mut events).await;
        assert!(!accepted);
        assert!(reason.unwrap().contains("protocol_version"));
    }

    #[tokio::test]
    async fn accepting_a_pairing_stores_the_derived_token_and_acks() {
        let (server, _identity, trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let mut dialer = pair(&server, peer_id).await;
        wait_pending(&server, 1).await;

        accept_pending(
            &server.pending,
            &trust,
            &server.connections,
            &server.connection_established_tx,
            peer_id,
        )
        .await
        .unwrap();
        assert!(dialer.token.len() >= MIN_AUTH_TOKEN_LEN);
        assert!(trust.read().await.verify(&peer_id, &dialer.token));

        let (accepted, _) = hello_ack(&mut dialer.events).await;
        assert!(accepted);
    }

    #[tokio::test]
    async fn rejecting_a_pairing_tells_the_dialer_and_drops_the_connection() {
        let (server, _identity, trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let mut dialer = pair(&server, peer_id).await;
        wait_pending(&server, 1).await;

        assert!(reject_pending(&server.pending, &server.connections, peer_id).await);

        let (accepted, reason) = hello_ack(&mut dialer.events).await;
        assert!(!accepted);
        assert!(reason.unwrap().contains("rejected"));
        assert!(!server.connections.read().await.contains_key(&peer_id));
        assert!(!trust.read().await.contains(&peer_id));
    }

    #[tokio::test]
    async fn trusted_peer_proves_the_token_and_is_auto_accepted() {
        let settings = Settings {
            auto_accept_trusted: true,
            ..Settings::default()
        };
        let (server, identity, trust, _sessions, _dir) = setup_with_settings(settings).await;
        let peer_id = Uuid::new_v4();
        let token = trust_peer(&trust, peer_id).await;

        let mut dialer = handshake(&server, peer_id, Some(&token), false).await;

        assert_eq!(dialer.server_peer_id, identity.peer_id.to_string());
        let (accepted, _) = hello_ack(&mut dialer.events).await;
        assert!(
            accepted,
            "a peer that proves the pairing token must be accepted"
        );
        assert!(server.pending.list().await.is_empty());
    }

    #[tokio::test]
    async fn trusted_peer_without_auto_accept_goes_to_pending_after_proving() {
        let off = Settings {
            auto_accept_trusted: false,
            ..Settings::default()
        };
        let (server, _identity, trust, _sessions, _dir) = setup_with_settings(off).await;
        let peer_id = Uuid::new_v4();
        let token = trust_peer(&trust, peer_id).await;

        let _dialer = handshake(&server, peer_id, Some(&token), false).await;

        let pending = wait_pending(&server, 1).await;
        assert_eq!(pending[0].peer_id, peer_id);
        assert!(
            pending[0].pairing_code.is_none(),
            "an existing pairing has nothing to compare"
        );
        assert_eq!(pending[0].token, token);
    }

    #[tokio::test]
    async fn trusted_peer_with_a_wrong_proof_is_rejected_not_queued() {
        let (server, _identity, trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let token = trust_peer(&trust, peer_id).await;

        let mut dialer = handshake(&server, peer_id, Some(&token), true).await;

        let (accepted, reason) = hello_ack(&mut dialer.events).await;
        assert!(!accepted);
        assert!(reason.unwrap().contains("authentication failed"));
        assert!(server.pending.list().await.is_empty());
    }

    #[tokio::test]
    async fn reconnect_while_pending_keeps_the_request_and_the_newest_connection() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let peer_id = Uuid::new_v4();
        let first = pair(&server, peer_id).await;
        wait_pending(&server, 1).await;

        let second = pair(&server, peer_id).await;
        // Let the replaced connection's teardown and its watcher run.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let pending = server.pending.list().await;
        assert_eq!(pending.len(), 1, "one request per peer: {pending:?}");
        assert_eq!(pending[0].pairing_code, second.code);
        let conns = server.connections.read().await;
        let handle = conns
            .get(&peer_id)
            .expect("the retrying connection must survive the old one's teardown");
        assert_eq!(handle.connection_id, pending[0].connection_id);
        drop(conns);
        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn pending_queue_is_capped() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let mut dialers = Vec::new();
        for _ in 0..MAX_PENDING_PEERS {
            dialers.push(pair(&server, Uuid::new_v4()).await);
        }
        wait_pending(&server, MAX_PENDING_PEERS).await;

        let mut overflow = pair(&server, Uuid::new_v4()).await;
        let (accepted, reason) = hello_ack(&mut overflow.events).await;
        assert!(!accepted);
        assert!(reason.unwrap().contains("too many pending"));
        assert_eq!(server.pending.list().await.len(), MAX_PENDING_PEERS);
    }

    fn pending_peer(peer_id: Uuid, connection_id: Uuid) -> PendingPeer {
        PendingPeer {
            peer_id,
            peer_name: "p".into(),
            remote_addr: "127.0.0.1:1".parse().unwrap(),
            connection_id,
            pairing_code: None,
            token: "t".repeat(43),
        }
    }

    #[test]
    fn pending_peer_debug_hides_the_token() {
        let rendered = format!("{:?}", pending_peer(Uuid::new_v4(), Uuid::new_v4()));
        assert!(!rendered.contains("ttt"), "{rendered}");
    }

    #[tokio::test]
    async fn pending_requests_are_addressed_by_peer_not_position() {
        let pending = PendingPeers::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        for peer_id in [a, b] {
            assert!(pending.upsert(pending_peer(peer_id, Uuid::new_v4())).await);
        }
        assert!(pending.remove_peer(&a).await);
        assert_eq!(pending.take(&b).await.map(|p| p.peer_id), Some(b));
        assert!(pending.take(&a).await.is_none());
    }

    #[tokio::test]
    async fn pending_changes_wake_a_watcher() {
        let pending = Arc::new(PendingPeers::default());
        let peer_id = Uuid::new_v4();
        let changed = |pending: &Arc<PendingPeers>| {
            let pending = pending.clone();
            async move {
                tokio::time::timeout(std::time::Duration::from_millis(200), pending.changed())
                    .await
                    .is_ok()
            }
        };

        pending.upsert(pending_peer(peer_id, Uuid::new_v4())).await;
        assert!(changed(&pending).await, "queued");
        assert!(!pending.remove_peer(&Uuid::new_v4()).await);
        assert!(!changed(&pending).await, "nothing was removed");
        pending.take(&peer_id).await;
        assert!(changed(&pending).await, "taken");
    }

    #[tokio::test]
    async fn remove_connection_only_drops_the_matching_request() {
        let pending = PendingPeers::default();
        let peer_id = Uuid::new_v4();
        let (old_conn, new_conn) = (Uuid::new_v4(), Uuid::new_v4());
        for connection_id in [old_conn, new_conn] {
            pending.upsert(pending_peer(peer_id, connection_id)).await;
        }
        assert!(!pending.remove_connection(&peer_id, old_conn).await);
        assert_eq!(pending.list().await.len(), 1);
        assert!(pending.remove_connection(&peer_id, new_conn).await);
        assert!(pending.list().await.is_empty());
    }

    #[tokio::test]
    async fn pre_accept_drop_evicts_pending_and_connections() {
        let (server, _identity, _trust, _sessions, _dir) = setup().await;
        let client_peer_id = Uuid::new_v4();
        let client = pair(&server, client_peer_id).await;

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
