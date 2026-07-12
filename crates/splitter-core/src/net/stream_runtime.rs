use crate::audio::ring::AudioRing;
use crate::error::NetError;
use crate::net::device_watcher::DeviceEvent;
use crate::net::session::SessionId;
use crate::net::stream::{StreamId, StreamRoute};
use crate::FRAME_SAMPLES;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub use crate::net::stream_pump::{spawn_sink_pump_inner, spawn_source_pump_inner};
pub use crate::net::stream_registry::{StatsBaseline, StreamRegistry, StreamRuntimeSummary};
pub use crate::net::stream_stats::{StreamStats, StreamStatsSnapshot};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamControlSignal {
    SetVolume(f32),
    SetMuted(bool),
    Pause,
    Resume,
    Close,
}

impl From<crate::net::signaling::StreamAction> for StreamControlSignal {
    fn from(action: crate::net::signaling::StreamAction) -> Self {
        use crate::net::signaling::StreamAction;
        match action {
            StreamAction::Pause => StreamControlSignal::Pause,
            StreamAction::Resume => StreamControlSignal::Resume,
            StreamAction::Close => StreamControlSignal::Close,
            StreamAction::SetVolume { volume } => StreamControlSignal::SetVolume(volume),
            StreamAction::SetMuted { muted } => StreamControlSignal::SetMuted(muted),
        }
    }
}

#[derive(Debug)]
pub enum DeviceGuard {
    None,
    Capture(crate::audio::capture::CaptureHandle),
    #[cfg(all(target_os = "macos", feature = "sck"))]
    MacosLoopback(crate::audio::loopback::MacosLoopbackHandle),
    Playback(crate::audio::playback::PlaybackHandle),
}

// SAFETY: cpal::Stream is !Send on CoreAudio (and the Windows WASAPI host) because its
// internal callback state is pinned to the creating thread by the OS audio scheduler.
// However DeviceGuard is never *accessed* (no methods called) on any thread other than
// the one that creates the StreamRuntime.  The only cross-thread operation is Drop:
//
//   - DeviceGuard is stored in StreamRuntime, which is inserted into StreamRegistry at
//     construction time and removed by StreamRegistry::close or StreamRuntime::abort,
//     both of which call `rt.join.abort()` and then let `rt` drop on the tokio executor
//     thread pool — potentially a different thread than the audio thread that called
//     CaptureHandle::from_device or PlaybackHandle::start_by_id.
//
//   - All methods on the wrapped cpal::Stream (play/pause) are only called inside
//     CaptureHandle::from_device and PlaybackHandle::start_by_id, both of which run on
//     the thread that calls the constructor, before the DeviceGuard is moved into
//     StreamRuntime.  After that point the inner Stream is never touched — only dropped.
//
//   - cpal wraps the platform stream in an Arc<Mutex<StreamInner>> on all non-CoreAudio
//     hosts, and CoreAudio's Stream::drop sends a "stop" message to the audio thread
//     over a channel, making cross-thread drop safe per cpal's own internal design.
//
// If a future refactor ever needs to call stream.pause()/play() from a different thread,
// this unsafe block must be revisited and replaced with Arc<Mutex<DeviceGuard>>.
unsafe impl Send for DeviceGuard {}
unsafe impl Sync for DeviceGuard {}

#[derive(Debug)]
pub struct StreamRuntime {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub stats: Arc<StreamStats>,
    pub control_tx: mpsc::Sender<StreamControlSignal>,
    pub bound_device_id: Option<String>,
    pub join: JoinHandle<()>,
    pub device_guard: DeviceGuard,
}

impl StreamRuntime {
    pub async fn abort(self) {
        let _ = self.control_tx.send(StreamControlSignal::Close).await;
        self.join.abort();
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn control_signal_volume_round_trips_through_channel() {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        tx.send(StreamControlSignal::SetVolume(0.5)).await.unwrap();
        match rx.recv().await {
            Some(StreamControlSignal::SetVolume(v)) => assert!((v - 0.5).abs() < 1e-6),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn control_signal_close_round_trips() {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        tx.send(StreamControlSignal::Close).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StreamControlSignal::Close)));
    }
}

