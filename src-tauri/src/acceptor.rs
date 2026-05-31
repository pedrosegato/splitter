use crate::core::AppCore;
use crate::events::{IncomingSession, PeerDisconnected, SnapshotChanged};
use splitter_core::net::session::SessionState;
use splitter_core::net::signaling::{
    CodecParams, DeviceDescriptor, Endpoint, PeerEvent, SignalingMessage, StreamAction,
};
use splitter_core::net::stream::StreamRoute;
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
    let local_peer_id = core.identity.read().unwrap().peer_id;
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(msg)) => match msg {
                    SignalingMessage::SessionRequest {
                        session_id,
                        requested_by,
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id) else {
                            continue;
                        };
                        let Ok(requester_uuid) = Uuid::parse_str(&requested_by) else {
                            continue;
                        };

                        let existing = core.sessions.snapshot().await.into_iter().find(|s| {
                            s.remote_peer_id == requester_uuid && s.state == SessionState::Active
                        });
                        if let Some(ex) = existing {
                            tracing::info!(peer = %peer_id, session = %ex.id, "re-opened existing session");
                            continue;
                        }

                        let _ = core
                            .sessions
                            .register_incoming(sid_uuid, local_peer_id, requester_uuid)
                            .await;
                        let _ = core.sessions.accept(&sid_uuid).await;
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
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id) else {
                            continue;
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
                            default_output.clone()
                        } else {
                            sink.device_id.clone()
                        };
                        let mut session_route = route.clone();
                        session_route.sink.device_id = chosen_output.clone();
                        match open_stream_as_sink(
                            core.stream_registry.clone(),
                            sid_uuid,
                            stream_id,
                            route,
                            chosen_output.clone(),
                        )
                        .await
                        {
                            Ok(port) => {
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
                                let stream = splitter_core::net::stream::Stream::new_negotiating(
                                    stream_id,
                                    session_route,
                                    port,
                                );
                                let _ = core.sessions.add_stream(&sid_uuid, stream).await;
                                let _ = core.sessions.activate_stream(&sid_uuid, stream_id).await;
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
                    SignalingMessage::StreamControl {
                        stream_id,
                        action,
                        volume,
                    } => {
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
                            StreamAction::SetVolume => tracing::info!(
                                peer = %peer_id,
                                stream_id,
                                volume = volume.unwrap_or(1.0),
                                "remote set stream volume"
                            ),
                        }
                        let session_ids: Vec<Uuid> = core
                            .sessions
                            .snapshot()
                            .await
                            .into_iter()
                            .filter(|s| s.remote_peer_id == peer_id)
                            .map(|s| s.id)
                            .collect();
                        match action {
                            StreamAction::Close => {
                                for sid in session_ids {
                                    let _ = core.stream_registry.close(&sid, stream_id).await;
                                    let _ = core.sessions.remove_stream(&sid, stream_id).await;
                                }
                            }
                            StreamAction::Pause => {
                                for sid in &session_ids {
                                    let _ = core
                                        .stream_registry
                                        .send_control(sid, stream_id, StreamControlSignal::Pause)
                                        .await;
                                }
                            }
                            StreamAction::Resume => {
                                for sid in &session_ids {
                                    let _ = core
                                        .stream_registry
                                        .send_control(sid, stream_id, StreamControlSignal::Resume)
                                        .await;
                                }
                            }
                            StreamAction::SetVolume => {
                                let gain = volume.unwrap_or(1.0).clamp(0.0, 2.0);
                                for sid in &session_ids {
                                    let _ = core
                                        .stream_registry
                                        .send_control(
                                            sid,
                                            stream_id,
                                            StreamControlSignal::SetVolume(gain),
                                        )
                                        .await;
                                }
                            }
                        }
                        core.emit(SnapshotChanged);
                    }
                    SignalingMessage::SessionResponse {
                        session_id,
                        accepted: false,
                    } => {
                        let Ok(sid_uuid) = Uuid::parse_str(&session_id) else {
                            continue;
                        };
                        let stream_ids: Vec<u8> = core
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
                                kind: format!("{:?}", d.kind),
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
                    _ => {}
                },
                Ok(PeerEvent::Disconnected { reason }) => {
                    tracing::info!(peer = %peer_id, %reason, "peer disconnected");
                    core.emit(PeerDisconnected {
                        peer_id: peer_id.to_string(),
                        reason: reason.clone(),
                    });
                    let session_ids: Vec<Uuid> = core
                        .sessions
                        .snapshot()
                        .await
                        .into_iter()
                        .filter(|s| s.remote_peer_id == peer_id)
                        .map(|s| s.id)
                        .collect();
                    let had_active_session = !session_ids.is_empty();
                    for sid in &session_ids {
                        let stream_ids: Vec<u8> = core
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
                    break;
                }
                Ok(PeerEvent::Connected { .. }) => {}
                Err(_) => break,
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
