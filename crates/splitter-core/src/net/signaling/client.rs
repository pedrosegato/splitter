use crate::error::NetError;
use crate::net::crypto::{self, KeyExchange, Transcript};
use crate::net::identity::PeerIdentity;
use crate::net::signaling::connection::{
    spawn_peer_connection, PeerConnectionHandle, PeerEvent, Role,
};
use crate::net::signaling::message::{SignalingMessage, PROTOCOL_VERSION};
use crate::net::signaling::server::local_capabilities;
use crate::net::trust::{TrustStore, TrustedPeer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use uuid::Uuid;

/// How long the other machine's operator has to compare the code and accept.
pub const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);

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
    on_pairing_code: impl FnOnce(Uuid, &str) + Send,
) -> Result<ConnectOutcome, NetError> {
    let stream = tokio::time::timeout(handshake_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| NetError::Timeout {
            what: "tcp connect".into(),
            millis: handshake_timeout.as_millis() as u64,
        })?
        .map_err(NetError::UdpIo)?;
    let handle = spawn_peer_connection(stream)?;
    let mut events = handle.events.subscribe();

    let my_id = identity.peer_id.to_string();
    let client_nonce = crypto::new_nonce();
    let exchange = KeyExchange::new();
    let client_key = exchange.public_key().to_string();
    let mut exchange = Some(exchange);
    let hello = SignalingMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        peer_id: my_id.clone(),
        peer_name: identity.peer_name.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        capabilities: local_capabilities(),
        client_nonce: client_nonce.clone(),
        client_key: client_key.clone(),
    };
    handle
        .tx
        .send(hello)
        .await
        .map_err(|_| protocol_error("peer connection closed before hello"))?;

    let mut on_pairing_code = Some(on_pairing_code);
    let mut wait = handshake_timeout;
    let mut deadline = tokio::time::Instant::now() + wait;
    let mut verified: Option<Verified> = None;
    let (accepted, reason) = loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .map_err(|_| NetError::Timeout {
                what: "hello_ack".into(),
                millis: wait.as_millis() as u64,
            })?;
        let (server_peer_id, server_nonce, server_key, server_proof) = match event {
            Ok(PeerEvent::Message(SignalingMessage::Challenge {
                server_peer_id,
                server_nonce,
                server_key,
                server_proof,
            })) => (server_peer_id, server_nonce, server_key, Some(server_proof)),
            Ok(PeerEvent::Message(SignalingMessage::PairingOffer {
                server_peer_id,
                server_nonce,
                server_key,
            })) => (server_peer_id, server_nonce, server_key, None),
            Ok(PeerEvent::Message(SignalingMessage::HelloAck { accepted, reason })) => {
                break (accepted, reason)
            }
            Ok(PeerEvent::Disconnected { reason }) => {
                return Err(NetError::SignalingProtocol { reason });
            }
            Ok(_) => continue,
            Err(e) => return Err(protocol_error(&format!("event channel: {e}"))),
        };

        let server_id = Uuid::parse_str(&server_peer_id)
            .map_err(|_| protocol_error("handshake with an invalid peer id"))?;
        if let Some(hint) = remote_peer_id_hint.filter(|hint| *hint != server_id) {
            return Err(protocol_error(&format!(
                "expected peer {hint}, but {server_id} answered"
            )));
        }
        let transcript = Transcript {
            client_peer_id: &my_id,
            server_peer_id: &server_peer_id,
            client_nonce: &client_nonce,
            server_nonce: &server_nonce,
            client_key: &client_key,
            server_key: &server_key,
        };
        let stored_token = trust.read().await.token_for(Some(server_id));
        let known_token = match (server_proof, stored_token) {
            (Some(proof), Some(token)) => {
                if !crypto::proofs_match(&crypto::server_proof(&token, &transcript), &proof) {
                    return Err(protocol_error(&format!(
                        "peer claiming to be {server_id} failed to prove its identity"
                    )));
                }
                Some(token)
            }
            (Some(_), None) => {
                return Err(protocol_error(&format!(
                    "peer {server_id} remembers a pairing this machine no longer has; \
                     remove it on the other machine to pair again"
                )));
            }
            // Re-pairing silently would let anyone answering at a known peer's address
            // replace the stored token.
            (None, Some(_)) => {
                return Err(protocol_error(&format!(
                    "peer {server_id} no longer recognizes this machine; \
                     remove it here to pair again"
                )));
            }
            (None, None) => None,
        };
        let shared = exchange
            .take()
            .and_then(|exchange| exchange.agree(&server_key))
            .ok_or_else(|| protocol_error("invalid key exchange"))?;
        let token = match known_token {
            Some(token) => {
                verified = Some(Verified::Known(server_id));
                token
            }
            None => {
                let token = crypto::pairing_token(&shared, &transcript);
                if let Some(show) = on_pairing_code.take() {
                    show(server_id, &crypto::pairing_code(&shared, &transcript));
                }
                wait = PAIRING_TIMEOUT;
                deadline = tokio::time::Instant::now() + wait;
                verified = Some(Verified::Pairing {
                    server_id,
                    token: token.clone(),
                });
                token
            }
        };
        handle.install_session(
            crypto::session_keys(&shared, &token, &transcript),
            Role::Dialer,
        );
        handle.start_encrypting();
        handle
            .tx
            .send(SignalingMessage::ChallengeResponse {
                client_proof: crypto::client_proof(&token, &transcript),
            })
            .await
            .map_err(|_| protocol_error("peer closed during the handshake"))?;
    };

    let remote_peer_id = verified
        .as_ref()
        .map(Verified::server_id)
        .or(remote_peer_id_hint);
    if accepted {
        match verified {
            None => return Err(protocol_error("peer accepted us without a key exchange")),
            Some(Verified::Pairing { server_id, token }) => {
                trust.write().await.add(TrustedPeer {
                    peer_id: server_id,
                    peer_name: String::new(),
                    auth_token: token,
                })?;
            }
            Some(Verified::Known(_)) => {}
        }
    }

    Ok(ConnectOutcome {
        remote_peer_id,
        handle,
        accepted,
        reason,
    })
}

