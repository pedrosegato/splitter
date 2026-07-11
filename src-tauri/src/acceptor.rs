use crate::core::AppCore;
use crate::events::{IncomingSession, PeerDisconnected, SnapshotChanged};
use splitter_core::net::session::SessionId;
use splitter_core::net::signaling::client_ops::{find_conn, ConnEndpoints};
use splitter_core::net::signaling::{
    spawn_control_plane, spawn_reconnect, ConnectOutcome, ControlPlaneDeps, ControlPlaneHost,
    ControlPlaneObserver, PeerEvent, ReconnectDriver, SourceKind, StreamAction,
};
use splitter_core::{PeerIdentity, TrustStore};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
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

pub struct TauriControlPlane {
    pub core: Arc<AppCore>,
}

fn make_deps(core: &Arc<AppCore>) -> Arc<ControlPlaneDeps> {
    Arc::new(ControlPlaneDeps {
        sessions: core.sessions.clone(),
        stream_registry: core.stream_registry.clone(),
        local_peer_id: core.identity.read().peer_id,
        default_output: pick_default_output_device_id(),
    })
}

fn observer(core: &Arc<AppCore>) -> Arc<TauriControlPlane> {
    Arc::new(TauriControlPlane { core: core.clone() })
}

pub fn spawn_acceptor(
    core: Arc<AppCore>,
    peer_id: Uuid,
    events: broadcast::Receiver<PeerEvent>,
    _addr: SocketAddr,
) {
    let deps = make_deps(&core);
    let obs = observer(&core);
    tokio::spawn(async move {
        let (conn_tx, connection_id) =
            match find_conn(&core.server.connections, &core.outgoing, peer_id).await {
                Some(c) => (c.tx, Some(c.connection_id)),
                None => {
                    let (tx, _rx) = mpsc::channel(1);
                    (tx, None)
                }
            };
        spawn_control_plane(deps, peer_id, conn_tx, events, obs, connection_id);
    });
}

#[async_trait::async_trait]
impl ControlPlaneObserver for TauriControlPlane {
    async fn on_session_opened(&self, _peer_id: Uuid, requester: Uuid, _session: SessionId) {
        let peer_name = {
            let trust_name = self
                .core
                .trust
                .read()
                .await
                .peer_for(&requester)
                .map(|p| p.peer_name.clone());
            if let Some(name) = trust_name {
                name
            } else {
                let discovered_name = self
                    .core
                    .peers
                    .read()
                    .await
                    .get(&requester.to_string())
                    .map(|p| p.peer_name.clone());
                discovered_name.unwrap_or_else(|| requester.to_string()[..8].to_string())
            }
        };
        self.core.emit(IncomingSession {
            peer_id: requester.to_string(),
            peer_name,
        });
        self.core.emit(SnapshotChanged);
    }

    async fn on_stream_opened(
        &self,
        peer_id: Uuid,
        stream_id: u8,
        source_device: &str,
        sink_device: &str,
    ) {
        tracing::info!(
            peer = %peer_id,
            stream_id,
            source = %source_device,
            sink = %sink_device,
            "opened stream as sink"
        );
        self.core.emit(SnapshotChanged);
    }

    async fn on_stream_control(&self, peer_id: Uuid, stream_id: u8, action: &StreamAction) {
        match action {
            StreamAction::Close => {
                tracing::info!(peer = %peer_id, stream_id, "remote closed stream")
            }
            StreamAction::Pause => {
                tracing::info!(peer = %peer_id, stream_id, "remote paused stream")
            }
            StreamAction::Resume => {
                tracing::info!(peer = %peer_id, stream_id, "remote resumed stream")
            }
            StreamAction::SetVolume { volume } => {
                tracing::info!(peer = %peer_id, stream_id, volume, "remote set stream volume")
            }
            StreamAction::SetMuted { muted } => {
                tracing::info!(peer = %peer_id, stream_id, muted, "remote set stream muted")
            }
        }
        self.core.emit(SnapshotChanged);
    }

    async fn on_session_closed(&self, _peer_id: Uuid, _session: SessionId) {
        self.core.emit(SnapshotChanged);
    }

    async fn on_devices_received(
        &self,
        peer_id: Uuid,
        devices: Vec<splitter_core::net::signaling::DeviceDescriptor>,
    ) {
        self.core
            .remote_devices
            .write()
            .await
            .insert(peer_id, devices);
        self.core.emit(SnapshotChanged);
    }

