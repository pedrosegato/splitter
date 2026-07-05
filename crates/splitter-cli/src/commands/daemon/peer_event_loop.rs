use super::context::{pick_default_output_device_id, short, DaemonContext};
use super::reconnect::spawn_reconnect_loop;
use splitter_core::net::signaling::{
    CodecParams, Endpoint, PeerEvent, SignalingMessage, StreamAction,
};
use splitter_core::net::stream_runtime::{open_stream_as_sink, StreamControlSignal};
use splitter_core::{SessionId, StreamId, StreamRoute};
use uuid::Uuid;

type ConnTx = tokio::sync::mpsc::Sender<SignalingMessage>;

pub(crate) fn spawn_stream_open_acceptor(
    ctx: DaemonContext,
    conn_tx: ConnTx,
    mut events: tokio::sync::broadcast::Receiver<PeerEvent>,
    peer_id: Uuid,
) {
    let default_output =
        pick_default_output_device_id().unwrap_or_else(|| "Output:0:default".into());
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(msg)) => match msg {
                    SignalingMessage::SessionRequest {
                        session_id,
                        requested_by,
                    } => {
                        handle_session_request(&ctx, peer_id, &session_id, &requested_by).await;
                    }
                    msg @ SignalingMessage::StreamOpen { .. } => {
                        handle_stream_open(&ctx, peer_id, &conn_tx, &default_output, msg).await;
                    }
                    SignalingMessage::StreamControl { stream_id, action } => {
                        handle_stream_control(&ctx, peer_id, stream_id, action).await;
                    }
                    SignalingMessage::SessionResponse {
                        session_id,
                        accepted: false,
                    } => {
                        handle_session_response_close(&ctx, peer_id, &session_id).await;
                    }
                    _ => {}
                },
                Ok(PeerEvent::Disconnected { reason }) => {
                    handle_peer_disconnected(&ctx, peer_id, &reason).await;
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

async fn handle_session_request(
    ctx: &DaemonContext,
    peer_id: Uuid,
    session_id: &str,
    requested_by: &str,
) {
    let Ok(sid_uuid) = Uuid::parse_str(session_id).map(SessionId) else {
        return;
    };
    let Ok(requester_uuid) = Uuid::parse_str(requested_by) else {
        return;
    };

    let existing = ctx.sessions.snapshot().await.into_iter().find(|s| {
        s.remote_peer_id == requester_uuid
            && s.state == splitter_core::net::session::SessionState::Active
    });
    if let Some(ref ex) = existing {
        let name = ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(
                ">> {name} re-opened existing session {}",
                short(&ex.id.get())
            );
        }
        return;
    }

    let _ = ctx
        .sessions
        .register_incoming(sid_uuid, ctx.local_peer_id, requester_uuid)
        .await;
    let _ = ctx.sessions.accept(&sid_uuid).await;
    let name = ctx.peer_display_name(&peer_id).await;
    #[allow(clippy::print_stdout)]
    {
        println!(">> {name} opened session {}", short(&sid_uuid.get()));
    }
}

async fn handle_stream_open(
    ctx: &DaemonContext,
    peer_id: Uuid,
    conn_tx: &ConnTx,
    default_output: &str,
    msg: SignalingMessage,
) {
    let SignalingMessage::StreamOpen {
        session_id,
        stream_id,
        source,
        sink,
        codec,
        ..
    } = msg
    else {
        return;
    };
    let Ok(sid_uuid) = Uuid::parse_str(&session_id).map(SessionId) else {
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
        default_output.to_string()
    } else {
        sink.device_id.clone()
    };
    match open_stream_as_sink(
        ctx.stream_registry.clone(),
        sid_uuid,
        StreamId(stream_id),
        route,
        chosen_output.clone(),
    )
    .await
    {
        Ok(port) => {
            let _ = conn_tx
                .send(SignalingMessage::StreamOpenAck {
                    stream_id,
                    accepted: true,
                    udp_port: Some(port),
                })
                .await;
            let name = ctx.peer_display_name(&peer_id).await;
            #[allow(clippy::print_stdout)]
            {
                println!(
                    ">> {name} opened stream {stream_id} from {} \u{2192} local {chosen_output}",
                    source.device_id
                );
            }
        }
        Err(e) => {
            tracing::warn!("stream_open accept failed: {e}");
            let _ = conn_tx
                .send(SignalingMessage::StreamOpenAck {
                    stream_id,
                    accepted: false,
                    udp_port: None,
                })
                .await;
        }
    }
}

