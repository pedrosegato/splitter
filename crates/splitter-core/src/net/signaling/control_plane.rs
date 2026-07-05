use crate::net::manager::SessionManager;
use crate::net::session::SessionId;
use crate::net::signaling::{
    CodecParams, DeviceDescriptor, Endpoint, PeerEvent, SignalingMessage, SourceKind, StreamAction,
};
use crate::net::stream::{Stream, StreamId, StreamRoute};
use crate::net::stream_runtime::{open_stream_as_sink, StreamControlSignal, StreamRegistry};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

type ConnTx = mpsc::Sender<SignalingMessage>;

pub struct ControlPlaneDeps {
    pub sessions: Arc<SessionManager>,
    pub stream_registry: Arc<StreamRegistry>,
    pub local_peer_id: Uuid,
    pub default_output: String,
}

#[async_trait::async_trait]
pub trait ControlPlaneObserver: Send + Sync + 'static {
    async fn on_session_opened(&self, peer_id: Uuid, requester: Uuid, session: SessionId);
    async fn on_stream_opened(
        &self,
        peer_id: Uuid,
        stream_id: u8,
        source_device: &str,
        sink_device: &str,
    );
    async fn on_stream_control(&self, peer_id: Uuid, stream_id: u8, action: &StreamAction);
    async fn on_session_closed(&self, peer_id: Uuid, session: SessionId);
    async fn on_peer_disconnected(&self, peer_id: Uuid, reason: &str, had_active_session: bool);

    async fn on_devices_received(&self, peer_id: Uuid, devices: Vec<DeviceDescriptor>) {
        let _ = (peer_id, devices);
    }
    async fn on_peer_renamed(&self, peer_id: &str, peer_name: &str) {
        let _ = (peer_id, peer_name);
    }
    async fn on_stream_requested(
        &self,
        peer_id: Uuid,
        session_id: Uuid,
        source: SourceKind,
        sink_device: String,
    ) {
        let _ = (peer_id, session_id, source, sink_device);
    }
}

pub fn spawn_control_plane(
    deps: Arc<ControlPlaneDeps>,
    peer_id: Uuid,
    conn_tx: ConnTx,
    mut events: broadcast::Receiver<PeerEvent>,
    observer: Arc<dyn ControlPlaneObserver>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(msg)) => {
                    handle_message(&deps, peer_id, &conn_tx, observer.as_ref(), msg).await;
                }
                Ok(PeerEvent::Disconnected { reason }) => {
                    handle_disconnected(&deps, peer_id, observer.as_ref(), &reason).await;
                    break;
                }
                Ok(PeerEvent::Connected { .. }) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "peer event stream lagged; continuing");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn handle_message(
    deps: &ControlPlaneDeps,
    peer_id: Uuid,
    conn_tx: &ConnTx,
    observer: &dyn ControlPlaneObserver,
    msg: SignalingMessage,
) {
    match msg {
        SignalingMessage::SessionRequest {
            session_id,
            requested_by,
        } => handle_session_request(deps, peer_id, observer, &session_id, &requested_by).await,
        SignalingMessage::StreamOpen {
            session_id,
            stream_id,
            source,
            sink,
            codec,
            ..
        } => {
            handle_stream_open(
                deps, peer_id, conn_tx, observer, session_id, stream_id, source, sink, codec,
            )
            .await
        }
        SignalingMessage::StreamControl { stream_id, action } => {
            handle_stream_control(deps, peer_id, observer, stream_id, action).await
        }
        SignalingMessage::SessionResponse {
            session_id,
            accepted: false,
        } => handle_session_response_close(deps, peer_id, observer, &session_id).await,
        SignalingMessage::DeviceListRequest {} => {
            let devices = crate::audio::devices::list_devices()
                .unwrap_or_default()
                .into_iter()
                .map(|d| DeviceDescriptor {
                    id: d.id,
                    name: d.name,
                    kind: d.kind,
                })
                .collect();
            let _ = conn_tx
                .send(SignalingMessage::DeviceListResponse { devices })
                .await;
        }
        SignalingMessage::DeviceListResponse { devices } => {
            observer.on_devices_received(peer_id, devices).await;
        }
        SignalingMessage::PeerRenamed {
            peer_id: rid,
            peer_name,
        } => {
            observer.on_peer_renamed(&rid, &peer_name).await;
            tracing::info!(peer = %peer_id, new_name = %peer_name, "peer renamed");
        }
        SignalingMessage::StreamRequest {
            session_id,
            source,
            sink_device,
        } => {
            let Ok(req_sid) = Uuid::parse_str(&session_id) else {
                return;
            };
            observer
                .on_stream_requested(peer_id, req_sid, source, sink_device)
                .await;
        }
        _ => {}
    }
}

