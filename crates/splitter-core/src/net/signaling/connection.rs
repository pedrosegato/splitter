use crate::error::NetError;
use crate::net::signaling::heartbeat::build_heartbeat;
use crate::net::signaling::message::SignalingMessage;
use crate::net::stream_runtime::StreamRegistry;
use futures::{SinkExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

pub const REMOTE_PEER_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum PeerEvent {
    Connected { peer_id: Uuid },
    Message(SignalingMessage),
    Disconnected { reason: String },
}

#[derive(Debug)]
pub struct PeerConnectionHandle {
    pub tx: mpsc::Sender<SignalingMessage>,
    pub events: broadcast::Sender<PeerEvent>,
    pub peer_addr: std::net::SocketAddr,
    pub remote_addr: std::net::SocketAddr,
}

pub fn spawn_peer_connection(
    stream: TcpStream,
    registry: Option<Arc<StreamRegistry>>,
) -> Result<PeerConnectionHandle, NetError> {
    let peer_addr = match stream.peer_addr() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "dropping connection: peer_addr unavailable");
            return Err(NetError::UdpIo(e));
        }
    };
    let (msg_tx, mut msg_rx) = mpsc::channel::<SignalingMessage>(64);
    let (event_tx, _) = broadcast::channel::<PeerEvent>(64);
    let event_tx_task = event_tx.clone();

    tokio::spawn(async move {
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(1 << 20)
            .new_codec();
        let mut framed = Framed::new(stream, codec);
        let mut hb_tick = tokio::time::interval(Duration::from_secs(1));
        hb_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_heard = tokio::time::Instant::now();
        let mut deadline = tokio::time::interval(Duration::from_millis(500));
        deadline.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                outgoing = msg_rx.recv() => {
                    let Some(msg) = outgoing else {
                        let _ = event_tx_task.send(PeerEvent::Disconnected {
                            reason: "outbox closed".into(),
                        });
                        break;
                    };
                    match msg.encode_to_bytes() {
                        Ok(bytes) => {
                            if let Err(e) = framed.send(bytes).await {
                                let _ = event_tx_task.send(PeerEvent::Disconnected {
                                    reason: format!("send: {e}"),
                                });
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = event_tx_task.send(PeerEvent::Disconnected {
                                reason: format!("encode: {e}"),
                            });
                            break;
                        }
                    }
                }
                incoming = framed.next() => {
                    match incoming {
                        Some(Ok(buf)) => match SignalingMessage::decode_from_slice(&buf) {
                            Ok(msg) => {
                                last_heard = tokio::time::Instant::now();
                                if let (
                                    SignalingMessage::Heartbeat { timestamp_ms: remote_ts, .. },
                                    Some(reg),
                                ) = (&msg, &registry) {
                                    let now_ms = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as u64)
                                        .unwrap_or(0);
                                    for (_, _, stats) in reg.current_stats().await {
                                        let echo_ms = stats.last_heartbeat_echo_ms.load(Ordering::Relaxed);
                                        if echo_ms > 0 && now_ms >= echo_ms {
                                            let rtt = (now_ms - echo_ms) as u32;
                                            stats.last_rtt_ms.store(rtt, Ordering::Relaxed);
                                        }
                                        stats.last_heartbeat_echo_ms.store(*remote_ts, Ordering::Relaxed);
                                    }
                                }
                                let _ = event_tx_task.send(PeerEvent::Message(msg));
                            }
                            Err(e) => {
                                let _ = event_tx_task.send(PeerEvent::Disconnected {
                                    reason: format!("decode: {e}"),
                                });
                                break;
                            }
                        },
                        Some(Err(e)) => {
                            let _ = event_tx_task.send(PeerEvent::Disconnected {
                                reason: format!("recv: {e}"),
                            });
                            break;
                        }
                        None => {
                            let _ = event_tx_task.send(PeerEvent::Disconnected {
                                reason: "remote eof".into(),
                            });
                            break;
                        }
                    }
                }
                _ = hb_tick.tick() => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let hb = match &registry {
                        Some(reg) => build_heartbeat(reg, 1_000, now_ms).await,
                        None => SignalingMessage::Heartbeat {
                            timestamp_ms: now_ms,
                            streams_stats: Vec::new(),
                        },
                    };
                    match hb.encode_to_bytes() {
                        Ok(bytes) => {
                            if let Err(e) = framed.send(bytes).await {
                                let _ = event_tx_task.send(PeerEvent::Disconnected {
                                    reason: format!("hb send: {e}"),
                                });
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("encode hb: {e}");
                        }
                    }
                }
                _ = deadline.tick() => {
                    if last_heard.elapsed() > REMOTE_PEER_HEARTBEAT_TIMEOUT {
                        let _ = event_tx_task.send(PeerEvent::Disconnected {
                            reason: "heartbeat timeout".into(),
                        });
                        break;
                    }
                }
            }
        }
    });

    Ok(PeerConnectionHandle {
        tx: msg_tx,
        events: event_tx.clone(),
        peer_addr,
        remote_addr: peer_addr,
    })
}

