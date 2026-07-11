use super::context::{pick_default_output_device_id, short, DaemonContext};
use splitter_core::net::signaling::client_ops::ConnEndpoints;
use splitter_core::net::signaling::{
    spawn_control_plane, spawn_reconnect, ConnectOutcome, ControlPlaneDeps, ControlPlaneHost,
    ControlPlaneObserver, PeerEvent, ReconnectDriver, SignalingMessage, StreamAction,
};
use splitter_core::{PeerIdentity, SessionId, TrustStore};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

type ConnTx = tokio::sync::mpsc::Sender<SignalingMessage>;

#[derive(Clone)]
pub(crate) struct CliControlPlane {
    pub ctx: DaemonContext,
}

fn make_deps(ctx: &DaemonContext) -> Arc<ControlPlaneDeps> {
    Arc::new(ControlPlaneDeps {
        sessions: ctx.sessions.clone(),
        stream_registry: ctx.stream_registry.clone(),
        local_peer_id: ctx.local_peer_id,
        default_output: pick_default_output_device_id()
            .unwrap_or_else(|| "Output:0:default".into()),
    })
}

pub(crate) fn spawn_control_plane_loop(
    ctx: DaemonContext,
    conn_tx: ConnTx,
    events: tokio::sync::broadcast::Receiver<PeerEvent>,
    peer_id: Uuid,
    connection_id: Option<Uuid>,
) {
    let deps = make_deps(&ctx);
    spawn_control_plane(
        deps,
        peer_id,
        conn_tx,
        events,
        Arc::new(CliControlPlane { ctx }),
        connection_id,
    );
}

#[async_trait::async_trait]
impl ControlPlaneObserver for CliControlPlane {
    async fn on_session_opened(&self, peer_id: Uuid, _requester: Uuid, session: SessionId) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> {name} opened session {}", short(&session.get()));
        }
    }

    async fn on_stream_opened(
        &self,
        peer_id: Uuid,
        stream_id: u8,
        source_device: &str,
        sink_device: &str,
    ) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(
                ">> {name} opened stream {stream_id} from {source_device} \u{2192} local {sink_device}"
            );
        }
    }

    async fn on_stream_control(&self, peer_id: Uuid, stream_id: u8, action: &StreamAction) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            match action {
                StreamAction::Close => println!(">> {name} closed stream {stream_id}"),
                StreamAction::Pause => println!(">> {name} paused stream {stream_id}"),
                StreamAction::Resume => println!(">> {name} resumed stream {stream_id}"),
                StreamAction::SetVolume { volume } => {
                    let pct = (volume * 100.0).round() as u32;
                    println!(">> {name} set stream {stream_id} volume to {pct}%");
                }
                StreamAction::SetMuted { muted } => {
                    let state = if *muted { "muted" } else { "unmuted" };
                    println!(">> {name} {state} stream {stream_id}");
                }
            }
        }
    }

    async fn on_session_closed(&self, peer_id: Uuid, session: SessionId) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> {name} closed session {}", short(&session.get()));
        }
    }

    async fn on_peer_disconnected(&self, peer_id: Uuid, reason: &str, had_active_session: bool) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> {name} disconnected (reason: {reason})");
        }
        if had_active_session {
            let still_present = self
                .ctx
                .discovered
                .read()
                .await
                .values()
                .any(|p| p.peer_id == peer_id.to_string());
            if still_present {
                spawn_reconnect(peer_id, Arc::new(self.clone()));
            }
        }
    }
}

#[async_trait::async_trait]
impl ReconnectDriver for CliControlPlane {
    fn identity(&self) -> PeerIdentity {
        self.ctx.identity.clone()
    }

    fn trust(&self) -> Arc<RwLock<TrustStore>> {
        self.ctx.trust.clone()
    }

    async fn resolve_addr(&self, peer_id: Uuid) -> Option<SocketAddr> {
        let map = self.ctx.discovered.read().await;
        map.values()
            .find(|p| p.peer_id == peer_id.to_string())
            .and_then(|p| format!("{}:{}", p.host, p.port).parse::<SocketAddr>().ok())
    }