async fn handle_session_request(
    deps: &ControlPlaneDeps,
    peer_id: Uuid,
    observer: &dyn ControlPlaneObserver,
    session_id: &str,
    requested_by: &str,
) {
    let Ok(sid) = Uuid::parse_str(session_id).map(SessionId) else {
        return;
    };
    let Ok(requester) = Uuid::parse_str(requested_by) else {
        return;
    };

    let stale = deps.sessions.snapshot().await;
    for old in stale
        .iter()
        .filter(|s| s.remote_peer_id == requester && s.id != sid)
    {
        for st in &old.streams {
            let _ = deps.stream_registry.close(&old.id, st.id).await;
        }
        let _ = deps.sessions.close(&old.id).await;
    }

    if let Err(e) = deps
        .sessions
        .register_incoming(sid, deps.local_peer_id, requester)
        .await
    {
        tracing::warn!(peer = %peer_id, session = %sid, "register_incoming failed: {e}");
        return;
    }
    if let Err(e) = deps.sessions.accept(&sid).await {
        tracing::warn!(peer = %peer_id, session = %sid, "accept failed after registration: {e}");
        let _ = deps.sessions.close(&sid).await;
        return;
    }
    observer.on_session_opened(peer_id, requester, sid).await;
    tracing::info!(peer = %peer_id, session = %sid, "opened session");
}

#[allow(clippy::too_many_arguments)]
async fn handle_stream_open(
    deps: &ControlPlaneDeps,
    peer_id: Uuid,
    conn_tx: &ConnTx,
    observer: &dyn ControlPlaneObserver,
    session_id: String,
    stream_id: u8,
    source: Endpoint,
    sink: Endpoint,
    codec: CodecParams,
) {
    let Ok(sid) = Uuid::parse_str(&session_id).map(SessionId) else {
        return;
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
        deps.default_output.clone()
    } else {
        sink.device_id.clone()
    };
    let mut session_route = route.clone();
    session_route.sink.device_id = chosen_output.clone();

    let port = match open_stream_as_sink(
        deps.stream_registry.clone(),
        sid,
        StreamId(stream_id),
        route,
        chosen_output.clone(),
    )
    .await
    {
        Ok(port) => port,
        Err(e) => {
            tracing::warn!("stream_open accept failed: {e}");
            let _ = conn_tx
                .send(SignalingMessage::StreamOpenAck {
                    stream_id,
                    accepted: false,
                    udp_port: None,
                })
                .await;
            return;
        }
    };

    let stream = Stream::new_negotiating(StreamId(stream_id), session_route, port);
    if let Err(e) = deps.sessions.add_stream(&sid, stream).await {
        tracing::warn!(peer = %peer_id, stream_id, "add_stream failed — tearing down runtime: {e}");
        let _ = deps.stream_registry.close(&sid, StreamId(stream_id)).await;
        let _ = conn_tx
            .send(SignalingMessage::StreamOpenAck {
                stream_id,
                accepted: false,
                udp_port: None,
            })
            .await;
        return;
    }
    if let Err(e) = deps.sessions.activate_stream(&sid, StreamId(stream_id)).await {
        tracing::warn!(peer = %peer_id, stream_id, "activate_stream failed — tearing down runtime: {e}");
        let _ = deps.stream_registry.close(&sid, StreamId(stream_id)).await;
        let _ = deps.sessions.remove_stream(&sid, StreamId(stream_id)).await;
        let _ = conn_tx
            .send(SignalingMessage::StreamOpenAck {
                stream_id,
                accepted: false,
                udp_port: None,
            })
            .await;
        return;
    }
    let _ = conn_tx
        .send(SignalingMessage::StreamOpenAck {
            stream_id,
            accepted: true,
            udp_port: Some(port),
        })
        .await;
    observer
        .on_stream_opened(peer_id, stream_id, &source.device_id, &chosen_output)
        .await;
}

