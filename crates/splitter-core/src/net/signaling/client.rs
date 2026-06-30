use crate::error::NetError;
use crate::net::identity::PeerIdentity;
use crate::net::signaling::connection::{spawn_peer_connection, PeerConnectionHandle, PeerEvent};
use crate::net::signaling::message::{SignalingMessage, PROTOCOL_VERSION};
use crate::net::signaling::server::local_capabilities;
use crate::net::trust::{TrustStore, TrustedPeer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug)]
pub struct ConnectOutcome {
    pub remote_peer_id: Option<Uuid>,
    pub handle: PeerConnectionHandle,
    pub accepted: bool,
    pub reason: Option<String>,
}

pub async fn connect_to_peer(
    addr: SocketAddr,
    identity: &PeerIdentity,
    trust: Arc<RwLock<TrustStore>>,
    remote_peer_id_hint: Option<Uuid>,
    handshake_timeout: Duration,
) -> Result<ConnectOutcome, NetError> {
    let stream = tokio::time::timeout(handshake_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| NetError::Timeout {
            what: "tcp connect".into(),
            millis: handshake_timeout.as_millis() as u64,
        })?
        .map_err(NetError::UdpIo)?;
    let handle = spawn_peer_connection(stream, None)?;
    let mut events = handle.events.subscribe();

    let resolved_token = trust
        .read()
        .await
        .token_for(remote_peer_id_hint)
        .unwrap_or_default();

    let hello = SignalingMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        peer_id: identity.peer_id.to_string(),
        peer_name: identity.peer_name.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        capabilities: local_capabilities(),
        auth_token: resolved_token,
    };
    handle
        .tx
        .send(hello)
        .await
        .map_err(|_| NetError::SignalingProtocol {
            reason: "peer connection closed before hello".into(),
        })?;

    let ack = tokio::time::timeout(handshake_timeout, async {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(SignalingMessage::HelloAck {
                    accepted,
                    reason,
                    auth_token,
                    peer_id,
                })) => {
                    return Ok((accepted, reason, auth_token, peer_id));
                }
                Ok(PeerEvent::Disconnected { reason }) => {
                    return Err(NetError::SignalingProtocol { reason });
                }
                Ok(_) => continue,
                Err(e) => {
                    return Err(NetError::SignalingProtocol {
                        reason: format!("event channel: {e}"),
                    });
                }
            }
        }
    })
    .await
    .map_err(|_| NetError::Timeout {
        what: "hello_ack".into(),
        millis: handshake_timeout.as_millis() as u64,
    })??;

    let (accepted, reason, received_token, ack_peer_id_str) = ack;

    let resolved_peer_id: Option<Uuid> = remote_peer_id_hint.or_else(|| {
        ack_peer_id_str
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
    });

    if accepted {
        if let (Some(token), Some(peer_id)) = (received_token, resolved_peer_id) {
            let mut t = trust.write().await;
            let peer_name = t
                .peer_for(&peer_id)
                .map(|p| p.peer_name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_default();
            let _ = t.add(TrustedPeer {
                peer_id,
                peer_name,
                auth_token: token,
            });
        }
    }

    Ok(ConnectOutcome {
        remote_peer_id: resolved_peer_id,
        handle,
        accepted,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::manager::SessionManager;
    use crate::net::signaling::server::{accept_pending, SignalingServer};
    use crate::net::trust::TrustedPeer;
    use crate::settings::Settings;
    use tempfile::tempdir;

    fn make_identity(name: &str) -> PeerIdentity {
        PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: name.into(),
        }
    }

    #[tokio::test]
    async fn first_connect_queues_pending_and_ack_after_accept() {
        let dir = tempdir().unwrap();
        let server_identity = make_identity("server");
        let server_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        let settings = Arc::new(RwLock::new(Settings::default()));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            server_identity,
            server_trust.clone(),
            sessions,
            settings,
        )
        .await
        .unwrap();

        let client_identity = make_identity("client");
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));

        let dial = tokio::spawn({
            let client_trust = client_trust.clone();
            let bind_addr = server.bind_addr;
            async move {
                connect_to_peer(
                    bind_addr,
                    &client_identity,
                    client_trust,
                    None,
                    Duration::from_secs(5),
                )
                .await
            }
        });

        let accepted_handle = tokio::spawn({
            let server_trust = server_trust.clone();
            let server_pending = server.pending.clone();
            let server_conns = server.connections.clone();
            let conn_est_tx = server.connection_established_tx.clone();
            async move {
                for _ in 0..50 {
                    if !server_pending.list().await.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                accept_pending(
                    &server_pending,
                    &server_trust,
                    &server_conns,
                    &conn_est_tx,
                    0,
                )
                .await
            }
        });

        let (dial_result, accept_result) = tokio::join!(dial, accepted_handle);
        let outcome = dial_result.unwrap().unwrap();
        accept_result.unwrap().unwrap();
        assert!(
            outcome.accepted,
            "client should receive HelloAck.accepted=true"
        );
    }

    #[tokio::test]
    async fn accept_pending_token_in_hello_ack_is_persisted_in_dialer_trust_store() {
        let dir = tempdir().unwrap();
        let server_identity = make_identity("server");
        let server_peer_id = server_identity.peer_id;
        let server_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        let settings = Arc::new(RwLock::new(Settings {
            auto_accept_trusted: true,
            ..Settings::default()
        }));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            server_identity,
            server_trust.clone(),
            sessions,
            settings,
        )
        .await
        .unwrap();

        let client_identity = make_identity("client");
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));

        let bind_addr = server.bind_addr;
        let dial = tokio::spawn({
            let client_trust = client_trust.clone();
            let client_identity = client_identity.clone();
            async move {
                connect_to_peer(
                    bind_addr,
                    &client_identity,
                    client_trust,
                    Some(server_peer_id),
                    Duration::from_secs(5),
                )
                .await
            }
        });

        let acceptor = tokio::spawn({
            let pending = server.pending.clone();
            let conns = server.connections.clone();
            let trust = server_trust.clone();
            let tx = server.connection_established_tx.clone();
            async move {
                for _ in 0..50 {
                    if !pending.list().await.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                accept_pending(&pending, &trust, &conns, &tx, 0).await
            }
        });

        let (dial_res, accept_res) = tokio::join!(dial, acceptor);
        let outcome = dial_res.unwrap().unwrap();
        let (_, stored_token) = accept_res.unwrap().unwrap();
        assert!(outcome.accepted);

        let t = client_trust.read().await;
        assert!(
            t.contains(&server_peer_id),
            "dialer's TrustStore must contain the server after accept"
        );
        assert!(
            t.verify(&server_peer_id, &stored_token),
            "dialer's token must match the server's stored token"
        );
    }

    #[tokio::test]
    async fn second_connect_with_stored_token_is_immediately_accepted() {
        let dir = tempdir().unwrap();
        let server_identity = make_identity("server");
        let server_peer_id = server_identity.peer_id;
        let server_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        // auto_accept_trusted must be true for known+verified peers to be accepted immediately
        let settings = Arc::new(RwLock::new(Settings {
            auto_accept_trusted: true,
            ..Settings::default()
        }));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            server_identity,
            server_trust.clone(),
            sessions,
            settings,
        )
        .await
        .unwrap();

        let client_identity = make_identity("client");
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));
        {
            let token = "shared-tok".to_string();
            server_trust
                .write()
                .await
                .add(TrustedPeer {
                    peer_id: client_identity.peer_id,
                    peer_name: client_identity.peer_name.clone(),
                    auth_token: token.clone(),
                })
                .unwrap();
            client_trust
                .write()
                .await
                .add(TrustedPeer {
                    peer_id: server_peer_id,
                    peer_name: "server".into(),
                    auth_token: token,
                })
                .unwrap();
        }

        let outcome = connect_to_peer(
            server.bind_addr,
            &client_identity,
            client_trust,
            Some(server_peer_id),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(outcome.accepted);
    }

    #[tokio::test]
    async fn connect_with_no_hint_and_accepted_does_not_panic() {
        let dir = tempdir().unwrap();
        let server_identity = make_identity("server");
        let server_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        let settings = Arc::new(RwLock::new(Settings::default()));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            server_identity,
            server_trust.clone(),
            sessions,
            settings,
        )
        .await
        .unwrap();

        let client_identity = make_identity("client");
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));

        let dial = tokio::spawn({
            let client_trust = client_trust.clone();
            let bind_addr = server.bind_addr;
            async move {
                connect_to_peer(
                    bind_addr,
                    &client_identity,
                    client_trust,
                    None,
                    Duration::from_secs(5),
                )
                .await
            }
        });

        let acceptor = tokio::spawn({
            let pending = server.pending.clone();
            let conns = server.connections.clone();
            let trust = server_trust.clone();
            let tx = server.connection_established_tx.clone();
            async move {
                for _ in 0..50 {
                    if !pending.list().await.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                accept_pending(&pending, &trust, &conns, &tx, 0).await
            }
        });

        let (dial_res, accept_res) = tokio::join!(dial, acceptor);
        let outcome = dial_res.unwrap().unwrap();
        accept_res.unwrap().unwrap();
        assert!(
            outcome.accepted,
            "connection must be accepted even with no hint"
        );
    }

    #[tokio::test]
    async fn accept_does_not_overwrite_stored_peer_name_with_empty() {
        let dir = tempdir().unwrap();
        let server_identity = make_identity("server");
        let server_peer_id = server_identity.peer_id;
        let server_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        let settings = Arc::new(RwLock::new(Settings {
            auto_accept_trusted: true,
            ..Settings::default()
        }));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            server_identity,
            server_trust.clone(),
            sessions,
            settings,
        )
        .await
        .unwrap();

        let client_identity = make_identity("client");
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));

        let known_name = "My Server";
        let shared_token = "shared-tok".to_string();

        server_trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id: client_identity.peer_id,
                peer_name: client_identity.peer_name.clone(),
                auth_token: shared_token.clone(),
            })
            .unwrap();
        client_trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id: server_peer_id,
                peer_name: known_name.into(),
                auth_token: shared_token,
            })
            .unwrap();

        let outcome = connect_to_peer(
            server.bind_addr,
            &client_identity,
            client_trust.clone(),
            Some(server_peer_id),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(outcome.accepted);

        let t = client_trust.read().await;
        let stored = t
            .peer_for(&server_peer_id)
            .expect("server must be in trust store");
        assert_eq!(
            stored.peer_name, known_name,
            "accept must not overwrite existing peer name with empty string"
        );
    }

    #[tokio::test]
    async fn dial_with_no_hint_accepted_persists_token_via_hello_ack_peer_id() {
        use crate::net::signaling::server::accept_pending_as;
        let dir = tempdir().unwrap();
        let server_identity = make_identity("server");
        let server_peer_id = server_identity.peer_id;
        let server_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
        ));
        let sessions = SessionManager::new();
        let settings = Arc::new(RwLock::new(Settings::default()));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            server_identity,
            server_trust.clone(),
            sessions,
            settings,
        )
        .await
        .unwrap();

        let client_identity = make_identity("client");
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));

        let dial = tokio::spawn({
            let client_trust = client_trust.clone();
            let bind_addr = server.bind_addr;
            async move {
                connect_to_peer(
                    bind_addr,
                    &client_identity,
                    client_trust,
                    None,
                    Duration::from_secs(5),
                )
                .await
            }
        });

        let acceptor = tokio::spawn({
            let pending = server.pending.clone();
            let conns = server.connections.clone();
            let trust = server_trust.clone();
            let tx = server.connection_established_tx.clone();
            async move {
                for _ in 0..50 {
                    if !pending.list().await.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                accept_pending_as(&pending, &trust, &conns, &tx, 0, Some(server_peer_id)).await
            }
        });

        let (dial_res, accept_res) = tokio::join!(dial, acceptor);
        let outcome = dial_res.unwrap().unwrap();
        let (_, stored_token) = accept_res.unwrap().unwrap();
        assert!(outcome.accepted, "connection must be accepted");
        assert_eq!(
            outcome.remote_peer_id,
            Some(server_peer_id),
            "ConnectOutcome must reflect the server peer_id learned from HelloAck"
        );

        let t = client_trust.read().await;
        assert!(
            t.contains(&server_peer_id),
            "dialer trust store must contain server_peer_id after first contact with no hint"
        );
        assert!(
            t.verify(&server_peer_id, &stored_token),
            "stored token must match the server token"
        );
    }
}
