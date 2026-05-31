use super::context::{pick_default_output_device_id, short, DaemonContext};
use super::reconnect::spawn_reconnect_loop;
use splitter_core::net::signaling::{
    CodecParams, Endpoint, PeerEvent, SignalingMessage, StreamAction,
};
use splitter_core::net::stream_runtime::{open_stream_as_sink, StreamControlSignal};
use splitter_core::StreamRoute;
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
                    SignalingMessage::StreamControl {
                        stream_id,
                        action,
                        volume,
                    } => {
                        handle_stream_control(&ctx, peer_id, stream_id, action, volume).await;
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
    let Ok(sid_uuid) = Uuid::parse_str(session_id) else {
        return;
    };
    let Ok(requester_uuid) = Uuid::parse_str(requested_by) else {
        return;
    };

    // A — dedupe incoming session requests: if there is already an Active session for
    // this (local, remote) pair, keep it and skip creating a duplicate.
    let existing = ctx.sessions.snapshot().await.into_iter().find(|s| {
        s.remote_peer_id == requester_uuid
            && s.state == splitter_core::net::session::SessionState::Active
    });
    if let Some(ref ex) = existing {
        let name = ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> {name} re-opened existing session {}", short(&ex.id));
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
        println!(">> {name} opened session {}", short(&sid_uuid));
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
    let Ok(sid_uuid) = Uuid::parse_str(&session_id) else {
        return;
    };
    let route = StreamRoute {
        source: Endpoint {
            peer_id: source.peer_id.clone(),
            device_id: source.device_id.clone(),
        },
        sink: Endpoint {
            peer_id: sink.peer_id.clone(),
            device_id: sink.device_id.clone(),
        },
        codec: CodecParams {
            name: codec.name.clone(),
            bitrate: codec.bitrate,
            frame_ms: codec.frame_ms,
        },
        volume: 1.0,
    };
    let chosen_output = if sink.device_id == "default" {
        default_output.to_string()
    } else {
        sink.device_id.clone()
    };
    match open_stream_as_sink(
        ctx.stream_registry.clone(),
        sid_uuid,
        stream_id,
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
    volume: Option<f32>,
) {
    let name = ctx.peer_display_name(&peer_id).await;
    #[allow(clippy::print_stdout)]
    {
        match action {
            StreamAction::Close => {
                println!(">> {name} closed stream {stream_id}");
            }
            StreamAction::Pause => {
                println!(">> {name} paused stream {stream_id}");
            }
            StreamAction::Resume => {
                println!(">> {name} resumed stream {stream_id}");
            }
            StreamAction::SetVolume => {
                let pct = volume.map(|v| (v * 100.0).round() as u32).unwrap_or(100);
                println!(">> {name} set stream {stream_id} volume to {pct}%");
            }
        }
    }
    // Propagate the control signal to every matching local StreamRuntime
    // (covers both source and sink runtimes on this daemon).
    let session_ids: Vec<Uuid> = ctx
        .sessions
        .snapshot()
        .await
        .into_iter()
        .filter(|s| s.remote_peer_id == peer_id)
        .map(|s| s.id)
        .collect();
    let registry = &ctx.stream_registry;
    match action {
        StreamAction::Close => {
            for sid in session_ids {
                let _ = registry.close(&sid, stream_id).await;
            }
        }
        StreamAction::Pause => {
            for sid in &session_ids {
                let _ = registry
                    .send_control(sid, stream_id, StreamControlSignal::Pause)
                    .await;
            }
        }
        StreamAction::Resume => {
            for sid in &session_ids {
                let _ = registry
                    .send_control(sid, stream_id, StreamControlSignal::Resume)
                    .await;
            }
        }
        StreamAction::SetVolume => {
            let gain = volume.unwrap_or(1.0).clamp(0.0, 2.0);
            for sid in &session_ids {
                let _ = registry
                    .send_control(sid, stream_id, StreamControlSignal::SetVolume(gain))
                    .await;
            }
        }
    }
}

async fn handle_session_response_close(ctx: &DaemonContext, peer_id: Uuid, session_id: &str) {
    let Ok(sid_uuid) = Uuid::parse_str(session_id) else {
        return;
    };
    // Remote is shutting down this session; close local streams + session.
    let stream_ids: Vec<u8> = ctx
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
        println!(">> {name} closed session {}", short(&sid_uuid));
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
    let session_streams: Vec<(Uuid, Vec<u8>)> = ctx
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
    // B — auto-reconnect if peer was in an active session and is still announced via mDNS.
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
