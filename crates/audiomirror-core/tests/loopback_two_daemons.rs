use audiomirror_core::net::manager::SessionManager;
use audiomirror_core::net::signaling::client::connect_to_peer;
use audiomirror_core::net::signaling::server::{
    accept_pending, local_capabilities, SignalingServer,
};
use audiomirror_core::net::trust::TrustStore;
use audiomirror_core::settings::Settings;
use audiomirror_core::PeerIdentity;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::RwLock;
use uuid::Uuid;

fn id(name: &str) -> PeerIdentity {
    PeerIdentity {
        peer_id: Uuid::new_v4(),
        peer_name: name.into(),
    }
}

#[tokio::test]
async fn two_local_daemons_full_handshake_and_session() {
    let dir = tempdir().unwrap();

    let a_identity = id("alice");
    let a_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("alice-trust.toml")).unwrap(),
    ));
    let a_sessions = SessionManager::new();
    let a_settings = Arc::new(RwLock::new(Settings::default()));
    let a_server = SignalingServer::start(
        "127.0.0.1:0".parse().unwrap(),
        a_identity.clone(),
        a_trust.clone(),
        a_sessions.clone(),
        a_settings,
    )
    .await
    .unwrap();

    let b_identity = id("bob");
    let b_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("bob-trust.toml")).unwrap(),
    ));
    let b_sessions = SessionManager::new();
    let b_settings = Arc::new(RwLock::new(Settings::default()));
    let _b_server = SignalingServer::start(
        "127.0.0.1:0".parse().unwrap(),
        b_identity.clone(),
        b_trust.clone(),
        b_sessions.clone(),
        b_settings,
    )
    .await
    .unwrap();

    let dial = tokio::spawn({
        let target = a_server.bind_addr;
        let b_identity = b_identity.clone();
        let b_trust = b_trust.clone();
        async move { connect_to_peer(target, &b_identity, b_trust, None, Duration::from_secs(5)).await }
    });
    let acceptor = tokio::spawn({
        let pending = a_server.pending.clone();
        let conns = a_server.connections.clone();
        let trust = a_trust.clone();
        async move {
            for _ in 0..50 {
                if !pending.list().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            accept_pending(&pending, &trust, &conns, 0).await
        }
    });
    let (dial_res, accept_res) = tokio::join!(dial, acceptor);
    let outcome = dial_res.unwrap().unwrap();
    accept_res.unwrap().unwrap();
    assert!(outcome.accepted);

    let session_id = a_sessions
        .open_outgoing(a_identity.peer_id, b_identity.peer_id)
        .await;
    a_sessions.accept(&session_id).await.unwrap();
    let _caps = local_capabilities();
    let snap = a_sessions.snapshot().await;
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].id, session_id);
}