    async fn on_peer_renamed(&self, peer_id: &str, peer_name: &str) {
        let changed = {
            let mut peers = self.core.peers.write().await;
            crate::core::apply_peer_rename(&mut peers, peer_id, peer_name)
        };
        if changed {
            let snapshot: Vec<_> = self.core.peers.read().await.values().cloned().collect();
            self.core.emit(crate::events::PeersChanged(snapshot));
        }
    }

    async fn on_stream_requested(
        &self,
        peer_id: Uuid,
        session_id: Uuid,
        source: SourceKind,
        sink_device: String,
    ) {
        let (source_device, source_is_system) = match source {
            SourceKind::Mic { device_id } => (device_id, false),
            SourceKind::System { device_id } => (device_id, true),
        };
        let core = self.core.clone();
        tauri::async_runtime::spawn(async move {
            match crate::commands::streams::open_stream_core(
                &core,
                session_id,
                source_device,
                source_is_system,
                peer_id,
                sink_device,
                64_000,
            )
            .await
            {
                Ok(_) => core.emit(SnapshotChanged),
                Err(e) => tracing::warn!(peer = %peer_id, "stream request failed: {e}"),
            }
        });
    }

    async fn on_peer_disconnected(&self, peer_id: Uuid, reason: &str, had_active_session: bool) {
        self.core.emit(PeerDisconnected {
            peer_id: peer_id.to_string(),
            reason: reason.to_string(),
        });
        if had_active_session {
            spawn_reconnect(peer_id, observer(&self.core));
        }
        // A concurrent reconnect may have re-inserted a live handle under the
        // same peer_id; only evict the entry that belongs to this dead
        // connection (its tx is closed, the reconnected handle's is open).
        {
            let mut conns = self.core.server.connections.write().await;
            if conns
                .get(&peer_id)
                .map(|h| h.tx.is_closed())
                .unwrap_or(false)
            {
                conns.remove(&peer_id);
            }
        }
        {
            let mut out = self.core.outgoing.write().await;
            if out.get(&peer_id).map(|h| h.tx.is_closed()).unwrap_or(false) {
                out.remove(&peer_id);
            }
        }
        self.core.remote_devices.write().await.remove(&peer_id);
    }
}

#[async_trait::async_trait]
impl ReconnectDriver for TauriControlPlane {
    fn identity(&self) -> PeerIdentity {
        self.core.identity.read().clone()
    }

    fn trust(&self) -> Arc<RwLock<TrustStore>> {
        self.core.trust.clone()
    }

    async fn resolve_addr(&self, peer_id: Uuid) -> Option<SocketAddr> {
        let mdns_addr = {
            let map = self.core.peers.read().await;
            map.values()
                .find(|p| p.peer_id == peer_id.to_string())
                .map(|p| format!("{}:{}", p.host, p.port))
                .and_then(|s| s.parse::<SocketAddr>().ok())
        };
        if mdns_addr.is_some() {
            return mdns_addr;
        }
        let out = self.core.outgoing.read().await;
        out.get(&peer_id).map(|h| h.remote_addr)
    }

    async fn still_discoverable(&self, peer_id: Uuid) -> bool {
        self.core
            .peers
            .read()
            .await
            .contains_key(&peer_id.to_string())
    }

    async fn on_reconnected(&self, _peer_id: Uuid, outcome: ConnectOutcome) {
        if let Some(pid) = outcome.remote_peer_id {
            let events = outcome.handle.events.subscribe();
            let reconnect_addr = outcome.handle.remote_addr;
            self.core.outgoing.write().await.insert(pid, outcome.handle);
            spawn_acceptor(self.core.clone(), pid, events, reconnect_addr);
        }
    }

    async fn on_reconnected_display(&self, peer_id: Uuid) {
        tracing::info!(%peer_id, "reconnected to peer");
    }

    async fn on_reconnect_failed_display(&self, peer_id: Uuid) {
        tracing::warn!(%peer_id, "reconnect exhausted all attempts");
    }
}

#[async_trait::async_trait]
impl ControlPlaneHost for TauriControlPlane {
    fn spawn_loop(&self, peer_id: Uuid, endpoints: ConnEndpoints) {
        let connection_id = endpoints.connection_id;
        spawn_control_plane(
            make_deps(&self.core),
            peer_id,
            endpoints.tx,
            endpoints.events.subscribe(),
            observer(&self.core),
            Some(connection_id),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::audio::devices::DeviceKind;
    use splitter_core::net::discovery::DiscoveredPeer;
    use splitter_core::net::signaling::{
        Codec, CodecParams, DeviceDescriptor, Endpoint, SignalingMessage,
    };
    use splitter_core::net::stream::{Stream, StreamId, StreamRoute};
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