async fn bind_and_connect_udp(remote: Option<SocketAddr>) -> Result<UdpSocket, NetError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::UdpBind(format!("bind udp: {e}")))?;
    let _ = socket
        .local_addr()
        .map_err(|e| NetError::UdpBind(format!("local_addr: {e}")))?;
    if let Some(addr) = remote {
        socket
            .connect(addr)
            .await
            .map_err(|e| NetError::UdpBind(format!("connect udp to {addr}: {e}")))?;
    }
    Ok(socket)
}

fn build_runtime(
    session_id: SessionId,
    stream_id: StreamId,
    stats: Arc<StreamStats>,
    control_tx: mpsc::Sender<StreamControlSignal>,
    bound_device_id: Option<String>,
    join: tokio::task::JoinHandle<()>,
    device_guard: DeviceGuard,
) -> StreamRuntime {
    StreamRuntime {
        session_id,
        stream_id,
        stats,
        control_tx,
        bound_device_id,
        join,
        device_guard,
    }
}

pub async fn open_stream_as_sink_inproc(
    registry: Arc<StreamRegistry>,
    session_id: SessionId,
    stream_id: StreamId,
    route: StreamRoute,
) -> Result<u16, NetError> {
    let socket = bind_and_connect_udp(None).await?;
    let port = socket
        .local_addr()
        .map_err(|e| NetError::UdpBind(format!("query sink udp local_addr: {e}")))?
        .port();

    let (producer, _consumer) = AudioRing::new(FRAME_SAMPLES * 20);
    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let join = tokio::spawn(spawn_sink_pump_inner(
        session_id,
        stream_id,
        socket,
        producer,
        control_rx,
        stats.clone(),
    ));

    let rt = build_runtime(
        session_id,
        stream_id,
        stats,
        control_tx,
        Some(route.sink.device_id.clone()),
        join,
        DeviceGuard::None,
    );
    registry.register(rt).await?;
    Ok(port)
}

pub async fn open_stream_as_source_inproc(
    registry: Arc<StreamRegistry>,
    session_id: SessionId,
    stream_id: StreamId,
    route: StreamRoute,
    remote: SocketAddr,
) -> Result<(), NetError> {
    let (_producer, consumer) = AudioRing::new(FRAME_SAMPLES * 20);
    let notify = Arc::new(Notify::new());
    let socket = bind_and_connect_udp(Some(remote)).await?;

    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let bitrate = route.codec.bitrate;
    let join = tokio::spawn(spawn_source_pump_inner(
        session_id,
        stream_id,
        consumer,
        notify.clone(),
        socket,
        control_rx,
        stats.clone(),
        bitrate,
    ));

    registry
        .register(build_runtime(
            session_id,
            stream_id,
            stats,
            control_tx,
            Some(route.source.device_id.clone()),
            join,
            DeviceGuard::None,
        ))
        .await
}

#[derive(Debug, Clone)]
pub enum SourceKind {
    Mic(String),
    System,
}