enum Verified {
    Known(Uuid),
    Pairing { server_id: Uuid, token: String },
}

impl Verified {
    fn server_id(&self) -> Uuid {
        match self {
            Verified::Known(id) | Verified::Pairing { server_id: id, .. } => *id,
        }
    }
}

fn protocol_error(reason: &str) -> NetError {
    NetError::SignalingProtocol {
        reason: reason.into(),
    }
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

    async fn assert_encrypted_both_ways(
        dialer: &PeerConnectionHandle,
        server: &crate::net::signaling::SignalingServerHandle,
        dialer_id: Uuid,
    ) {
        let conns = server.connections.read().await;
        let acceptor = conns.get(&dialer_id).expect("server keeps the connection");
        let renamed = |name: &str| SignalingMessage::PeerRenamed {
            peer_id: "p".into(),
            peer_name: name.into(),
        };
        let mut at_acceptor = acceptor.events.subscribe();
        let mut at_dialer = dialer.events.subscribe();
        dialer.tx.send(renamed("from-dialer")).await.unwrap();
        acceptor.tx.send(renamed("from-acceptor")).await.unwrap();
        for (events, expected) in [
            (&mut at_acceptor, "from-dialer"),
            (&mut at_dialer, "from-acceptor"),
        ] {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(PeerEvent::Message(SignalingMessage::PeerRenamed {
                        peer_name, ..
                    })) = events.recv().await
                    {
                        if peer_name == expected {
                            return;
                        }
                    }
                }
            })
            .await
            .expect("message must cross the encrypted connection");
        }
        assert_eq!(dialer.encryption_state(), (true, true));
        assert_eq!(acceptor.encryption_state(), (true, true));
        assert_eq!(
            dialer.session_keys().unwrap().client_to_server,
            acceptor.session_keys().unwrap().client_to_server,
            "both ends must derive the same session"
        );
    }

    #[tokio::test]
    async fn first_connect_shows_the_same_code_on_both_sides_and_is_acked_after_accept() {
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
        let client_peer_id = client_identity.peer_id;
        let client_trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
        ));

        let (code_tx, code_rx) = std::sync::mpsc::channel();
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
                    move |server_id, code| code_tx.send((server_id, code.to_string())).unwrap(),
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
                let request = server_pending.list().await[0].clone();
                accept_pending(
                    &server_pending,
                    &server_trust,
                    &server_conns,
                    &conn_est_tx,
                    request.peer_id,
                )
                .await
                .map(|()| request.pairing_code)
            }
        });

        let (dial_result, accept_result) = tokio::join!(dial, accepted_handle);
        let outcome = dial_result.unwrap().unwrap();
        let server_code = accept_result.unwrap().unwrap();
        assert!(
            outcome.accepted,
            "client should receive HelloAck.accepted=true"
        );
        let (shown_for, client_code) = code_rx.try_recv().expect("dialer was shown a code");
        assert_eq!(shown_for, server_peer_id);
        assert_eq!(server_code, Some(client_code));
        assert_encrypted_both_ways(&outcome.handle, &server, client_peer_id).await;
    }

    #[tokio::test]
    async fn the_derived_pairing_token_is_persisted_on_both_sides() {
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
                    |_, _| {},
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
                accept_pending(
                    &pending,
                    &trust,
                    &conns,
                    &tx,
                    pending.list().await[0].peer_id,
                )
                .await
            }
        });

        let (dial_res, accept_res) = tokio::join!(dial, acceptor);
        let outcome = dial_res.unwrap().unwrap();
        accept_res.unwrap().unwrap();
        assert!(outcome.accepted);
        let stored_token = server_trust
            .read()
            .await
            .token_for(Some(client_identity.peer_id))
            .expect("server stores the pairing on accept");

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
            let token = "a".repeat(43);
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
            |_, _| panic!("an existing pairing has no code to show"),
        )
        .await
        .unwrap();
        assert!(outcome.accepted);
        assert_encrypted_both_ways(&outcome.handle, &server, client_identity.peer_id).await;
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
                    |_, _| {},
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
                accept_pending(
                    &pending,
                    &trust,
                    &conns,
                    &tx,
                    pending.list().await[0].peer_id,
                )
                .await
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
        let shared_token = "a".repeat(43);

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
            |_, _| {},
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
    async fn dial_without_a_hint_learns_the_server_id_from_the_handshake() {
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
        let client_peer_id = client_identity.peer_id;
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
                    |_, _| {},
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
                accept_pending(
                    &pending,
                    &trust,
                    &conns,
                    &tx,
                    pending.list().await[0].peer_id,
                )
                .await
            }
        });

        let (dial_res, accept_res) = tokio::join!(dial, acceptor);
        let outcome = dial_res.unwrap().unwrap();
        accept_res.unwrap().unwrap();
        assert!(outcome.accepted, "connection must be accepted");
        assert_eq!(
            outcome.remote_peer_id,
            Some(server_peer_id),
            "ConnectOutcome must reflect the server peer_id learned from the handshake"
        );
        let stored_token = server_trust
            .read()
            .await
            .token_for(Some(client_peer_id))
            .unwrap();

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

    async fn impostor(reply: SignalingMessage) -> (SocketAddr, tokio::task::JoinHandle<bool>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let handle = spawn_peer_connection(stream).unwrap();
            let mut events = handle.events.subscribe();
            loop {
                if let Ok(PeerEvent::Message(SignalingMessage::Hello { .. })) = events.recv().await
                {
                    break;
                }
            }
            handle.tx.send(reply).await.unwrap();
            let answered = tokio::time::timeout(Duration::from_millis(500), async {
                loop {
                    match events.recv().await {
                        Ok(PeerEvent::Message(SignalingMessage::ChallengeResponse { .. })) => {
                            return true
                        }
                        Ok(_) => continue,
                        Err(_) => return false,
                    }
                }
            })
            .await
            .unwrap_or(false);
            drop(handle);
            answered
        });
        (addr, task)
    }

    async fn trust_with(
        server_peer_id: Uuid,
        token: &str,
    ) -> (Arc<RwLock<TrustStore>>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("trust.toml")).unwrap(),
        ));
        trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id: server_peer_id,
                peer_name: "server".into(),
                auth_token: token.into(),
            })
            .unwrap();
        (trust, dir)
    }

    #[tokio::test]
    async fn client_aborts_when_the_server_cannot_prove_the_pairing_token() {
        let server_peer_id = Uuid::new_v4();
        let (trust, _dir) = trust_with(server_peer_id, &"t".repeat(43)).await;
        let (addr, saw_response) = impostor(SignalingMessage::Challenge {
            server_peer_id: server_peer_id.to_string(),
            server_nonce: "sn".into(),
            server_key: KeyExchange::new().public_key().into(),
            server_proof: "forged".into(),
        })
        .await;

        let err = connect_to_peer(
            addr,
            &make_identity("client"),
            trust,
            Some(server_peer_id),
            Duration::from_secs(2),
            |_, _| {},
        )
        .await
        .expect_err("an impostor must not be accepted");

        assert!(err.to_string().contains("failed to prove"), "{err}");
        assert!(
            !saw_response.await.unwrap(),
            "the client must not answer a challenge it could not verify"
        );
    }

    #[tokio::test]
    async fn client_refuses_an_ack_without_a_key_exchange() {
        let (addr, _task) = impostor(SignalingMessage::HelloAck {
            accepted: true,
            reason: None,
        })
        .await;
        let dir = tempdir().unwrap();
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("trust.toml")).unwrap(),
        ));

        let err = connect_to_peer(
            addr,
            &make_identity("client"),
            trust,
            None,
            Duration::from_secs(2),
            |_, _| {},
        )
        .await
        .expect_err("an ack must follow a key exchange");

        assert!(err.to_string().contains("without a key exchange"), "{err}");
    }

    #[tokio::test]
    async fn client_refuses_to_re_pair_with_a_peer_it_already_trusts() {
        let server_peer_id = Uuid::new_v4();
        let original = "o".repeat(43);
        let (trust, _dir) = trust_with(server_peer_id, &original).await;
        let (addr, saw_response) = impostor(SignalingMessage::PairingOffer {
            server_peer_id: server_peer_id.to_string(),
            server_nonce: "sn".into(),
            server_key: KeyExchange::new().public_key().into(),
        })
        .await;
        let shown = std::sync::atomic::AtomicBool::new(false);

        let err = connect_to_peer(
            addr,
            &make_identity("client"),
            trust.clone(),
            None,
            Duration::from_secs(2),
            |_, _| shown.store(true, std::sync::atomic::Ordering::Relaxed),
        )
        .await
        .expect_err("a known peer must prove the existing pairing");

        assert!(err.to_string().contains("no longer recognizes"), "{err}");
        assert!(!shown.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!saw_response.await.unwrap());
        assert!(
            trust.read().await.verify(&server_peer_id, &original),
            "the stored pairing must survive"
        );
    }
}
