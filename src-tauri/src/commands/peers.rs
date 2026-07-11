use crate::core::AppCore;
use crate::dto::{IdentityDto, PendingPeerDto};
use splitter_core::net::discovery::DiscoveredPeer;
use splitter_core::net::signaling::client_ops::find_conn_tx;
use splitter_core::net::signaling::{DeviceDescriptor, SignalingMessage, StreamAction};
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

#[tauri::command]
#[specta::specta]
pub async fn identity(core: State<'_, Arc<AppCore>>) -> Result<IdentityDto, String> {
    let id = core.identity.read().clone();
    Ok(IdentityDto {
        peer_id: id.peer_id.to_string(),
        peer_name: id.peer_name,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn discovered_peers(
    core: State<'_, Arc<AppCore>>,
) -> Result<Vec<DiscoveredPeer>, String> {
    Ok(core.peers.read().await.values().cloned().collect())
}

#[tauri::command]
#[specta::specta]
pub async fn pending_peers(core: State<'_, Arc<AppCore>>) -> Result<Vec<PendingPeerDto>, String> {
    Ok(core
        .server
        .pending
        .list()
        .await
        .iter()
        .map(PendingPeerDto::from)
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn accept_pending(core: State<'_, Arc<AppCore>>, index: u32) -> Result<String, String> {
    let server_peer_id = core.identity.read().peer_id;
    let (peer_id, _token) = splitter_core::net::signaling::server::accept_pending_as(
        &core.server.pending,
        &core.trust,
        &core.server.connections,
        &core.server.connection_established_tx,
        index as usize,
        Some(server_peer_id),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(peer_id.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn reject_pending(core: State<'_, Arc<AppCore>>, index: u32) -> Result<(), String> {
    core.server.pending.take(index as usize).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn connect_peer(
    core: State<'_, Arc<AppCore>>,
    host: String,
    port: u16,
    peer_id: Option<String>,
) -> Result<bool, String> {
    let addr = format!("{host}:{port}")
        .parse()
        .map_err(|_| format!("invalid address '{host}:{port}'"))?;
    let hint = match peer_id {
        Some(s) => Some(uuid::Uuid::parse_str(&s).map_err(|e| e.to_string())?),
        None => None,
    };
    let identity = core.identity.read().clone();
    let outcome = splitter_core::net::signaling::client::connect_to_peer(
        addr,
        &identity,
        core.trust.clone(),
        hint,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Some(pid) = outcome.remote_peer_id {
        let events = outcome.handle.events.subscribe();
        let addr = outcome.handle.remote_addr;
        let tx = outcome.handle.tx.clone();
        let connection_id = outcome.handle.connection_id;
        let previous = core.outgoing.write().await.insert(pid, outcome.handle);
        if let Some(old) = previous {
            old.shutdown();
        }
        core.local_disconnects.write().await.remove(&pid);
        crate::acceptor::spawn_acceptor((*core).clone(), pid, connection_id, events, addr);
        tx.send(SignalingMessage::DeviceListRequest {}).await.ok();
    }
    Ok(outcome.accepted)
}

#[tauri::command]
#[specta::specta]
pub async fn peer_devices(
    core: State<'_, Arc<AppCore>>,
    peer_id: String,
) -> Result<Vec<DeviceDescriptor>, String> {
    let pid = uuid::Uuid::parse_str(&peer_id).map_err(|e| e.to_string())?;
    let cached = core.remote_devices.read().await.get(&pid).cloned();
    if cached.is_none() {
        if let Some(tx) = find_conn_tx(&core.server.connections, &core.outgoing, pid).await {
            tx.send(SignalingMessage::DeviceListRequest {}).await.ok();
        }
    }
    Ok(cached.unwrap_or_default())
}

pub fn validate_device_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("o nome não pode ser vazio".into());
    }
    if trimmed.chars().count() > 40 {
        return Err("o nome deve ter no máximo 40 caracteres".into());
    }
    Ok(trimmed.to_string())
}

async fn broadcast_rename(core: &AppCore, peer_id: String, peer_name: String) {
    let msg = SignalingMessage::PeerRenamed { peer_id, peer_name };
    let conns: Vec<_> = {
        let g = core.server.connections.read().await;
        g.values().map(|c| c.tx.clone()).collect()
    };
    let outs: Vec<_> = {
        let g = core.outgoing.read().await;
        g.values().map(|c| c.tx.clone()).collect()
    };
    for tx in conns.into_iter().chain(outs) {
        let _ = tx.send(msg.clone()).await;
    }
}

#[tauri::command]
#[specta::specta]
pub async fn set_device_name(
    core: State<'_, Arc<AppCore>>,
    name: String,
) -> Result<IdentityDto, String> {
    let validated = validate_device_name(&name)?;
    let (peer_id, snapshot) = {
        let mut id = core.identity.write();
        id.peer_name = validated.clone();
        (id.peer_id.to_string(), id.clone())
    };
    let path = splitter_core::net::identity::identity_path().map_err(|e| e.to_string())?;
    snapshot.save_atomic(&path).map_err(|e| e.to_string())?;
    if let Some(handle) = core.discovery.get() {
        let port = core.server.bind_addr.port();
        if let Err(e) = handle.reannounce(&snapshot, port) {
            tracing::warn!("mDNS reannounce after rename failed: {e}");
        }
    }
    broadcast_rename(&core, peer_id.clone(), validated.clone()).await;
    Ok(IdentityDto {
        peer_id,
        peer_name: validated,
    })
}

pub(crate) async fn teardown_session(
    core: &AppCore,
    sid: splitter_core::SessionId,
) -> Result<(), String> {
    let snap = core.sessions.snapshot().await;
    let Some(sess) = snap.iter().find(|s| s.id == sid) else {
        return Ok(());
    };
    for stream in &sess.streams {
        if let Err(e) = core.stream_registry.close(&sid, stream.id).await {
            tracing::warn!(%sid, stream_id = %stream.id, "teardown_session: stream_registry.close error: {e}");
        }
        crate::commands::streams::notify_remote(
            core,
            sid.get(),
            stream.id.get(),
            StreamAction::Close,
        )
        .await;
    }

    let remote = sess.remote_peer_id;
    core.local_disconnects.write().await.insert(remote);
    let tx = {
        let g = core.server.connections.read().await;
        g.get(&remote).map(|c| c.tx.clone())
    };
    let tx = match tx {
        Some(t) => Some(t),
        None => core
            .outgoing
            .read()
            .await
            .get(&remote)
            .map(|c| c.tx.clone()),
    };
    if let Some(tx) = tx {
        let _ = tx
            .send(SignalingMessage::SessionResponse {
                session_id: sid.to_string(),
                accepted: false,
            })
            .await;
    }

    core.sessions.remove(&sid).await;

    // shutdown() aborts the connection task so the socket dies now, even if some
    // other task still holds a tx clone. The abort emits no Disconnected event,
    // so the acceptor sees the broadcast close and exits without reconnecting.
    if let Some(handle) = core.server.connections.write().await.remove(&remote) {
        handle.shutdown();
    }
    if let Some(handle) = core.outgoing.write().await.remove(&remote) {
        handle.shutdown();
    }
    core.remote_devices.write().await.remove(&remote);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn disconnect(core: State<'_, Arc<AppCore>>, session_id: String) -> Result<(), String> {
    let sid =
        splitter_core::SessionId(uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?);
    teardown_session(&core, sid).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::net::signaling::connection::spawn_peer_connection;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};

    #[test]
    fn validate_trims_and_rejects_empty_and_too_long() {
        assert_eq!(validate_device_name("  Studio  ").unwrap(), "Studio");
        assert!(validate_device_name("   ").is_err());
        assert!(validate_device_name(&"x".repeat(41)).is_err());
        assert_eq!(validate_device_name(&"x".repeat(40)).unwrap().len(), 40);
    }

    #[tokio::test]
    async fn teardown_marks_peer_as_locally_disconnected() {
        let dir = tempfile::tempdir().unwrap();
        let core = AppCore::init(dir.path()).await.unwrap();
        let remote = uuid::Uuid::new_v4();
        let local = core.identity.read().peer_id;
        let sid = core.sessions.open_outgoing(local, remote).await;

        teardown_session(&core, sid).await.unwrap();

        assert!(
            core.local_disconnects.read().await.contains(&remote),
            "local teardown must mark the peer so a late disconnect does not auto-reconnect"
        );
    }

    #[tokio::test]
    async fn teardown_evicts_session_drops_handle_and_closes_socket() {
        let dir = tempfile::tempdir().unwrap();
        let core = AppCore::init(dir.path()).await.unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client, accepted) =
            tokio::join!(TcpStream::connect(addr), async { listener.accept().await });
        let client = client.unwrap();
        let mut peer_end = accepted.unwrap().0;

        let remote = uuid::Uuid::new_v4();
        let handle = spawn_peer_connection(client, None).unwrap();
        // A lingering tx clone (as CLI's stream-open acceptor holds) must not
        // keep the socket alive: shutdown() aborts the task regardless.
        let _lingering_tx = handle.tx.clone();
        core.outgoing.write().await.insert(remote, handle);

        let local = core.identity.read().peer_id;
        let sid = core.sessions.open_outgoing(local, remote).await;

        teardown_session(&core, sid).await.unwrap();

        assert!(
            core.sessions.snapshot().await.is_empty(),
            "session must be evicted from the snapshot"
        );
        assert!(
            !core.outgoing.read().await.contains_key(&remote),
            "connection handle must be removed so its socket can die"
        );

        let socket_closed = tokio::time::timeout(Duration::from_secs(2), async {
            let mut buf = [0u8; 1024];
            loop {
                match peer_end.read(&mut buf).await {
                    Ok(0) | Err(_) => break true,
                    Ok(_) => continue,
                }
            }
        })
        .await
        .expect("socket should reach EOF within the timeout");
        assert!(socket_closed, "peer end must see the socket close");
    }
}