async fn handle_stream_control(
    ctx: &DaemonContext,
    peer_id: Uuid,
    stream_id: u8,
    action: StreamAction,
) {
    let name = ctx.peer_display_name(&peer_id).await;
    #[allow(clippy::print_stdout)]
    {
        match &action {
            StreamAction::Close => {
                println!(">> {name} closed stream {stream_id}");
            }
            StreamAction::Pause => {
                println!(">> {name} paused stream {stream_id}");
            }
            StreamAction::Resume => {
                println!(">> {name} resumed stream {stream_id}");
            }
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
    let session_ids: Vec<SessionId> = ctx
        .sessions
        .snapshot()
        .await
        .into_iter()
        .filter(|s| s.remote_peer_id == peer_id)
        .map(|s| s.id)
        .collect();
    let registry = &ctx.stream_registry;
    if matches!(action, StreamAction::Close) {
        for sid in session_ids {
            let _ = registry.close(&sid, StreamId(stream_id)).await;
        }
    } else {
        let signal = StreamControlSignal::from(action);
        for sid in &session_ids {
            let _ = registry
                .send_control(sid, StreamId(stream_id), signal)
                .await;
        }
    }
}

async fn handle_session_response_close(ctx: &DaemonContext, peer_id: Uuid, session_id: &str) {
    let Ok(sid_uuid) = Uuid::parse_str(session_id).map(SessionId) else {
        return;
    };
    // Remote is shutting down this session; close local streams + session.
    let stream_ids: Vec<StreamId> = ctx
        .sessions
        .snapshot()
        .await
        .into_iter()
        .find(|s| s.id == sid_uuid)
        .map(|s| s.streams.iter().map(|st| st.id).collect())
        .unwrap_or_default();
    for sid_stream in stream_ids {
        let _ = ctx.stream_registry.close(&sid_uuid, sid_stream).await;
    }
    let _ = ctx.sessions.close(&sid_uuid).await;
    let name = ctx.peer_display_name(&peer_id).await;
    #[allow(clippy::print_stdout)]
    {
        println!(">> {name} closed session {}", short(&sid_uuid.get()));
    }
}

async fn handle_peer_disconnected(ctx: &DaemonContext, peer_id: Uuid, reason: &str) {
    let name = ctx.peer_display_name(&peer_id).await;
    #[allow(clippy::print_stdout)]
    {
        println!(">> {name} disconnected (reason: {reason})");
    }
    // Tear down all streams and sessions for this peer so the registry and
    // SessionManager don't accumulate stale entries after an abrupt disconnect.
    let session_streams: Vec<(SessionId, Vec<StreamId>)> = ctx
        .sessions
        .snapshot()
        .await
        .into_iter()
        .filter(|s| s.remote_peer_id == peer_id)
        .map(|s| (s.id, s.streams.iter().map(|st| st.id).collect()))
        .collect();
    let had_active_session = !session_streams.is_empty();
    for (sid, stream_ids) in &session_streams {
        for stream_id in stream_ids {
            let _ = ctx.stream_registry.close(sid, *stream_id).await;
        }
        let _ = ctx.sessions.close(sid).await;
    }
    if had_active_session {
        let still_present = ctx
            .discovered
            .read()
            .await
            .values()
            .any(|p| p.peer_id == peer_id.to_string());
        if still_present {
            spawn_reconnect_loop(ctx.clone(), peer_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::context::test_ctx;
    use super::*;
    use splitter_core::net::session::SessionState;
    use splitter_core::net::signaling::Codec;
    use splitter_core::net::stream::Stream;
    use std::time::Duration;

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
        let peer = Uuid::new_v4();
        let requester = Uuid::new_v4();
        let sid = Uuid::new_v4();

        handle_session_request(&ctx, peer, &sid.to_string(), &requester.to_string()).await;

        let snap = ctx.sessions.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].remote_peer_id, requester);
        assert_eq!(snap[0].state, SessionState::Active);
    }

    #[tokio::test]
    async fn session_request_with_existing_active_session_is_noop() {
        let ctx = test_ctx();
        let peer = Uuid::new_v4();
        let requester = Uuid::new_v4();
        let existing = seed_active_session(&ctx, requester).await;
        let new_sid = Uuid::new_v4();

        handle_session_request(&ctx, peer, &new_sid.to_string(), &requester.to_string()).await;

        let snap = ctx.sessions.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, existing);
    }

    #[tokio::test]
    async fn session_request_bad_uuid_changes_nothing() {
        let ctx = test_ctx();
        let peer = Uuid::new_v4();
        let requester = Uuid::new_v4();

        handle_session_request(&ctx, peer, "not-a-uuid", &requester.to_string()).await;

        assert!(ctx.sessions.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn stream_control_set_muted_does_not_touch_session_state() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        seed_active_session_with_stream(&ctx, remote).await;

        handle_stream_control(&ctx, remote, 0, StreamAction::SetMuted { muted: true }).await;

        let snap = ctx.sessions.snapshot().await;
        assert!(!snap[0].streams[0].muted);
    }

    #[tokio::test]
    async fn stream_control_close_closes_registry_entry() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let sid = seed_active_session_with_stream(&ctx, remote).await;

        handle_stream_control(&ctx, remote, 0, StreamAction::Close).await;

        let snap = ctx.sessions.snapshot().await;
        assert!(snap.iter().any(|s| s.id == sid));
    }

    #[tokio::test]
    async fn session_response_close_closes_session() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let sid = seed_active_session(&ctx, remote).await;

        handle_session_response_close(&ctx, remote, &sid.get().to_string()).await;

        let snap = ctx.sessions.snapshot().await;
        let session = snap.iter().find(|s| s.id == sid).unwrap();
        assert_eq!(session.state, SessionState::Closed);
    }

    #[tokio::test]
    async fn peer_disconnected_tears_down_all_sessions_for_peer() {
        let ctx = test_ctx();
        let remote = Uuid::new_v4();
        let other = Uuid::new_v4();
        let a = seed_active_session(&ctx, remote).await;
        let b = seed_active_session(&ctx, remote).await;
        let untouched = seed_active_session(&ctx, other).await;

        tokio::time::timeout(
            Duration::from_secs(2),
            handle_peer_disconnected(&ctx, remote, "test"),
        )
        .await
        .expect("teardown must not hang");

        let snap = ctx.sessions.snapshot().await;
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

        tokio::time::timeout(
            Duration::from_secs(2),
            handle_peer_disconnected(&ctx, remote, "test"),
        )
        .await
        .expect("call must return promptly with empty discovered map");

        let snap = ctx.sessions.snapshot().await;
        let session = snap.iter().find(|s| s.id == sid).unwrap();
        assert_eq!(session.state, SessionState::Closed);
    }
}
