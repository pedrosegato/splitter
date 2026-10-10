use crate::error::NetError;
use crate::net::crypto::{FrameCipher, SessionKeys};
use crate::net::signaling::message::SignalingMessage;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

pub const REMOTE_PEER_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

const FRAME_PLAIN: u8 = 0;
const FRAME_ENCRYPTED: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Dialer,
    Acceptor,
}

#[derive(Default)]
pub(crate) struct FrameCrypto {
    send: Option<FrameCipher>,
    pending_send: Option<FrameCipher>,
    recv: Option<FrameCipher>,
    peer_encrypting: bool,
}

impl std::fmt::Debug for FrameCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCrypto")
            .field("sending_encrypted", &self.send.is_some())
            .field("peer_encrypting", &self.peer_encrypting)
            .finish_non_exhaustive()
    }
}

impl FrameCrypto {
    fn seal(&mut self, plaintext: &[u8]) -> Bytes {
        let mut frame = Vec::with_capacity(plaintext.len() + 17);
        match self.send.as_mut() {
            Some(cipher) => {
                frame.push(FRAME_ENCRYPTED);
                frame.extend(cipher.seal(plaintext));
            }
            None => {
                frame.push(FRAME_PLAIN);
                frame.extend_from_slice(plaintext);
            }
        }
        Bytes::from(frame)
    }

    fn open(&mut self, frame: &[u8]) -> Result<SignalingMessage, String> {
        let (plaintext, encrypted) = match frame.split_first() {
            Some((&FRAME_PLAIN, _)) if self.peer_encrypting => {
                return Err("plaintext frame after encryption started".into())
            }
            Some((&FRAME_PLAIN, body)) => (body.to_vec(), false),
            Some((&FRAME_ENCRYPTED, body)) => {
                let cipher = self.recv.as_mut().ok_or("encrypted frame before keys")?;
                let plaintext = cipher.open(body).ok_or("frame failed authentication")?;
                self.peer_encrypting = true;
                (plaintext, true)
            }
            _ => return Err("malformed frame".into()),
        };
        let msg =
            SignalingMessage::decode_from_slice(&plaintext).map_err(|e| format!("decode: {e}"))?;
        if !encrypted && self.recv.is_some() && !allowed_in_plaintext(&msg) {
            return Err("plaintext message after key exchange".into());
        }
        Ok(msg)
    }
}

/// What a peer may still send in the clear after the key exchange: heartbeats already
/// in flight, and a rejection it sends before switching to encryption.
fn allowed_in_plaintext(msg: &SignalingMessage) -> bool {
    matches!(
        msg,
        SignalingMessage::Heartbeat { .. }
            | SignalingMessage::HelloAck {
                accepted: false,
                ..
            }
    )
}

fn lock(crypto: &Mutex<FrameCrypto>) -> std::sync::MutexGuard<'_, FrameCrypto> {
    crypto
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    pub remote_addr: std::net::SocketAddr,
    pub connection_id: Uuid,
    pub(crate) abort: tokio::task::AbortHandle,
    pub(crate) abort_on_drop: AtomicBool,
    pub(crate) crypto: Arc<Mutex<FrameCrypto>>,
    pub(crate) session: Arc<OnceLock<SessionKeys>>,
}

impl PeerConnectionHandle {
    /// Frames from the peer may be encrypted from now on; ours stay plaintext until
    /// `start_encrypting`, so the peer can still read anything sent before it has the keys.
    /// Call it before sending the message that hands the peer its half of the exchange.
    pub fn install_session(&self, keys: SessionKeys, role: Role) {
        let (send, recv) = match role {
            Role::Dialer => (keys.client_to_server, keys.server_to_client),
            Role::Acceptor => (keys.server_to_client, keys.client_to_server),
        };
        let mut crypto = lock(&self.crypto);
        crypto.pending_send = Some(FrameCipher::new(&send));
        crypto.recv = Some(FrameCipher::new(&recv));
        drop(crypto);
        let _ = self.session.set(keys);
    }

    pub fn start_encrypting(&self) {
        let mut crypto = lock(&self.crypto);
        if let Some(cipher) = crypto.pending_send.take() {
            crypto.send = Some(cipher);
        }
    }

    pub fn session_keys(&self) -> Option<SessionKeys> {
        self.session.get().cloned()
    }

    /// (sending encrypted, peer has sent encrypted)
    #[cfg(test)]
    pub(crate) fn encryption_state(&self) -> (bool, bool) {
        let crypto = lock(&self.crypto);
        (crypto.send.is_some(), crypto.peer_encrypting)
    }

    /// Force the connection task to stop, dropping its `TcpStream` and closing
    /// the socket immediately. Needed because a lingering `tx` clone anywhere
    /// would otherwise keep the task's `recv()` alive and the socket open.
    pub fn shutdown(&self) {
        self.abort.abort();
    }