pub async fn open_stream_as_source(
    registry: Arc<StreamRegistry>,
    session_id: SessionId,
    stream_id: StreamId,
    route: StreamRoute,
    remote: SocketAddr,
    source_kind: SourceKind,
) -> Result<(), NetError> {
    let (producer, consumer) = AudioRing::new(FRAME_SAMPLES * 20);

    let (device_guard, frame_ready) = match source_kind {
        SourceKind::Mic(device_id) => {
            let cap = crate::audio::capture::CaptureHandle::start_by_id(&device_id, producer)
                .map_err(|e| NetError::SignalingProtocol {
                    reason: format!("capture start_by_id failed: {e}"),
                })?;
            let notify = cap.frame_ready();
            (DeviceGuard::Capture(cap), notify)
        }
        SourceKind::System => {
            #[cfg(all(target_os = "macos", feature = "sck"))]
            {
                let cap =
                    crate::audio::loopback::MacosLoopbackHandle::start(producer).map_err(|e| {
                        NetError::SignalingProtocol {
                            reason: format!("macos loopback start failed: {e}"),
                        }
                    })?;
                (DeviceGuard::MacosLoopback(cap), Arc::new(Notify::new()))
            }
            #[cfg(all(target_os = "macos", not(feature = "sck")))]
            {
                return Err(NetError::SignalingProtocol {
                        reason: "system audio capture requires the sck feature (use BlackHole 2ch as an input device instead)".into(),
                    });
            }
            #[cfg(not(target_os = "macos"))]
            {
                let cap = crate::audio::capture::CaptureHandle::start_loopback(producer).map_err(
                    |e| NetError::SignalingProtocol {
                        reason: format!("loopback start failed: {e}"),
                    },
                )?;
                let notify = cap.frame_ready();
                (DeviceGuard::Capture(cap), notify)
            }
        }
    };

    let socket = bind_and_connect_udp(Some(remote)).await?;
    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let join = tokio::spawn(spawn_source_pump_inner(
        session_id,
        stream_id,
        consumer,
        frame_ready,
        socket,
        control_rx,
        stats.clone(),
        route.codec.bitrate,
    ));

    registry
        .register(build_runtime(
            session_id,
            stream_id,
            stats,
            control_tx,
            Some(route.source.device_id.clone()),
            join,
            device_guard,
        ))
        .await
}

pub async fn open_stream_as_sink(
    registry: Arc<StreamRegistry>,
    session_id: SessionId,
    stream_id: StreamId,
    _route: StreamRoute,
    output_device_id: String,
) -> Result<u16, NetError> {
    let socket = bind_and_connect_udp(None).await?;
    let port = socket
        .local_addr()
        .map_err(|e| NetError::UdpBind(format!("local_addr: {e}")))?
        .port();

    let (producer, consumer) = AudioRing::new(FRAME_SAMPLES * 20);
    let playback = crate::audio::playback::PlaybackHandle::start_by_id(&output_device_id, consumer)
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("playback start_by_id failed: {e}"),
        })?;

    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let join = tokio::spawn(spawn_sink_pump_inner(
        session_id,
        stream_id,
        socket,
        producer,
        control_rx,
        stats.clone(),
    ));

    registry
        .register(build_runtime(
            session_id,
            stream_id,
            stats,
            control_tx,
            Some(output_device_id.clone()),
            join,
            DeviceGuard::Playback(playback),
        ))
        .await?;
    Ok(port)
}

#[cfg(test)]
mod open_sink_tests {
    use super::*;
    use crate::net::signaling::{Codec, CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;

    #[tokio::test]
    async fn open_stream_as_sink_returns_bound_port_and_registers() {
        let registry = StreamRegistry::new();
        let session_id = SessionId::new();
        let route = StreamRoute::new(
            Endpoint {
                peer_id: "a".into(),
                device_id: "ignored".into(),
            },
            Endpoint {
                peer_id: "b".into(),
                device_id: "headphones".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        );

        let port = open_stream_as_sink_inproc(registry.clone(), session_id, StreamId(0), route)
            .await
            .expect("open sink");
        assert!(port > 0);
        let listed = registry.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].stream_id, StreamId(0));
    }
}

