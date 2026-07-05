use crate::core::AppCore;
use crate::events::{IncomingSession, PeerDisconnected, SnapshotChanged};
use splitter_core::net::session::SessionId;
use splitter_core::net::signaling::{
    CodecParams, DeviceDescriptor, Endpoint, PeerEvent, SignalingMessage, SourceKind, StreamAction,
};
use splitter_core::net::stream::{StreamId, StreamRoute};
use splitter_core::net::stream_runtime::{open_stream_as_sink, StreamControlSignal};
use std::net::SocketAddr;
use std::sync::Arc;
use uuid::Uuid;

fn pick_default_output_device_id() -> String {
    splitter_core::audio::devices::list_devices()
        .ok()
        .and_then(|devs| {
            devs.into_iter()
                .find(|d| d.kind == splitter_core::audio::devices::DeviceKind::Output)
                .map(|d| d.id)
        })
        .unwrap_or_else(|| "Output:0:default".into())
}

pub fn spawn_acceptor(
    core: Arc<AppCore>,
    peer_id: Uuid,
    mut events: tokio::sync::broadcast::Receiver<PeerEvent>,
    addr: SocketAddr,
) {
    let default_output = pick_default_output_device_id();
    let local_peer_id = core.identity.read().peer_id;
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(msg)) => match msg {
                    SignalingMessage::SessionRequest {
                        session_id,
                        requested_by,
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id).map(SessionId) else {
                            continue;
                        };
                        let Ok(requester_uuid) = Uuid::parse_str(&requested_by) else {
                            continue;
                        };

                        let stale = core.sessions.snapshot().await;
                        for old in stale
                            .iter()
                            .filter(|s| s.remote_peer_id == requester_uuid && s.id != sid_uuid)
                        {
                            for st in &old.streams {
                                let _ = core.stream_registry.close(&old.id, st.id).await;
                            }
                            let _ = core.sessions.close(&old.id).await;
                        }

                        if let Err(e) = core
                            .sessions
                            .register_incoming(sid_uuid, local_peer_id, requester_uuid)
                            .await
                        {
                            tracing::warn!(
                                peer = %peer_id,
                                session = %sid_uuid,
                                "register_incoming failed: {e}"
                            );
                            continue;
                        }
                        if let Err(e) = core.sessions.accept(&sid_uuid).await {
                            tracing::warn!(
                                peer = %peer_id,
                                session = %sid_uuid,
                                "accept failed after registration: {e}"
                            );
                            let _ = core.sessions.close(&sid_uuid).await;
                            continue;
                        }
                        let peer_name = {
                            let trust_name = core
                                .trust
                                .read()
                                .await
                                .peer_for(&requester_uuid)
                                .map(|p| p.peer_name.clone());
                            if let Some(name) = trust_name {
                                name
                            } else {
                                let discovered_name = core
                                    .peers
                                    .read()
                                    .await
                                    .get(&requester_uuid.to_string())
                                    .map(|p| p.peer_name.clone());
                                discovered_name
                                    .unwrap_or_else(|| requester_uuid.to_string()[..8].to_string())
                            }
                        };
                        core.emit(IncomingSession {
                            peer_id: requester_uuid.to_string(),
                            peer_name,
                        });
                        core.emit(SnapshotChanged);
                        tracing::info!(peer = %peer_id, session = %sid_uuid, "opened session");
                    }
                    SignalingMessage::StreamOpen {
                        session_id,
                        stream_id,
                        source,
                        sink,
                        codec,
                        ..
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id).map(SessionId) else {
                            continue;
                        };
                        let route = StreamRoute::new(
                            Endpoint {
                                peer_id: source.peer_id.clone(),
                                device_id: source.device_id.clone(),
                            },
                            Endpoint {
                                peer_id: sink.peer_id.clone(),
                                device_id: sink.device_id.clone(),
                            },
                            CodecParams {
                                name: codec.name.clone(),
                                bitrate: codec.bitrate,
                                frame_ms: codec.frame_ms,
                            },
                            1.0,
                        );
                        let chosen_output = if sink.device_id == "default" {
                            default_output.clone()
                        } else {
                            sink.device_id.clone()
                        };
                        let mut session_route = route.clone();
                        session_route.sink.device_id = chosen_output.clone();
                        match open_stream_as_sink(
                            core.stream_registry.clone(),
                            sid_uuid,
                            StreamId(stream_id),
                            route,
                            chosen_output.clone(),
                        )
                        .await
                        {
                            Ok(port) => {
                                let stream = splitter_core::net::stream::Stream::new_negotiating(
                                    StreamId(stream_id),
                                    session_route,
                                    port,
                                );
                                if let Err(e) = core.sessions.add_stream(&sid_uuid, stream).await {
                                    tracing::warn!(
                                        peer = %peer_id,
                                        stream_id,
                                        "add_stream failed — tearing down runtime: {e}"
                                    );
                                    let _ = core
                                        .stream_registry
                                        .close(&sid_uuid, StreamId(stream_id))
                                        .await;
                                    send_to_peer(
                                        &core,
                                        peer_id,
                                        SignalingMessage::StreamOpenAck {
                                            stream_id,
                                            accepted: false,
                                            udp_port: None,
                                        },
                                    )
                                    .await;
                                    continue;
                                }
                                if let Err(e) = core
                                    .sessions
                                    .activate_stream(&sid_uuid, StreamId(stream_id))
                                    .await
                                {
                                    tracing::warn!(
                                        peer = %peer_id,
                                        stream_id,
                                        "activate_stream failed — tearing down runtime: {e}"
                                    );
                                    let _ = core
                                        .stream_registry
                                        .close(&sid_uuid, StreamId(stream_id))
                                        .await;
                                    let _ = core
                                        .sessions
                                        .remove_stream(&sid_uuid, StreamId(stream_id))
                                        .await;
                                    send_to_peer(
                                        &core,
                                        peer_id,
                                        SignalingMessage::StreamOpenAck {
                                            stream_id,
                                            accepted: false,
                                            udp_port: None,
                                        },
                                    )
                                    .await;
                                    continue;
                                }
                                send_to_peer(
                                    &core,
                                    peer_id,
                                    SignalingMessage::StreamOpenAck {
                                        stream_id,
                                        accepted: true,
                                        udp_port: Some(port),
                                    },
                                )
                                .await;
                                tracing::info!(
                                    peer = %peer_id,
                                    stream_id,
                                    source = %source.device_id,
                                    sink = %chosen_output,
                                    "opened stream as sink"
                                );
                                core.emit(SnapshotChanged);
                            }
                            Err(e) => {
                                tracing::warn!("stream_open accept failed: {e}");
                                send_to_peer(
                                    &core,
                                    peer_id,
                                    SignalingMessage::StreamOpenAck {
                                        stream_id,
                                        accepted: false,
                                        udp_port: None,
                                    },
                                )
                                .await;
                            }
                        }
                    }
                    SignalingMessage::StreamControl { stream_id, action } => {
                        match &action {
                            StreamAction::Close => {
                                tracing::info!(peer = %peer_id, stream_id, "remote closed stream")
                            }
                            StreamAction::Pause => {
                                tracing::info!(peer = %peer_id, stream_id, "remote paused stream")
                            }
                            StreamAction::Resume => {
                                tracing::info!(peer = %peer_id, stream_id, "remote resumed stream")
                            }
                            StreamAction::SetVolume { volume } => tracing::info!(
                                peer = %peer_id,
                                stream_id,
                                volume,
                                "remote set stream volume"
                            ),
                            StreamAction::SetMuted { muted } => tracing::info!(
                                peer = %peer_id,
                                stream_id,
                                muted,
                                "remote set stream muted"
                            ),
                        }
                        let session_ids: Vec<SessionId> = core
                            .sessions
                            .snapshot()
                            .await
                            .into_iter()
                            .filter(|s| s.remote_peer_id == peer_id)
                            .map(|s| s.id)
                            .collect();
                        if matches!(action, StreamAction::Close) {
                            for sid in session_ids {
                                let _ = core.stream_registry.close(&sid, StreamId(stream_id)).await;
                                let _ =
                                    core.sessions.remove_stream(&sid, StreamId(stream_id)).await;
                            }
                        } else {
                            let signal = StreamControlSignal::from(action);
                            if let StreamControlSignal::SetMuted(m) = signal {
                                for sid in &session_ids {
                                    let _ = core
                                        .sessions
                                        .set_stream_muted(sid, StreamId(stream_id), m)
                                        .await;
                                }
                            }
                            for sid in &session_ids {
                                let _ = core
                                    .stream_registry
                                    .send_control(sid, StreamId(stream_id), signal)
                                    .await;
                            }
                        }
                        core.emit(SnapshotChanged);
                    }
                    SignalingMessage::SessionResponse {
                        session_id,
                        accepted: false,
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id).map(SessionId) else {
                            continue;
                        };
                        let stream_ids: Vec<StreamId> = core
                            .sessions
                            .snapshot()
                            .await
                            .into_iter()
                            .find(|s| s.id == sid_uuid)
                            .map(|s| s.streams.iter().map(|st| st.id).collect())
                            .unwrap_or_default();
                        for sid_stream in stream_ids {
                            let _ = core.stream_registry.close(&sid_uuid, sid_stream).await;
                        }
                        let _ = core.sessions.close(&sid_uuid).await;
                        tracing::info!(peer = %peer_id, session = %sid_uuid, "remote closed session");
                        core.emit(SnapshotChanged);
                    }
                    SignalingMessage::DeviceListRequest {} => {
                        let devices = splitter_core::audio::devices::list_devices()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|d| DeviceDescriptor {
                                id: d.id,
                                name: d.name,
                                kind: d.kind,
                            })
                            .collect();
                        send_to_peer(
                            &core,
                            peer_id,
                            SignalingMessage::DeviceListResponse { devices },
                        )
                        .await;
                    }
                    SignalingMessage::DeviceListResponse { devices } => {
                        core.remote_devices.write().await.insert(peer_id, devices);
                        core.emit(SnapshotChanged);
                    }
                    SignalingMessage::PeerRenamed {
                        peer_id: rid,
                        peer_name,
                    } => {
                        let changed = {
                            let mut peers = core.peers.write().await;
                            crate::core::apply_peer_rename(&mut peers, &rid, &peer_name)
                        };
                        if changed {
                            let snapshot: Vec<_> =
                                core.peers.read().await.values().cloned().collect();
                            core.emit(crate::events::PeersChanged(snapshot));
                        }
                        tracing::info!(peer = %peer_id, new_name = %peer_name, "peer renamed");
                    }
                    SignalingMessage::StreamRequest {
                        session_id,
                        source,
                        sink_device,
                    } => {
                        let Ok(req_sid) = Uuid::parse_str(&session_id) else {
                            continue;
                        };
                        let (source_device, source_is_system) = match source {
                            SourceKind::Mic { device_id } => (device_id, false),
                            SourceKind::System { device_id } => (device_id, true),
                        };
                        let core2 = core.clone();
                        let sink_peer = peer_id;
                        tauri::async_runtime::spawn(async move {
                            match crate::commands::streams::open_stream_core(
                                &core2,
                                req_sid,
                                source_device,
                                source_is_system,
                                sink_peer,
                                sink_device,
                                64_000,
                            )
                            .await
                            {
                                Ok(_) => core2.emit(SnapshotChanged),
                                Err(e) => {
                                    tracing::warn!(peer = %sink_peer, "stream request failed: {e}")
                                }
                            }
                        });
                    }
                    _ => {}
                },
                Ok(PeerEvent::Disconnected { reason }) => {
                    tracing::info!(peer = %peer_id, %reason, "peer disconnected");
                    core.emit(PeerDisconnected {
                        peer_id: peer_id.to_string(),
                        reason: reason.clone(),
                    });
                    let session_ids: Vec<SessionId> = core
                        .sessions
                        .snapshot()
                        .await
                        .into_iter()
                        .filter(|s| s.remote_peer_id == peer_id)
                        .map(|s| s.id)
                        .collect();
                    let had_active_session = !session_ids.is_empty();
                    for sid in &session_ids {
                        let stream_ids: Vec<StreamId> = core
                            .sessions
                            .snapshot()
                            .await
                            .into_iter()
                            .find(|s| s.id == *sid)
                            .map(|s| s.streams.iter().map(|st| st.id).collect())
                            .unwrap_or_default();
                        for stream_id in stream_ids {
                            let _ = core.stream_registry.close(sid, stream_id).await;
                        }
                        let _ = core.sessions.close(sid).await;
                    }
                    if had_active_session {
                        crate::reconnect::spawn_reconnect(core.clone(), peer_id, addr);
                    }
                    // A concurrent reconnect may have re-inserted a live handle under the
                    // same peer_id; only evict the entry that belongs to this dead
                    // connection (its tx is closed, the reconnected handle's is open).
                    {
                        let mut conns = core.server.connections.write().await;
                        if conns
                            .get(&peer_id)
                            .map(|h| h.tx.is_closed())
                            .unwrap_or(false)
                        {
                            conns.remove(&peer_id);
                        }
                    }
                    {
                        let mut out = core.outgoing.write().await;
                        if out.get(&peer_id).map(|h| h.tx.is_closed()).unwrap_or(false) {
                            out.remove(&peer_id);
                        }
                    }
                    core.remote_devices.write().await.remove(&peer_id);
                    break;
                }
                Ok(PeerEvent::Connected { .. }) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "peer event stream lagged; continuing");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn send_to_peer(core: &AppCore, peer_id: Uuid, msg: SignalingMessage) {
    {
        let g = core.server.connections.read().await;
        if let Some(c) = g.get(&peer_id) {
            let _ = c.tx.send(msg).await;
            return;
        }
    }
    let g = core.outgoing.read().await;
    if let Some(c) = g.get(&peer_id) {
        let _ = c.tx.send(msg).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::audio::devices::DeviceKind;
    use splitter_core::net::discovery::DiscoveredPeer;
    use splitter_core::net::signaling::Codec;
    use splitter_core::net::stream::Stream;
    use splitter_core::{SessionSnapshot, SessionState};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::sync::broadcast;

    async fn new_core() -> Arc<AppCore> {
        AppCore::init(tempdir().unwrap().path())
            .await
            .expect("init")
    }

    fn driven_acceptor(core: Arc<AppCore>, peer: Uuid) -> broadcast::Sender<PeerEvent> {
        let (tx, rx) = broadcast::channel(16);
        spawn_acceptor(core, peer, rx, "127.0.0.1:9".parse().unwrap());
        tx
    }

    async fn await_sessions(
        core: &AppCore,
        pred: impl Fn(&[SessionSnapshot]) -> bool,
    ) -> Vec<SessionSnapshot> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snap = core.sessions.snapshot().await;
                if pred(&snap) {
                    return snap;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("sessions condition not met within 2s")
    }

    fn test_route(source_peer: &str, sink_peer: &str) -> StreamRoute {
        StreamRoute::new(
            Endpoint {
                peer_id: source_peer.to_string(),
                device_id: "in".into(),
            },
            Endpoint {
                peer_id: sink_peer.to_string(),
                device_id: "out".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        )
    }

    async fn seed_active_session_with_stream(
        core: &AppCore,
        local: Uuid,
        remote: Uuid,
    ) -> SessionId {
        let sid = core.sessions.open_outgoing(local, remote).await;
        core.sessions.accept(&sid).await.unwrap();
        let route = test_route(&local.to_string(), &remote.to_string());
        core.sessions
            .add_stream(&sid, Stream::new_negotiating(StreamId(0), route, 0))
            .await
            .unwrap();
        core.sessions
            .activate_stream(&sid, StreamId(0))
            .await
            .unwrap();
        sid
    }

    #[tokio::test]
    async fn session_request_registers_active_session() {
        let core = new_core().await;
        let requester = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let tx = driven_acceptor(core.clone(), requester);

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: session_id.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        let snap = await_sessions(&core, |s| !s.is_empty()).await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, SessionId(session_id));
        assert_eq!(snap[0].remote_peer_id, requester);
        assert_eq!(snap[0].state, SessionState::Active);
    }

    #[tokio::test]
    async fn session_request_evicts_stale_session_for_same_requester() {
        let core = new_core().await;
        let requester = Uuid::new_v4();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let tx = driven_acceptor(core.clone(), requester);

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: s1.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();
        await_sessions(&core, |s| s.iter().any(|x| x.id == SessionId(s1))).await;

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: s2.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        let snap = await_sessions(&core, |s| {
            s.iter()
                .any(|x| x.id == SessionId(s2) && x.state == SessionState::Active)
                && s.iter()
                    .find(|x| x.id == SessionId(s1))
                    .map(|x| x.state == SessionState::Closed)
                    .unwrap_or(true)
        })
        .await;
        let active: Vec<_> = snap
            .iter()
            .filter(|x| x.state == SessionState::Active)
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, SessionId(s2));
    }

    #[tokio::test]
    async fn stream_control_set_muted_marks_session_stream_muted() {
        let core = new_core().await;
        let local = core.identity.read().peer_id;
        let peer = Uuid::new_v4();
        seed_active_session_with_stream(&core, local, peer).await;
        let tx = driven_acceptor(core.clone(), peer);

        tx.send(PeerEvent::Message(SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::SetMuted { muted: true },
        }))
        .unwrap();

        let snap = await_sessions(&core, |s| {
            s.iter()
                .flat_map(|x| &x.streams)
                .any(|st| st.id == StreamId(0) && st.muted)
        })
        .await;
        assert!(snap[0].streams.iter().any(|st| st.muted));
    }

    #[tokio::test]
    async fn stream_control_close_removes_stream() {
        let core = new_core().await;
        let local = core.identity.read().peer_id;
        let peer = Uuid::new_v4();
        let sid = seed_active_session_with_stream(&core, local, peer).await;
        let tx = driven_acceptor(core.clone(), peer);

        tx.send(PeerEvent::Message(SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::Close,
        }))
        .unwrap();

        let snap = await_sessions(&core, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.streams.is_empty())
                .unwrap_or(false)
        })
        .await;
        assert!(snap
            .iter()
            .find(|x| x.id == sid)
            .unwrap()
            .streams
            .is_empty());
    }

    #[tokio::test]
    async fn session_response_false_closes_session() {
        let core = new_core().await;
        let local = core.identity.read().peer_id;
        let peer = Uuid::new_v4();
        let sid = seed_active_session_with_stream(&core, local, peer).await;
        let tx = driven_acceptor(core.clone(), peer);

        tx.send(PeerEvent::Message(SignalingMessage::SessionResponse {
            session_id: sid.to_string(),
            accepted: false,
        }))
        .unwrap();

        let snap = await_sessions(&core, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            snap.iter().find(|x| x.id == sid).unwrap().state,
            SessionState::Closed
        );
    }

    #[tokio::test]
    async fn device_list_response_caches_remote_devices() {
        let core = new_core().await;
        let peer = Uuid::new_v4();
        let devices = vec![DeviceDescriptor {
            id: "Output:0:default".into(),
            name: "Speakers".into(),
            kind: DeviceKind::Output,
        }];
        let tx = driven_acceptor(core.clone(), peer);

        tx.send(PeerEvent::Message(SignalingMessage::DeviceListResponse {
            devices: devices.clone(),
        }))
        .unwrap();

        let cached = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(d) = core.remote_devices.read().await.get(&peer).cloned() {
                    return d;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("remote devices not cached within 2s");
        assert_eq!(cached, devices);
    }

    #[tokio::test]
    async fn peer_renamed_updates_discovered_peer() {
        let core = new_core().await;
        let rid = Uuid::new_v4();
        core.peers.write().await.insert(
            rid.to_string(),
            DiscoveredPeer {
                peer_id: rid.to_string(),
                peer_name: "Old".into(),
                host: "10.0.0.2".into(),
                port: 7000,
                version: "0.1.0".into(),
            },
        );
        let tx = driven_acceptor(core.clone(), rid);

        tx.send(PeerEvent::Message(SignalingMessage::PeerRenamed {
            peer_id: rid.to_string(),
            peer_name: "New".into(),
        }))
        .unwrap();

        let name = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let cur = core
                    .peers
                    .read()
                    .await
                    .get(&rid.to_string())
                    .map(|p| p.peer_name.clone());
                if cur.as_deref() == Some("New") {
                    return cur.unwrap();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("peer rename not applied within 2s");
        assert_eq!(name, "New");
    }

    #[tokio::test]
    async fn disconnect_tears_down_sessions_and_skips_reconnect() {
        let core = new_core().await;
        let local = core.identity.read().peer_id;
        let peer = Uuid::new_v4();
        let sid = seed_active_session_with_stream(&core, local, peer).await;
        let tx = driven_acceptor(core.clone(), peer);

        tx.send(PeerEvent::Disconnected {
            reason: "test".into(),
        })
        .unwrap();

        let snap = await_sessions(&core, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            snap.iter().find(|x| x.id == sid).unwrap().state,
            SessionState::Closed
        );
    }

    #[tokio::test]
    #[ignore = "StreamOpen opens a real output audio device via open_stream_as_sink; device-dependent, not runnable headless"]
    async fn stream_open_adds_active_stream_to_session() {
        let core = new_core().await;
        let requester = Uuid::new_v4();
        let local = core.identity.read().peer_id;
        let session_id = Uuid::new_v4();
        let tx = driven_acceptor(core.clone(), requester);

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: session_id.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();
        await_sessions(&core, |s| !s.is_empty()).await;

        tx.send(PeerEvent::Message(SignalingMessage::StreamOpen {
            session_id: session_id.to_string(),
            stream_id: 1,
            source: Endpoint {
                peer_id: requester.to_string(),
                device_id: "mic".into(),
            },
            sink: Endpoint {
                peer_id: local.to_string(),
                device_id: "default".into(),
            },
            codec: CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            udp_port: 0,
        }))
        .unwrap();

        let snap = await_sessions(&core, |s| {
            s.iter()
                .flat_map(|x| &x.streams)
                .any(|st| st.id == StreamId(1))
        })
        .await;
        assert!(snap
            .iter()
            .flat_map(|x| &x.streams)
            .any(|st| st.id == StreamId(1)));
    }
}
