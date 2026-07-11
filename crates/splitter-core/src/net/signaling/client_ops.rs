use crate::error::NetError;
use crate::net::manager::SessionManager;
use crate::net::signaling::{
    Codec, CodecParams, Endpoint, PeerConnectionHandle, PeerEvent, SignalingMessage, StreamAction,
};
use crate::net::stream::StreamRoute;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

pub type ConnectionMap = Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>;

pub struct ConnEndpoints {
    pub tx: mpsc::Sender<SignalingMessage>,
    pub remote_addr: SocketAddr,
    pub events: broadcast::Sender<PeerEvent>,
    pub connection_id: Uuid,
}

pub async fn find_conn(
    server_conns: &ConnectionMap,
    outgoing_conns: &ConnectionMap,
    peer_id: Uuid,
) -> Option<ConnEndpoints> {
    {
        let g = server_conns.read().await;
        if let Some(c) = g.get(&peer_id) {
            return Some(ConnEndpoints {
                tx: c.tx.clone(),
                remote_addr: c.remote_addr,
                events: c.events.clone(),
                connection_id: c.connection_id,
            });
        }
    }
    {
        let g = outgoing_conns.read().await;
        if let Some(c) = g.get(&peer_id) {
            return Some(ConnEndpoints {
                tx: c.tx.clone(),
                remote_addr: c.remote_addr,
                events: c.events.clone(),
                connection_id: c.connection_id,
            });
        }
    }
    None
}

pub async fn find_conn_tx(
    server_conns: &ConnectionMap,
    outgoing_conns: &ConnectionMap,
    peer_id: Uuid,
) -> Option<mpsc::Sender<SignalingMessage>> {
    find_conn(server_conns, outgoing_conns, peer_id)
        .await
        .map(|c| c.tx)
}

fn opus_codec(bitrate: u32) -> CodecParams {
    CodecParams {
        name: Codec::Opus,
        bitrate,
        frame_ms: 20,
    }
}

pub fn build_stream_route(
    local: Uuid,
    sink_peer: Uuid,
    source_dev: &str,
    sink_dev: &str,
    bitrate: u32,
) -> StreamRoute {
    StreamRoute::new(
        Endpoint {
            peer_id: local.to_string(),
            device_id: source_dev.to_string(),
        },
        Endpoint {
            peer_id: sink_peer.to_string(),
            device_id: sink_dev.to_string(),
        },
        opus_codec(bitrate),
        1.0,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn stream_open_message(
    session_id: Uuid,
    stream_id: u8,
    local: Uuid,
    sink_peer: Uuid,
    source_dev: &str,
    sink_dev: &str,
    bitrate: u32,
) -> SignalingMessage {
    SignalingMessage::StreamOpen {
        session_id: session_id.to_string(),
        stream_id,
        source: Endpoint {
            peer_id: local.to_string(),
            device_id: source_dev.to_string(),
        },
        sink: Endpoint {
            peer_id: sink_peer.to_string(),
            device_id: sink_dev.to_string(),
        },
        codec: opus_codec(bitrate),
        udp_port: 0,
    }
}

pub async fn wait_for_stream_open_ack(
    rx: &mut broadcast::Receiver<PeerEvent>,
    stream_id: u8,
    timeout: Duration,
) -> Result<u16, NetError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(NetError::Timeout {
                what: "stream_open_ack".into(),
                millis: timeout.as_millis() as u64,
            });
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(PeerEvent::Message(SignalingMessage::StreamOpenAck {
                stream_id: id,
                accepted,
                udp_port,
            }))) if id == stream_id => {
                return if accepted {
                    udp_port.ok_or_else(|| NetError::SignalingProtocol {
                        reason: "stream_open_ack accepted without a udp_port".into(),
                    })
                } else {
                    Err(NetError::SignalingProtocol {
                        reason: "peer rejected stream".into(),
                    })
                };
            }
            Ok(Ok(_)) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err(NetError::ChannelClosed);
            }
            Err(_) => {
                return Err(NetError::Timeout {
                    what: "stream_open_ack".into(),
                    millis: timeout.as_millis() as u64,
                });
            }
        }
    }
}

pub async fn notify_remote_control(
    tx: &mpsc::Sender<SignalingMessage>,
    stream_id: u8,
    action: StreamAction,
) {
    tx.send(SignalingMessage::StreamControl { stream_id, action })
        .await
        .ok();
}