pub async fn send_with_timeout(
    handle: &PeerConnectionHandle,
    msg: SignalingMessage,
    timeout: Duration,
) -> Result<(), NetError> {
    tokio::time::timeout(timeout, handle.tx.send(msg))
        .await
        .map_err(|_| NetError::Timeout {
            what: "signaling send".into(),
            millis: timeout.as_millis() as u64,
        })?
        .map_err(|_| NetError::ChannelClosed)?;
    Ok(())
}

#[doc(hidden)]
pub async fn _wire_for_tests() -> (PeerConnectionHandle, PeerConnectionHandle) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_fut = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        s
    });
    let client = TcpStream::connect(addr).await.unwrap();
    let server = server_fut.await.unwrap();
    (
        spawn_peer_connection(server, None).unwrap(),
        spawn_peer_connection(client, None).unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::signaling::message::{Capabilities, PROTOCOL_VERSION};

    #[tokio::test]
    async fn round_trip_hello() {
        let (server, client) = _wire_for_tests().await;
        let mut server_events = server.events.subscribe();

        let hello = SignalingMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            peer_id: "p".into(),
            peer_name: "n".into(),
            app_version: "0".into(),
            capabilities: Capabilities {
                codecs: vec!["opus".into()],
                max_streams: 1,
            },
            auth_token: "t".into(),
        };
        client.tx.send(hello.clone()).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), server_events.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            PeerEvent::Message(SignalingMessage::Hello { peer_id, .. }) => {
                assert_eq!(peer_id, "p");
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropping_handle_disconnects_remote() {
        let (server, client) = _wire_for_tests().await;
        let mut server_events = server.events.subscribe();
        drop(client);
        let event = tokio::time::timeout(Duration::from_secs(2), server_events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, PeerEvent::Disconnected { .. }));
    }

    #[tokio::test]
    async fn heartbeats_arrive_at_one_per_second_window() {
        let (server, _client) = _wire_for_tests().await;
        let mut server_events = server.events.subscribe();
        let start = std::time::Instant::now();
        let mut beats = 0u32;
        while start.elapsed() < Duration::from_millis(2_400) {
            if let Ok(Ok(PeerEvent::Message(SignalingMessage::Heartbeat { .. }))) =
                tokio::time::timeout(Duration::from_millis(1_500), server_events.recv()).await
            {
                beats += 1;
            }
        }
        assert!(
            beats >= 2,
            "expected >= 2 heartbeats in 2.4s window, got {beats}"
        );
    }

    #[tokio::test]
    async fn missing_heartbeats_for_5s_emits_disconnect() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_fut = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            s
        });
        let _client = TcpStream::connect(addr).await.unwrap();
        let server = server_fut.await.unwrap();
        let handle = spawn_peer_connection(server, None).unwrap();
        let mut events = handle.events.subscribe();
        let saw_disconnect = tokio::time::timeout(
            REMOTE_PEER_HEARTBEAT_TIMEOUT + Duration::from_secs(2),
            async {
                loop {
                    if let Ok(PeerEvent::Disconnected { .. }) = events.recv().await {
                        return true;
                    }
                }
            },
        )
        .await
        .unwrap_or(false);
        assert!(
            saw_disconnect,
            "expected disconnect after 5s of no heartbeats"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn peer_addr_failure_returns_err_not_panic() {
        use std::os::unix::io::FromRawFd;
        let (r, _w) = nix_pipe();
        let stream = unsafe { std::net::TcpStream::from_raw_fd(r) };
        let tokio_stream = {
            stream.set_nonblocking(true).unwrap();
            tokio::net::TcpStream::from_std(stream).unwrap()
        };
        let result = spawn_peer_connection(tokio_stream, None);
        assert!(
            result.is_err(),
            "spawn_peer_connection must return Err when peer_addr is unavailable"
        );
        match result.unwrap_err() {
            crate::error::NetError::UdpIo(_) => {}
            other => panic!("expected NetError::UdpIo, got {other:?}"),
        }
    }

    #[cfg(unix)]
    fn nix_pipe() -> (i32, i32) {
        let mut fds = [0i32; 2];
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0, "pipe() failed");
        }
        (fds[0], fds[1])
    }
}