    async fn still_discoverable(&self, peer_id: Uuid) -> bool {
        self.ctx
            .discovered
            .read()
            .await
            .values()
            .any(|p| p.peer_id == peer_id.to_string())
    }

    async fn on_reconnected(&self, _peer_id: Uuid, outcome: ConnectOutcome) {
        if let Some(pid) = outcome.remote_peer_id {
            self.ctx
                .register_outgoing_connection(pid, outcome.handle)
                .await;
        }
    }

    async fn on_reconnected_display(&self, peer_id: Uuid) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> reconnected to {name}");
        }
    }

    async fn on_reconnect_failed_display(&self, peer_id: Uuid) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> reconnect to {name} failed");
        }
    }
}

#[async_trait::async_trait]
impl ControlPlaneHost for CliControlPlane {
    fn spawn_loop(&self, peer_id: Uuid, endpoints: ConnEndpoints) {
        let connection_id = endpoints.connection_id;
        spawn_control_plane_loop(
            self.ctx.clone(),
            endpoints.tx,
            endpoints.events.subscribe(),
            peer_id,
            Some(connection_id),
        );
    }

    async fn on_peer_connected(&self, peer_id: Uuid) {
        let name = self.ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> {name} connected (peer_id {})", short(&peer_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::context::test_ctx;
    use super::*;
    use splitter_core::net::session::SessionState;
    use splitter_core::net::signaling::{Codec, CodecParams, Endpoint};
    use splitter_core::net::stream::Stream;
    use splitter_core::{StreamId, StreamRoute};
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc};

    fn driven(
        ctx: DaemonContext,
        peer: Uuid,
    ) -> (
        broadcast::Sender<PeerEvent>,
        mpsc::Receiver<SignalingMessage>,
    ) {
        let (tx, rx) = broadcast::channel(16);
        let (conn_tx, conn_rx) = mpsc::channel(16);
        spawn_control_plane_loop(ctx, conn_tx, rx, peer, None);
        (tx, conn_rx)
    }

    async fn await_sessions(
        ctx: &DaemonContext,
        pred: impl Fn(&[splitter_core::SessionSnapshot]) -> bool,
    ) -> Vec<splitter_core::SessionSnapshot> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snap = ctx.sessions.snapshot().await;
                if pred(&snap) {
                    return snap;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("sessions condition not met within 2s")
    }

    async fn seed_active_session(ctx: &DaemonContext, remote: Uuid) -> SessionId {
        let sid = ctx.sessions.open_outgoing(ctx.local_peer_id, remote).await;
        ctx.sessions.accept(&sid).await.unwrap();
        sid
    }