async fn handle_stream_control(
    deps: &ControlPlaneDeps,
    peer_id: Uuid,
    observer: &dyn ControlPlaneObserver,
    stream_id: u8,
    action: StreamAction,
) {
    let session_ids: Vec<SessionId> = deps
        .sessions
        .snapshot()
        .await
        .into_iter()
        .filter(|s| s.remote_peer_id == peer_id)
        .map(|s| s.id)
        .collect();
    if matches!(action, StreamAction::Close) {
        for sid in &session_ids {
            let _ = deps.stream_registry.close(sid, StreamId(stream_id)).await;
            let _ = deps.sessions.remove_stream(sid, StreamId(stream_id)).await;
        }
    } else {
        let signal = StreamControlSignal::from(action.clone());
        if let StreamControlSignal::SetMuted(m) = signal {
            for sid in &session_ids {
                let _ = deps
                    .sessions
                    .set_stream_muted(sid, StreamId(stream_id), m)
                    .await;
            }
        }
        for sid in &session_ids {
            let _ = deps
                .stream_registry
                .send_control(sid, StreamId(stream_id), signal)
                .await;
        }
    }
    observer.on_stream_control(peer_id, stream_id, &action).await;
}

async fn handle_session_response_close(
    deps: &ControlPlaneDeps,
    peer_id: Uuid,
    observer: &dyn ControlPlaneObserver,
    session_id: &str,
) {
    let Ok(sid) = Uuid::parse_str(session_id).map(SessionId) else {
        return;
    };
    let stream_ids: Vec<StreamId> = deps
        .sessions
        .snapshot()
        .await
        .into_iter()
        .find(|s| s.id == sid)
        .map(|s| s.streams.iter().map(|st| st.id).collect())
        .unwrap_or_default();
    for stream_id in stream_ids {
        let _ = deps.stream_registry.close(&sid, stream_id).await;
    }
    let _ = deps.sessions.close(&sid).await;
    tracing::info!(peer = %peer_id, session = %sid, "remote closed session");
    observer.on_session_closed(peer_id, sid).await;
}