pub async fn notify_remote_by_session(
    sessions: &SessionManager,
    server_conns: &ConnectionMap,
    outgoing_conns: &ConnectionMap,
    session_id: Uuid,
    stream_id: u8,
    action: StreamAction,
) {
    let snap = sessions.snapshot().await;
    let remote = match snap
        .iter()
        .find(|s| s.id.get() == session_id)
        .map(|s| s.remote_peer_id)
    {
        Some(r) => r,
        None => {
            tracing::warn!(%session_id, "notify_remote_by_session: session not found, skipping remote signal");
            return;
        }
    };
    match find_conn_tx(server_conns, outgoing_conns, remote).await {
        Some(tx) => notify_remote_control(&tx, stream_id, action).await,
        None => {
            tracing::warn!(%session_id, %remote, "notify_remote_by_session: no live connection to remote peer, skipping remote signal");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map() -> ConnectionMap {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn fake_handle() -> (PeerConnectionHandle, mpsc::Receiver<SignalingMessage>) {
        let (tx, rx) = mpsc::channel(8);
        let (events, _) = broadcast::channel(8);
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        (
            PeerConnectionHandle {
                tx,
                events,
                remote_addr: addr,
                connection_id: Uuid::new_v4(),
                abort: tokio::spawn(async {}).abort_handle(),
                abort_on_drop: std::sync::atomic::AtomicBool::new(true),
            },
            rx,
        )
    }

    #[tokio::test]
    async fn find_conn_tx_found_in_server() {
        let server = empty_map();
        let outgoing = empty_map();
        let peer = Uuid::new_v4();
        let (handle, _rx) = fake_handle();
        server.write().await.insert(peer, handle);
        assert!(find_conn_tx(&server, &outgoing, peer).await.is_some());
    }

    #[tokio::test]
    async fn find_conn_tx_found_in_outgoing() {
        let server = empty_map();
        let outgoing = empty_map();
        let peer = Uuid::new_v4();
        let (handle, _rx) = fake_handle();
        outgoing.write().await.insert(peer, handle);
        assert!(find_conn_tx(&server, &outgoing, peer).await.is_some());
    }

    #[tokio::test]
    async fn find_conn_tx_missing_returns_none() {
        let server = empty_map();
        let outgoing = empty_map();
        assert!(find_conn_tx(&server, &outgoing, Uuid::new_v4())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn notify_by_session_sends_control_to_remote() {
        let sessions = crate::net::manager::SessionManager::new();
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let sid = sessions.open_outgoing(local, remote).await;
        sessions.accept(&sid).await.unwrap();

        let server = empty_map();
        let outgoing = empty_map();
        let (handle, mut rx) = fake_handle();
        server.write().await.insert(remote, handle);

        notify_remote_by_session(
            &sessions,
            &server,
            &outgoing,
            sid.get(),
            7,
            StreamAction::Close,
        )
        .await;

        match rx.recv().await {
            Some(SignalingMessage::StreamControl { stream_id, action }) => {
                assert_eq!(stream_id, 7);
                assert_eq!(action, StreamAction::Close);
            }
            other => panic!("expected StreamControl, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_ack_accepted_returns_port() {
        let (tx, _) = broadcast::channel(8);
        let mut rx = tx.subscribe();
        tx.send(PeerEvent::Message(SignalingMessage::StreamOpenAck {
            stream_id: 7,
            accepted: true,
            udp_port: Some(5555),
        }))
        .unwrap();
        let port = wait_for_stream_open_ack(&mut rx, 7, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(port, 5555);
    }

    #[tokio::test]
    async fn wait_for_ack_rejected_is_protocol_error_not_timeout() {
        let (tx, _) = broadcast::channel(8);
        let mut rx = tx.subscribe();
        tx.send(PeerEvent::Message(SignalingMessage::StreamOpenAck {
            stream_id: 3,
            accepted: false,
            udp_port: None,
        }))
        .unwrap();
        let err = wait_for_stream_open_ack(&mut rx, 3, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(
            matches!(err, NetError::SignalingProtocol { .. }),
            "rejection must surface as SignalingProtocol, got {err:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_ack_lagged_keeps_waiting() {
        let (tx, _) = broadcast::channel(2);
        let mut rx = tx.subscribe();
        for _ in 0..5 {
            tx.send(PeerEvent::Connected {
                peer_id: Uuid::new_v4(),
            })
            .unwrap();
        }
        tx.send(PeerEvent::Message(SignalingMessage::StreamOpenAck {
            stream_id: 1,
            accepted: true,
            udp_port: Some(4242),
        }))
        .unwrap();
        let port = wait_for_stream_open_ack(&mut rx, 1, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(port, 4242);
    }

    #[tokio::test]
    async fn wait_for_ack_deadline_is_timeout() {
        let (tx, _) = broadcast::channel(8);
        let mut rx = tx.subscribe();
        let err = wait_for_stream_open_ack(&mut rx, 9, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(
            matches!(err, NetError::Timeout { .. }),
            "no ack should time out, got {err:?}"
        );
    }

    #[test]
    fn build_stream_route_has_correct_endpoints_and_codec() {
        let local = Uuid::new_v4();
        let sink = Uuid::new_v4();
        let route = build_stream_route(local, sink, "mic0", "spk1", 96_000);
        assert_eq!(route.source.peer_id, local.to_string());
        assert_eq!(route.source.device_id, "mic0");
        assert_eq!(route.sink.peer_id, sink.to_string());
        assert_eq!(route.sink.device_id, "spk1");
        assert_eq!(route.codec.name, Codec::Opus);
        assert_eq!(route.codec.bitrate, 96_000);
        assert_eq!(route.codec.frame_ms, 20);
        assert_eq!(route.volume(), 1.0);
    }

    #[test]
    fn stream_open_message_has_correct_endpoints_and_codec() {
        let session = Uuid::new_v4();
        let local = Uuid::new_v4();
        let sink = Uuid::new_v4();
        let msg = stream_open_message(session, 4, local, sink, "system", "default", 64_000);
        match msg {
            SignalingMessage::StreamOpen {
                session_id,
                stream_id,
                source,
                sink: sink_ep,
                codec,
                udp_port,
            } => {
                assert_eq!(session_id, session.to_string());
                assert_eq!(stream_id, 4);
                assert_eq!(source.peer_id, local.to_string());
                assert_eq!(source.device_id, "system");
                assert_eq!(sink_ep.peer_id, sink.to_string());
                assert_eq!(sink_ep.device_id, "default");
                assert_eq!(codec.name, Codec::Opus);
                assert_eq!(codec.bitrate, 64_000);
                assert_eq!(codec.frame_ms, 20);
                assert_eq!(udp_port, 0);
            }
            other => panic!("expected StreamOpen, got {other:?}"),
        }
    }
}
