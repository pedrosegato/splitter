use splitter_core::net::manager::SessionManager;
use splitter_core::net::signaling::client::connect_to_peer;
use splitter_core::net::signaling::server::{accept_pending, local_capabilities, SignalingServer};
use splitter_core::net::trust::{TrustStore, TrustedPeer};
use splitter_core::settings::Settings;
use splitter_core::PeerIdentity;
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
        let conn_est_tx = a_server.connection_established_tx.clone();
        let trust = a_trust.clone();
        async move {
            for _ in 0..50 {
                if !pending.list().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            accept_pending(&pending, &trust, &conns, &conn_est_tx, 0).await
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

/// Verify that `connection_established_tx` fires on both the manual-accept path
/// and the auto-accept-trusted path.  The daemon background task relies on this
/// event to spawn `spawn_stream_open_acceptor` for every established connection.
#[tokio::test]
async fn connection_established_fires_on_both_accept_paths() {
    let dir = tempdir().unwrap();

    // ── manual-accept path ────────────────────────────────────────────────────
    let server_identity = id("server-manual");
    let server_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("s1-trust.toml")).unwrap(),
    ));
    let settings = Arc::new(RwLock::new(Settings::default())); // auto_accept_trusted = false
    let server = SignalingServer::start(
        "127.0.0.1:0".parse().unwrap(),
        server_identity,
        server_trust.clone(),
        SessionManager::new(),
        settings,
    )
    .await
    .unwrap();

    let mut est_rx = server.connection_established_tx.subscribe();

    let client_identity = id("client-manual");
    let client_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("c1-trust.toml")).unwrap(),
    ));

    let expected_peer_id = client_identity.peer_id;
    let dial = tokio::spawn({
        let addr = server.bind_addr;
        async move {
            connect_to_peer(
                addr,
                &client_identity,
                client_trust,
                None,
                Duration::from_secs(5),
            )
            .await
        }
    });
    let manual_accept = tokio::spawn({
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
    let (dial_res, accept_res) = tokio::join!(dial, manual_accept);
    dial_res.unwrap().unwrap();
    accept_res.unwrap().unwrap();

    let fired_id = tokio::time::timeout(Duration::from_secs(2), est_rx.recv())
        .await
        .expect("timed out waiting for connection_established event on manual-accept path")
        .expect("channel closed");
    assert_eq!(
        fired_id, expected_peer_id,
        "connection_established must carry the accepted peer's UUID"
    );

    // ── auto-accept-trusted path ──────────────────────────────────────────────
    let at_server_identity = id("server-auto");
    let at_server_peer_id = at_server_identity.peer_id;
    let at_server_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("s2-trust.toml")).unwrap(),
    ));
    let at_settings = Arc::new(RwLock::new(Settings {
        auto_accept_trusted: true,
        ..Settings::default()
    }));
    let at_server = SignalingServer::start(
        "127.0.0.1:0".parse().unwrap(),
        at_server_identity,
        at_server_trust.clone(),
        SessionManager::new(),
        at_settings,
    )
    .await
    .unwrap();

    let mut at_est_rx = at_server.connection_established_tx.subscribe();

    let at_client_identity = id("client-auto");
    let at_client_peer_id = at_client_identity.peer_id;
    let at_client_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("c2-trust.toml")).unwrap(),
    ));
    {
        let token = "a".repeat(43);
        // Server trusts the client by peer_id + token.
        at_server_trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id: at_client_identity.peer_id,
                peer_name: at_client_identity.peer_name.clone(),
                auth_token: token.clone(),
            })
            .unwrap();
        // Client trust store maps the server's peer_id → shared token so
        // connect_to_peer sends the right auth_token in its Hello.
        at_client_trust
            .write()
            .await
            .add(TrustedPeer {
                peer_id: at_server_peer_id,
                peer_name: "server-auto".into(),
                auth_token: token,
            })
            .unwrap();
    }

    connect_to_peer(
        at_server.bind_addr,
        &at_client_identity,
        at_client_trust,
        Some(at_server_peer_id),
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    let at_fired_id = tokio::time::timeout(Duration::from_secs(2), at_est_rx.recv())
        .await
        .expect("timed out waiting for connection_established event on auto-accept-trusted path")
        .expect("channel closed");
    assert_eq!(
        at_fired_id, at_client_peer_id,
        "connection_established must carry the auto-accepted peer's UUID"
    );

    // Pending queue must stay empty on the auto-accept path
    assert!(
        at_server.pending.list().await.is_empty(),
        "auto-accepted trusted peer must not appear in the pending queue"
    );
}

/// After the first handshake (manual accept), the dialer's TrustStore should
/// have been populated with the echoed auth_token.  A second `connect_to_peer`
/// must therefore land on the auto-accept-trusted path without any `accept_pending`
/// call — i.e. the pending queue stays empty and the connection is accepted
/// immediately.
#[tokio::test]
async fn second_connect_skips_pending_after_first_accept() {
    let dir = tempdir().unwrap();

    let server_identity = id("server");
    let server_peer_id = server_identity.peer_id;
    let server_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("server-trust.toml")).unwrap(),
    ));
    let settings = Arc::new(RwLock::new(Settings {
        auto_accept_trusted: true,
        ..Settings::default()
    }));
    let server = SignalingServer::start(
        "127.0.0.1:0".parse().unwrap(),
        server_identity,
        server_trust.clone(),
        SessionManager::new(),
        settings,
    )
    .await
    .unwrap();

    let client_identity = id("client");
    let client_trust = Arc::new(RwLock::new(
        TrustStore::load_or_create(&dir.path().join("client-trust.toml")).unwrap(),
    ));

    // First connect — unknown peer → goes to pending, server accepts manually.
    let first_dial = tokio::spawn({
        let trust = client_trust.clone();
        let ident = client_identity.clone();
        let addr = server.bind_addr;
        async move {
            connect_to_peer(
                addr,
                &ident,
                trust,
                Some(server_peer_id),
                Duration::from_secs(5),
            )
            .await
        }
    });
    let first_accept = tokio::spawn({
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
    let (first_dial_res, first_accept_res) = tokio::join!(first_dial, first_accept);
    let first_outcome = first_dial_res.unwrap().unwrap();
    first_accept_res.unwrap().unwrap();
    assert!(first_outcome.accepted, "first connect must be accepted");

    // Verify the dialer now has the server's token stored.
    assert!(
        client_trust.read().await.contains(&server_peer_id),
        "client TrustStore must contain the server after first accept"
    );

    // Second connect — dialer sends the stored token; server auto-accepts.
    let second_outcome = connect_to_peer(
        server.bind_addr,
        &client_identity,
        client_trust.clone(),
        Some(server_peer_id),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(
        second_outcome.accepted,
        "second connect must be auto-accepted"
    );

    // Pending queue must stay empty — no manual accept required.
    assert!(
        server.pending.list().await.is_empty(),
        "second connect must not land in pending queue"
    );
}
