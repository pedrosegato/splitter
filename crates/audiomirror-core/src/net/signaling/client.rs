use crate::error::NetError;
use crate::net::identity::PeerIdentity;
use crate::net::signaling::connection::{spawn_peer_connection, PeerConnectionHandle, PeerEvent};
use crate::net::signaling::message::{SignalingMessage, PROTOCOL_VERSION};
use crate::net::signaling::server::local_capabilities;
use crate::net::trust::TrustStore;
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
    let handle = spawn_peer_connection(stream, None);
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
                Ok(PeerEvent::Message(SignalingMessage::HelloAck { accepted, reason })) => {
                    return Ok((accepted, reason));
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

    Ok(ConnectOutcome {
        remote_peer_id: remote_peer_id_hint,
        handle,
        accepted: ack.0,
        reason: ack.1,
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
}