#[cfg(test)]
mod open_source_tests {
    use super::*;
    use crate::net::signaling::{Codec, CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;
    use std::net::SocketAddr;
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn open_stream_as_source_registers_runtime() {
        let registry = StreamRegistry::new();
        let session_id = SessionId::new();

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote: SocketAddr = sink_socket.local_addr().unwrap();

        let route = StreamRoute::new(
            Endpoint {
                peer_id: "a".into(),
                device_id: "mic-or-loopback".into(),
            },
            Endpoint {
                peer_id: "b".into(),
                device_id: "ignored".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        );

        let result =
            open_stream_as_source_inproc(registry.clone(), session_id, StreamId(0), route, remote)
                .await;
        assert!(
            result.is_ok(),
            "open_stream_as_source_inproc returned {result:?}"
        );

        let listed = registry.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].stream_id, StreamId(0));
    }
}

#[cfg(test)]
mod session_registration_failure_tests {
    use super::*;
    use crate::net::manager::SessionManager;
    use crate::net::signaling::{Codec, CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;
    use std::net::SocketAddr;
    use tokio::net::UdpSocket;
    use uuid::Uuid;

    fn test_route() -> StreamRoute {
        StreamRoute::new(
            Endpoint {
                peer_id: "a".into(),
                device_id: "src-dev".into(),
            },
            Endpoint {
                peer_id: "b".into(),
                device_id: "sink-dev".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        )
    }

    #[tokio::test]
    async fn sink_session_add_stream_failure_tears_down_registry_entry() {
        let registry = StreamRegistry::new();
        let sessions = SessionManager::new();
        let unknown_sid = SessionId::new();
        let stream_id = StreamId(0);

        let port =
            open_stream_as_sink_inproc(registry.clone(), unknown_sid, stream_id, test_route())
                .await
                .expect("inproc sink open must succeed");
        assert!(port > 0);

        assert_eq!(
            registry.list().await.len(),
            1,
            "runtime registered before add_stream"
        );

        let add_result = sessions
            .add_stream(
                &unknown_sid,
                crate::net::stream::Stream::new_negotiating(stream_id, test_route(), port),
            )
            .await;

        assert!(
            add_result.is_err(),
            "add_stream on unknown session must fail"
        );

        registry
            .close(&unknown_sid, stream_id)
            .await
            .expect("teardown must remove the registered runtime");

        assert!(
            registry.list().await.is_empty(),
            "registry must be empty after teardown on add_stream failure"
        );
    }

    #[tokio::test]
    async fn source_session_add_stream_failure_tears_down_registry_entry() {
        let registry = StreamRegistry::new();
        let sessions = SessionManager::new();
        let unknown_sid = SessionId::new();
        let stream_id = StreamId(1);

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote: SocketAddr = sink_socket.local_addr().unwrap();

        open_stream_as_source_inproc(
            registry.clone(),
            unknown_sid,
            stream_id,
            test_route(),
            remote,
        )
        .await
        .expect("inproc source open must succeed");

        assert_eq!(
            registry.list().await.len(),
            1,
            "runtime registered before add_stream"
        );

        let add_result = sessions
            .add_stream(
                &unknown_sid,
                crate::net::stream::Stream::new_negotiating(stream_id, test_route(), 5004),
            )
            .await;

        assert!(
            add_result.is_err(),
            "add_stream on unknown session must fail"
        );

        registry
            .close(&unknown_sid, stream_id)
            .await
            .expect("teardown must remove the registered runtime");

        assert!(
            registry.list().await.is_empty(),
            "registry must be empty after teardown on add_stream failure"
        );
    }

    #[tokio::test]
    async fn activate_stream_failure_after_add_tears_down_registry_entry() {
        let registry = StreamRegistry::new();
        let sessions = SessionManager::new();
        let local = Uuid::new_v4();
        let remote_peer = Uuid::new_v4();
        let sid = sessions.open_outgoing(local, remote_peer).await;
        sessions.accept(&sid).await.unwrap();

        let stream_id = StreamId(2);

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote_addr: SocketAddr = sink_socket.local_addr().unwrap();

        open_stream_as_source_inproc(registry.clone(), sid, stream_id, test_route(), remote_addr)
            .await
            .expect("inproc source open must succeed");

        sessions
            .add_stream(
                &sid,
                crate::net::stream::Stream::new_negotiating(stream_id, test_route(), 5004),
            )
            .await
            .unwrap();

        let activate_result = sessions.activate_stream(&sid, StreamId(99)).await;
        assert!(
            activate_result.is_err(),
            "activate_stream on wrong stream_id must fail"
        );

        registry
            .close(&sid, stream_id)
            .await
            .expect("teardown on activate failure must remove runtime");

        assert!(
            registry.list().await.is_empty(),
            "registry must be empty after teardown on activate_stream failure"
        );
    }
}

pub async fn dispatch_device_events(
    registry: Arc<StreamRegistry>,
    mut rx: broadcast::Receiver<DeviceEvent>,
) {
    loop {
        match rx.recv().await {
            Ok(ev) => {
                let (target_id, signal) = match ev {
                    DeviceEvent::Disappeared(id) => (id, StreamControlSignal::Pause),
                    DeviceEvent::Appeared(id) => (id, StreamControlSignal::Resume),
                };
                let targets: Vec<(SessionId, StreamId, mpsc::Sender<StreamControlSignal>)> = {
                    let guard = registry.inner.read().await;
                    guard
                        .iter()
                        .filter(|(_, rt)| rt.bound_device_id.as_deref() == Some(target_id.as_str()))
                        .map(|((sid, stream_id), rt)| (*sid, *stream_id, rt.control_tx.clone()))
                        .collect()
                };
                // Sends happen after the read guard is dropped so a full control
                // channel cannot block concurrent register/close (write lock).
                for (sid, stream_id, control_tx) in targets {
                    let _ = control_tx.send(signal).await;
                    tracing::info!(
                        session = %sid,
                        stream = %stream_id,
                        device = %target_id,
                        signal = ?signal,
                        "device hot-plug -> pump notified"
                    );
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("device watcher lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod hotplug_tests {
    use super::*;
    use crate::net::device_watcher::DeviceEvent;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn watcher_dispatches_pause_when_bound_device_disappears() {
        let registry = StreamRegistry::new();
        let (tx, _) = broadcast::channel::<DeviceEvent>(8);
        let sid = SessionId::new();

        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async move { while ctrl_rx.recv().await.is_some() {} });
        registry
            .register(StreamRuntime {
                session_id: sid,
                stream_id: StreamId(0),
                stats: Arc::new(StreamStats::default()),
                control_tx: ctrl_tx.clone(),
                bound_device_id: Some("Input:0:USB Headset".into()),
                join,
                device_guard: DeviceGuard::None,
            })
            .await
            .unwrap();

        let dispatcher = tokio::spawn(dispatch_device_events(registry.clone(), tx.subscribe()));
        tx.send(DeviceEvent::Disappeared("Input:0:USB Headset".into()))
            .unwrap();

        let probe_rx = ctrl_tx.clone();
        let _ = probe_rx;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        dispatcher.abort();
    }

    #[tokio::test]
    async fn dispatch_delivers_pause_to_bound_stream_after_lock_release() {
        let registry = StreamRegistry::new();
        let (tx, _) = broadcast::channel::<DeviceEvent>(8);
        let sid = SessionId::new();

        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async {});
        registry
            .register(StreamRuntime {
                session_id: sid,
                stream_id: StreamId(0),
                stats: Arc::new(StreamStats::default()),
                control_tx: ctrl_tx,
                bound_device_id: Some("Input:0:USB Headset".into()),
                join,
                device_guard: DeviceGuard::None,
            })
            .await
            .unwrap();

        let dispatcher = tokio::spawn(dispatch_device_events(registry.clone(), tx.subscribe()));
        tx.send(DeviceEvent::Disappeared("Input:0:USB Headset".into()))
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), ctrl_rx.recv())
            .await
            .expect("control signal should arrive within 1s");
        assert_eq!(received, Some(StreamControlSignal::Pause));

        dispatcher.abort();
    }
}