    /// Opt this handle out of abort-on-drop so the task drains its outbox and
    /// flushes any buffered outgoing message before exiting. Used on the reject
    /// paths that send a final `HelloAck` then drop the handle.
    pub fn disarm(&self) {
        self.abort_on_drop.store(false, Ordering::Relaxed);
    }
}

impl Drop for PeerConnectionHandle {
    fn drop(&mut self) {
        if self.abort_on_drop.load(Ordering::Relaxed) {
            self.abort.abort();
        }
    }
}

pub fn spawn_peer_connection(stream: TcpStream) -> Result<PeerConnectionHandle, NetError> {
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
    let crypto: Arc<Mutex<FrameCrypto>> = Arc::default();
    let crypto_task = crypto.clone();

    let task = tokio::spawn(async move {
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
                        // Closing with unread inbound bytes makes Windows send RST, which can
                        // discard our last message (a rejecting HelloAck, a ProbeAck).
                        let _ = tokio::io::AsyncWriteExt::shutdown(framed.get_mut()).await;
                        let _ = tokio::time::timeout(Duration::from_millis(500), async {
                            while let Some(Ok(_)) = framed.next().await {}
                        })
                        .await;
                        break;
                    };
                    match msg.encode_to_bytes() {
                        Ok(bytes) => {
                            let frame = lock(&crypto_task).seal(&bytes);
                            if let Err(e) = framed.send(frame).await {
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
                        Some(Ok(buf)) => {
                            let decoded = lock(&crypto_task).open(&buf);
                            match decoded {
                                Ok(msg) => {
                                    last_heard = tokio::time::Instant::now();
                                    let _ = event_tx_task.send(PeerEvent::Message(msg));
                                }
                                Err(reason) => {
                                    let _ = event_tx_task.send(PeerEvent::Disconnected { reason });
                                    break;
                                }
                            }
                        }
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
                    // Liveness only; stream stats and RTT travel in UDP receiver reports.
                    let hb = SignalingMessage::Heartbeat {
                        timestamp_ms: now_ms,
                        streams_stats: Vec::new(),
                    };
                    match hb.encode_to_bytes() {
                        Ok(bytes) => {
                            let frame = lock(&crypto_task).seal(&bytes);
                            if let Err(e) = framed.send(frame).await {
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
        remote_addr: peer_addr,
        connection_id: Uuid::new_v4(),
        abort: task.abort_handle(),
        abort_on_drop: AtomicBool::new(true),
        crypto,
        session: Arc::default(),
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
        spawn_peer_connection(server).unwrap(),
        spawn_peer_connection(client).unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::crypto::_session_for_tests as test_session;
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
            client_nonce: "n".into(),
            client_key: "k".into(),
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
    async fn dropping_handle_aborts_its_task() {
        let (server, client) = _wire_for_tests().await;
        let abort = server.abort.clone();
        assert!(!abort.is_finished(), "task must be live before drop");
        drop(server);
        let aborted = tokio::time::timeout(Duration::from_secs(2), async {
            while !abort.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        assert!(aborted, "dropping the handle must abort its spawned task");
        drop(client);
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
        let handle = spawn_peer_connection(server).unwrap();
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

    fn renamed(name: &str) -> SignalingMessage {
        SignalingMessage::PeerRenamed {
            peer_id: "p".into(),
            peer_name: name.into(),
        }
    }

    /// A connection handle on one end and the raw framed socket on the other.
    async fn raw_wire() -> (
        PeerConnectionHandle,
        Framed<TcpStream, LengthDelimitedCodec>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await.unwrap().0 });
        let raw = TcpStream::connect(addr).await.unwrap();
        let handle = spawn_peer_connection(accept.await.unwrap()).unwrap();
        (handle, Framed::new(raw, LengthDelimitedCodec::new()))
    }

    async fn next_frame_with(
        raw: &mut Framed<TcpStream, LengthDelimitedCodec>,
        mut wanted: impl FnMut(&[u8]) -> bool,
    ) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let frame = raw.next().await.unwrap().unwrap().to_vec();
                if wanted(&frame) {
                    return frame;
                }
            }
        })
        .await
        .expect("frame not received")
    }

    async fn disconnect_reason(events: &mut broadcast::Receiver<PeerEvent>) -> String {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(PeerEvent::Disconnected { reason }) = events.recv().await {
                    return reason;
                }
            }
        })
        .await
        .expect("connection should have dropped")
    }

    fn plain_frame(msg: &SignalingMessage) -> Bytes {
        let mut frame = vec![FRAME_PLAIN];
        frame.extend_from_slice(&msg.encode_to_bytes().unwrap());
        frame.into()
    }

    fn sealed_frame(cipher: &mut FrameCipher, msg: &SignalingMessage) -> Bytes {
        let mut frame = vec![FRAME_ENCRYPTED];
        frame.extend(cipher.seal(&msg.encode_to_bytes().unwrap()));
        frame.into()
    }

    #[tokio::test]
    async fn messages_round_trip_once_both_sides_encrypt() {
        let (server, client) = _wire_for_tests().await;
        let keys = test_session();
        server.install_session(keys.clone(), Role::Acceptor);
        client.install_session(keys, Role::Dialer);
        client.start_encrypting();
        server.start_encrypting();
        let mut server_events = server.events.subscribe();
        let mut client_events = client.events.subscribe();

        client.tx.send(renamed("to-server")).await.unwrap();
        server.tx.send(renamed("to-client")).await.unwrap();

        for (events, expected) in [
            (&mut server_events, "to-server"),
            (&mut client_events, "to-client"),
        ] {
            let got = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(PeerEvent::Message(SignalingMessage::PeerRenamed {
                        peer_name, ..
                    })) = events.recv().await
                    {
                        return peer_name;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(got, expected);
        }
    }

    #[tokio::test]
    async fn encrypted_frames_do_not_reveal_the_message() {
        let (handle, mut raw) = raw_wire().await;
        let keys = test_session();
        handle.install_session(keys.clone(), Role::Acceptor);
        handle.start_encrypting();
        handle.tx.send(renamed("secret-marker")).await.unwrap();

        let mut cipher = FrameCipher::new(&keys.server_to_client);
        let mut opened = Vec::new();
        next_frame_with(&mut raw, |frame| {
            assert_eq!(frame[0], FRAME_ENCRYPTED);
            assert!(!String::from_utf8_lossy(frame).contains("secret-marker"));
            opened = cipher.open(&frame[1..]).expect("sealed with the s2c key");
            String::from_utf8_lossy(&opened).contains("peer_renamed")
        })
        .await;
        assert!(String::from_utf8_lossy(&opened).contains("secret-marker"));
    }

    #[tokio::test]
    async fn plaintext_is_refused_once_the_peer_encrypts() {
        let (handle, mut raw) = raw_wire().await;
        let mut events = handle.events.subscribe();
        let keys = test_session();
        handle.install_session(keys.clone(), Role::Acceptor);
        let mut cipher = FrameCipher::new(&keys.client_to_server);

        raw.send(sealed_frame(&mut cipher, &renamed("sealed")))
            .await
            .unwrap();
        raw.send(plain_frame(&SignalingMessage::Heartbeat {
            timestamp_ms: 0,
            streams_stats: Vec::new(),
        }))
        .await
        .unwrap();

        let reason = disconnect_reason(&mut events).await;
        assert!(reason.contains("plaintext"), "{reason}");
    }

    #[tokio::test]
    async fn only_heartbeats_and_rejections_may_stay_plaintext_after_the_key_exchange() {
        let (handle, mut raw) = raw_wire().await;
        let mut events = handle.events.subscribe();
        handle.install_session(test_session(), Role::Acceptor);

        raw.send(plain_frame(&SignalingMessage::Heartbeat {
            timestamp_ms: 0,
            streams_stats: Vec::new(),
        }))
        .await
        .unwrap();
        raw.send(plain_frame(&renamed("forged"))).await.unwrap();

        let saw_heartbeat = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match events.recv().await {
                    Ok(PeerEvent::Message(SignalingMessage::Heartbeat { .. })) => return true,
                    Ok(PeerEvent::Message(_)) => return false,
                    Ok(PeerEvent::Disconnected { .. }) | Err(_) => return false,
                    Ok(PeerEvent::Connected { .. }) => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(saw_heartbeat);
        let reason = disconnect_reason(&mut events).await;
        assert!(reason.contains("plaintext"), "{reason}");
    }

    #[tokio::test]
    async fn a_tampered_frame_drops_the_connection() {
        let (handle, mut raw) = raw_wire().await;
        let mut events = handle.events.subscribe();
        let keys = test_session();
        handle.install_session(keys.clone(), Role::Acceptor);
        let mut frame =
            sealed_frame(&mut FrameCipher::new(&keys.client_to_server), &renamed("x")).to_vec();
        let last = frame.len() - 1;
        frame[last] ^= 1;
        raw.send(frame.into()).await.unwrap();

        let reason = disconnect_reason(&mut events).await;
        assert!(reason.contains("authentication"), "{reason}");
    }

    #[tokio::test]
    async fn own_frames_stay_plaintext_until_start_encrypting() {
        let (handle, mut raw) = raw_wire().await;
        handle.install_session(test_session(), Role::Acceptor);
        handle.tx.send(renamed("before")).await.unwrap();
        let frame =
            next_frame_with(&mut raw, |f| String::from_utf8_lossy(f).contains("before")).await;
        assert_eq!(frame[0], FRAME_PLAIN);
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
        let result = spawn_peer_connection(tokio_stream);
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