    async fn seed_active_session_with_stream(ctx: &DaemonContext, remote: Uuid) -> SessionId {
        let sid = seed_active_session(ctx, remote).await;
        let route = StreamRoute::new(
            Endpoint {
                peer_id: remote.to_string(),
                device_id: "src".into(),
            },
            Endpoint {
                peer_id: ctx.local_peer_id.to_string(),
                device_id: "sink".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        );
        ctx.sessions
            .add_stream(&sid, Stream::new_negotiating(StreamId(0), route, 5004))
            .await
            .unwrap();
        ctx.sessions
            .activate_stream(&sid, StreamId(0))
            .await
            .unwrap();
        sid
    }

    #[tokio::test]
    async fn session_request_registers_new_active_session() {
        let ctx = test_ctx();
        let requester = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let (tx, _rx) = driven(ctx.clone(), requester);

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: sid.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        let snap = await_sessions(&ctx, |s| !s.is_empty()).await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].remote_peer_id, requester);
        assert_eq!(snap[0].state, SessionState::Active);
    }

    // Unification convergence (was `..._is_noop`): the CLI now evicts a stale
    // active session from the same requester instead of dedup-returning early,
    // matching the Tauri acceptor's safer policy.
    #[tokio::test]
    async fn session_request_with_existing_active_session_evicts_it() {
        let ctx = test_ctx();
        let requester = Uuid::new_v4();
        let existing = seed_active_session(&ctx, requester).await;
        let new_sid = Uuid::new_v4();
        let (tx, _rx) = driven(ctx.clone(), requester);

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: new_sid.to_string(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        let snap = await_sessions(&ctx, |s| {
            s.iter()
                .any(|x| x.id == SessionId(new_sid) && x.state == SessionState::Active)
                && s.iter()
                    .find(|x| x.id == existing)
                    .map(|x| x.state == SessionState::Closed)
                    .unwrap_or(false)
        })
        .await;
        let active: Vec<_> = snap
            .iter()
            .filter(|x| x.state == SessionState::Active)
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, SessionId(new_sid));
    }

    #[tokio::test]
    async fn session_request_bad_uuid_changes_nothing() {
        let ctx = test_ctx();
        let requester = Uuid::new_v4();
        let (tx, _rx) = driven(ctx.clone(), requester);

        tx.send(PeerEvent::Message(SignalingMessage::SessionRequest {
            session_id: "not-a-uuid".into(),
            requested_by: requester.to_string(),
        }))
        .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(ctx.sessions.snapshot().await.is_empty());
    }

    // Unification convergence (was `..._does_not_touch_session_state`): the CLI
    // now mirrors a remote SetMuted into session state like the Tauri acceptor.
    #[tokio::test]
    async fn stream_control_set_muted_marks_session_stream_muted() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        seed_active_session_with_stream(&ctx, remote).await;
        let (tx, _rx) = driven(ctx.clone(), remote);

        tx.send(PeerEvent::Message(SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::SetMuted { muted: true },
        }))
        .unwrap();

        await_sessions(&ctx, |s| {
            s.iter()
                .flat_map(|x| &x.streams)
                .any(|st| st.id == StreamId(0) && st.muted)
        })
        .await;
    }

    #[tokio::test]
    async fn stream_control_close_removes_stream_from_session() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let sid = seed_active_session_with_stream(&ctx, remote).await;
        let (tx, _rx) = driven(ctx.clone(), remote);

        tx.send(PeerEvent::Message(SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::Close,
        }))
        .unwrap();

        let snap = await_sessions(&ctx, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.streams.is_empty())
                .unwrap_or(false)
        })
        .await;
        assert!(snap.iter().any(|s| s.id == sid));
    }

    #[tokio::test]
    async fn session_response_close_closes_session() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let sid = seed_active_session(&ctx, remote).await;
        let (tx, _rx) = driven(ctx.clone(), remote);

        tx.send(PeerEvent::Message(SignalingMessage::SessionResponse {
            session_id: sid.get().to_string(),
            accepted: false,
        }))
        .unwrap();

        let snap = await_sessions(&ctx, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            snap.iter().find(|s| s.id == sid).unwrap().state,
            SessionState::Closed
        );
    }

    #[tokio::test]
    async fn peer_disconnected_tears_down_all_sessions_for_peer() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let other = Uuid::new_v4();
        let a = seed_active_session(&ctx, remote).await;
        let b = seed_active_session(&ctx, remote).await;
        let untouched = seed_active_session(&ctx, other).await;
        let (tx, _rx) = driven(ctx.clone(), remote);

        tx.send(PeerEvent::Disconnected {
            reason: "test".into(),
        })
        .unwrap();

        let snap = await_sessions(&ctx, |s| {
            s.iter()
                .find(|x| x.id == a)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        let state_of = |id: SessionId| snap.iter().find(|s| s.id == id).unwrap().state;
        assert_eq!(state_of(a), SessionState::Closed);
        assert_eq!(state_of(b), SessionState::Closed);
        assert_eq!(state_of(untouched), SessionState::Active);
    }

    #[tokio::test]
    async fn peer_disconnected_without_discovery_entry_does_not_reconnect() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let sid = seed_active_session(&ctx, remote).await;
        let (tx, _rx) = driven(ctx.clone(), remote);

        tx.send(PeerEvent::Disconnected {
            reason: "test".into(),
        })
        .unwrap();

        let snap = await_sessions(&ctx, |s| {
            s.iter()
                .find(|x| x.id == sid)
                .map(|x| x.state == SessionState::Closed)
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            snap.iter().find(|s| s.id == sid).unwrap().state,
            SessionState::Closed
        );
    }
}