async fn handle_disconnected(
    deps: &ControlPlaneDeps,
    peer_id: Uuid,
    observer: &dyn ControlPlaneObserver,
    reason: &str,
) {
    tracing::info!(peer = %peer_id, %reason, "peer disconnected");
    let session_ids: Vec<SessionId> = deps
        .sessions
        .snapshot()
        .await
        .into_iter()
        .filter(|s| s.remote_peer_id == peer_id)
        .map(|s| s.id)
        .collect();
    let had_active_session = !session_ids.is_empty();
    for sid in &session_ids {
        let stream_ids: Vec<StreamId> = deps
            .sessions
            .snapshot()
            .await
            .into_iter()
            .find(|s| s.id == *sid)
            .map(|s| s.streams.iter().map(|st| st.id).collect())
            .unwrap_or_default();
        for stream_id in stream_ids {
            let _ = deps.stream_registry.close(sid, stream_id).await;
        }
        let _ = deps.sessions.close(sid).await;
    }
    observer
        .on_peer_disconnected(peer_id, reason, had_active_session)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::session::SessionState;
    use crate::net::signaling::Codec;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct Recorder {
        sessions_opened: Mutex<Vec<(Uuid, SessionId)>>,
        sessions_closed: Mutex<Vec<SessionId>>,
        controls: Mutex<Vec<(u8, StreamAction)>>,
        devices: Mutex<Vec<(Uuid, Vec<DeviceDescriptor>)>>,
        renames: Mutex<Vec<(String, String)>>,
        disconnects: Mutex<Vec<(Uuid, String, bool)>>,
    }

    #[async_trait::async_trait]
    impl ControlPlaneObserver for Arc<Recorder> {
        async fn on_session_opened(&self, _peer: Uuid, requester: Uuid, session: SessionId) {
            self.sessions_opened.lock().unwrap().push((requester, session));
        }
        async fn on_stream_opened(&self, _p: Uuid, _s: u8, _src: &str, _sink: &str) {}
        async fn on_stream_control(&self, _peer: Uuid, stream_id: u8, action: &StreamAction) {
            self.controls.lock().unwrap().push((stream_id, action.clone()));
        }
        async fn on_session_closed(&self, _peer: Uuid, session: SessionId) {
            self.sessions_closed.lock().unwrap().push(session);
        }
        async fn on_peer_disconnected(&self, peer: Uuid, reason: &str, had: bool) {
            self.disconnects.lock().unwrap().push((peer, reason.to_string(), had));
        }
        async fn on_devices_received(&self, peer: Uuid, devices: Vec<DeviceDescriptor>) {
            self.devices.lock().unwrap().push((peer, devices));
        }
        async fn on_peer_renamed(&self, peer_id: &str, peer_name: &str) {
            self.renames
                .lock().unwrap()
                .push((peer_id.to_string(), peer_name.to_string()));
        }
    }

    fn deps() -> Arc<ControlPlaneDeps> {
        Arc::new(ControlPlaneDeps {
            sessions: SessionManager::new(),
            stream_registry: StreamRegistry::new(),
            local_peer_id: Uuid::new_v4(),
            default_output: "Output:0:default".into(),
        })
    }

    fn driven(
        deps: Arc<ControlPlaneDeps>,
        peer: Uuid,
        rec: Arc<Recorder>,
    ) -> (broadcast::Sender<PeerEvent>, mpsc::Receiver<SignalingMessage>) {
        let (tx, rx) = broadcast::channel(16);
        let (ctx, crx) = mpsc::channel(16);
        spawn_control_plane(deps, peer, ctx, rx, Arc::new(rec));
        (tx, crx)
    }

    async fn await_sessions(
        deps: &ControlPlaneDeps,
        pred: impl Fn(&[crate::net::manager::SessionSnapshot]) -> bool,
    ) -> Vec<crate::net::manager::SessionSnapshot> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snap = deps.sessions.snapshot().await;
                if pred(&snap) {
                    return snap;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition not met within 2s")
    }

    async fn seed_active_stream(deps: &ControlPlaneDeps, remote: Uuid) -> SessionId {
        let sid = deps
            .sessions
            .open_outgoing(deps.local_peer_id, remote)
            .await;
        deps.sessions.accept(&sid).await.unwrap();
        let route = StreamRoute::new(
            Endpoint {
                peer_id: remote.to_string(),
                device_id: "src".into(),
            },
            Endpoint {
                peer_id: deps.local_peer_id.to_string(),
                device_id: "sink".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        );
        deps.sessions
            .add_stream(&sid, Stream::new_negotiating(StreamId(0), route, 5004))
            .await
            .unwrap();
        deps.sessions
            .activate_stream(&sid, StreamId(0))
            .await
            .unwrap();
        sid
    }

    #[tokio::test]
    async fn session_request_registers_active_session() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let requester = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let (tx, _crx) = driven(deps.clone(), requester, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: sid.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        let snap = await_sessions(&deps, |s| !s.is_empty()).await;
        assert_eq!(snap[0].id, SessionId(sid));
        assert_eq!(snap[0].state, SessionState::Active);
    }

    #[tokio::test]
    async fn session_request_evicts_stale_session_for_same_requester() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let requester = Uuid::new_v4();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let (tx, _crx) = driven(deps.clone(), requester, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: s1.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();
        await_sessions(&deps, |s| s.iter().any(|x| x.id == SessionId(s1))).await;

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: s2.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        let snap = await_sessions(&deps, |s| {
            s.iter()
                .any(|x| x.id == SessionId(s2) && x.state == SessionState::Active)
                && s.iter()
                    .find(|x| x.id == SessionId(s1))
                    .map(|x| x.state == SessionState::Closed)
                    .unwrap_or(false)
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
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let peer = Uuid::new_v4();
        seed_active_stream(&deps, peer).await;
        let (tx, _crx) = driven(deps.clone(), peer, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::SetMuted { muted: true },
        }))
        .unwrap();

        await_sessions(&deps, |s| {
            s.iter()
                .flat_map(|x| &x.streams)
                .any(|st| st.id == StreamId(0) && st.muted)
        })
        .await;
    }

    #[tokio::test]
    async fn stream_control_close_removes_stream() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let peer = Uuid::new_v4();
        let sid = seed_active_stream(&deps, peer).await;
        let (tx, _crx) = driven(deps.clone(), peer, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::Close,
        }))
        .unwrap();

        await_sessions(&deps, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.streams.is_empty())
                .unwrap_or(false)
        })
        .await;
    }

    #[tokio::test]
    async fn session_response_false_closes_session() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let peer = Uuid::new_v4();
        let sid = seed_active_stream(&deps, peer).await;
        let (tx, _crx) = driven(deps.clone(), peer, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::SessionResponse {
            session_id: sid.to_string(),
            accepted: false,
        }))
        .unwrap();

        await_sessions(&deps, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        assert_eq!(rec.sessions_closed.lock().unwrap().as_slice(), &[sid]);
    }

    #[tokio::test]
    async fn device_list_request_replies_on_channel() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let peer = Uuid::new_v4();
        let (tx, mut crx) = driven(deps.clone(), peer, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::DeviceListRequest {}))
            .unwrap();

        let reply = tokio::time::timeout(Duration::from_secs(2), crx.recv())
            .await
            .expect("no reply within 2s");
        assert!(matches!(
            reply,
            Some(SignalingMessage::DeviceListResponse { .. })
        ));
    }

    #[tokio::test]
    async fn device_list_response_forwarded_to_observer() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let peer = Uuid::new_v4();
        let devices = vec![DeviceDescriptor {
            id: "Output:0:default".into(),
            name: "Speakers".into(),
            kind: crate::audio::devices::DeviceKind::Output,
        }];
        let (tx, _crx) = driven(deps.clone(), peer, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::DeviceListResponse {
            devices: devices.clone(),
        }))
        .unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !rec.devices.lock().unwrap().is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("observer not called within 2s");
        assert_eq!(rec.devices.lock().unwrap()[0], (peer, devices));
    }

    #[tokio::test]
    async fn peer_renamed_forwarded_to_observer() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let rid = Uuid::new_v4();
        let (tx, _crx) = driven(deps.clone(), rid, rec.clone());

        tx.send(PeerEvent::Message(SignalingMessage::PeerRenamed {
            peer_id: rid.to_string(),
            peer_name: "New".into(),
        }))
        .unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !rec.renames.lock().unwrap().is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("observer not called within 2s");
        assert_eq!(rec.renames.lock().unwrap()[0].1, "New");
    }

    #[tokio::test]
    async fn disconnect_tears_down_sessions_and_reports_active() {
        let deps = deps();
        let rec = Arc::new(Recorder::default());
        let peer = Uuid::new_v4();
        let sid = seed_active_stream(&deps, peer).await;
        let (tx, _crx) = driven(deps.clone(), peer, rec.clone());

        tx.send(PeerEvent::Disconnected {
            reason: "test".into(),
        })
        .unwrap();

        await_sessions(&deps, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !rec.disconnects.lock().unwrap().is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("disconnect observer not called within 2s");
        assert_eq!(rec.disconnects.lock().unwrap()[0], (peer, "test".to_string(), true));
    }
}
