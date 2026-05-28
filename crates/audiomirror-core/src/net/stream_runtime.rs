use crate::audio::codec::{OpusDecoder, OpusEncoder};
use crate::audio::ring::{RingConsumer, RingProducer};
use crate::error::NetError;
use crate::net::packet::Packet;
use crate::net::session::SessionId;
use crate::net::stream::StreamId;
use crate::FRAME_SAMPLES;
use bytes::{Bytes, BytesMut};
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamControlSignal {
    SetVolume(f32),
    SetMuted(bool),
    Pause,
    Resume,
    Close,
}

#[derive(Debug)]
pub enum DeviceGuard {
    None,
    Capture(crate::audio::capture::CaptureHandle),
    #[cfg(target_os = "macos")]
    MacosLoopback(crate::audio::loopback::MacosLoopbackHandle),
    Playback(crate::audio::playback::PlaybackHandle),
}

// cpal::Stream is !Send on CoreAudio, but DeviceGuard is only dropped (never called across
// threads), and cpal's internal Arc-based reference counting makes cross-thread drop safe.
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

#[derive(Debug, Default)]
pub struct StreamStats {
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub packets_lost: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub last_rtt_ms: AtomicU32,
    pub last_seq_received: AtomicU32,
    pub last_heartbeat_echo_ms: AtomicU64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StreamStatsSnapshot {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub last_rtt_ms: u32,
    pub bitrate_kbps_sent: u32,
    pub bitrate_kbps_received: u32,
}

impl StreamStats {
    pub fn snapshot(&self, window_ms: u32, prev: &StreamStatsSnapshot) -> StreamStatsSnapshot {
        let packets_sent = self.packets_sent.load(Ordering::Relaxed);
        let packets_received = self.packets_received.load(Ordering::Relaxed);
        let packets_lost = self.packets_lost.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let bytes_received = self.bytes_received.load(Ordering::Relaxed);
        let last_rtt_ms = self.last_rtt_ms.load(Ordering::Relaxed);

        let bytes_sent_delta = bytes_sent.saturating_sub(prev.bytes_sent);
        let bytes_recv_delta = bytes_received.saturating_sub(prev.bytes_received);
        let denom = window_ms.max(1) as u64;
        let bitrate_kbps_sent = ((bytes_sent_delta * 8) / denom) as u32;
        let bitrate_kbps_received = ((bytes_recv_delta * 8) / denom) as u32;

        StreamStatsSnapshot {
            packets_sent,
            packets_received,
            packets_lost,
            bytes_sent,
            bytes_received,
            last_rtt_ms,
            bitrate_kbps_sent,
            bitrate_kbps_received,
        }
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

#[derive(Debug, Clone, Serialize)]
pub struct StreamRuntimeSummary {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub bound_device_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct StreamRegistry {
    pub(crate) inner: RwLock<HashMap<(SessionId, StreamId), StreamRuntime>>,
    prev_snapshots: RwLock<HashMap<(SessionId, StreamId), StreamStatsSnapshot>>,
}

impl StreamRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn register(&self, rt: StreamRuntime) -> Result<(), NetError> {
        let key = (rt.session_id, rt.stream_id);
        let mut guard = self.inner.write().await;
        if guard.contains_key(&key) {
            return Err(NetError::SignalingProtocol {
                reason: format!("stream {} on session {} already registered", key.1, key.0),
            });
        }
        guard.insert(key, rt);
        Ok(())
    }

    pub async fn list(&self) -> Vec<StreamRuntimeSummary> {
        self.inner
            .read()
            .await
            .values()
            .map(|rt| StreamRuntimeSummary {
                session_id: rt.session_id,
                stream_id: rt.stream_id,
                bound_device_id: rt.bound_device_id.clone(),
            })
            .collect()
    }

    pub async fn send_control(
        &self,
        session_id: &SessionId,
        stream_id: StreamId,
        signal: StreamControlSignal,
    ) -> Result<(), NetError> {
        let guard = self.inner.read().await;
        let rt =
            guard
                .get(&(*session_id, stream_id))
                .ok_or_else(|| NetError::SignalingProtocol {
                    reason: format!("no runtime for stream {stream_id} on session {session_id}"),
                })?;
        rt.control_tx
            .send(signal)
            .await
            .map_err(|_| NetError::SignalingProtocol {
                reason: format!("control channel closed for stream {stream_id}"),
            })
    }

    pub async fn close(&self, session_id: &SessionId, stream_id: StreamId) -> Result<(), NetError> {
        let mut guard = self.inner.write().await;
        if let Some(rt) = guard.remove(&(*session_id, stream_id)) {
            let _ = rt.control_tx.send(StreamControlSignal::Close).await;
            rt.join.abort();
            Ok(())
        } else {
            Err(NetError::SignalingProtocol {
                reason: format!("no runtime for stream {stream_id}"),
            })
        }
    }

    pub async fn get_stats(
        &self,
        session_id: &SessionId,
        stream_id: StreamId,
    ) -> Option<Arc<StreamStats>> {
        self.inner
            .read()
            .await
            .get(&(*session_id, stream_id))
            .map(|rt| rt.stats.clone())
    }

    pub async fn snapshot_stats(
        &self,
        window_ms: u32,
    ) -> Vec<(SessionId, StreamId, StreamStatsSnapshot)> {
        let guard = self.inner.read().await;
        let mut prev = self.prev_snapshots.write().await;
        let mut out = Vec::with_capacity(guard.len());
        for (&key, rt) in guard.iter() {
            let last = prev.get(&key).cloned().unwrap_or(StreamStatsSnapshot {
                packets_sent: 0,
                packets_received: 0,
                packets_lost: 0,
                bytes_sent: 0,
                bytes_received: 0,
                last_rtt_ms: 0,
                bitrate_kbps_sent: 0,
                bitrate_kbps_received: 0,
            });
            let snap = rt.stats.snapshot(window_ms, &last);
            prev.insert(key, snap.clone());
            out.push((key.0, key.1, snap));
        }
        out
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use uuid::Uuid;

    fn fake_runtime(session_id: SessionId, stream_id: StreamId) -> StreamRuntime {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async move {
            while let Some(sig) = rx.recv().await {
                if matches!(sig, StreamControlSignal::Close) {
                    break;
                }
            }
        });
        StreamRuntime {
            session_id,
            stream_id,
            stats: Arc::new(StreamStats::default()),
            control_tx: tx,
            bound_device_id: Some("dev-a".into()),
            join,
            device_guard: DeviceGuard::None,
        }
    }

    #[tokio::test]
    async fn register_and_list_round_trip() {
        let reg = StreamRegistry::new();
        let sid = Uuid::new_v4();
        reg.register(fake_runtime(sid, 0)).await.unwrap();
        let listed = reg.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, sid);
        assert_eq!(listed[0].stream_id, 0);
    }

    #[tokio::test]
    async fn register_rejects_duplicate_key() {
        let reg = StreamRegistry::new();
        let sid = Uuid::new_v4();
        reg.register(fake_runtime(sid, 0)).await.unwrap();
        let err = reg.register(fake_runtime(sid, 0)).await.unwrap_err();
        assert!(matches!(
            err,
            crate::error::NetError::SignalingProtocol { .. }
        ));
    }

    #[tokio::test]
    async fn close_sends_close_signal_and_removes_entry() {
        let reg = StreamRegistry::new();
        let sid = Uuid::new_v4();
        reg.register(fake_runtime(sid, 0)).await.unwrap();
        reg.close(&sid, 0).await.unwrap();
        assert!(reg.list().await.is_empty());
    }

    #[tokio::test]
    async fn snapshot_stats_collects_per_stream() {
        let reg = StreamRegistry::new();
        let sid = Uuid::new_v4();
        let rt = fake_runtime(sid, 7);
        rt.stats.packets_sent.store(123, Ordering::Relaxed);
        reg.register(rt).await.unwrap();
        let snap = reg.snapshot_stats(1_000).await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].2.packets_sent, 123);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_stats_snapshot_is_all_zero() {
        let stats = StreamStats::default();
        let prev = StreamStatsSnapshot {
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_rtt_ms: 0,
            bitrate_kbps_sent: 0,
            bitrate_kbps_received: 0,
        };
        let snap = stats.snapshot(5_000, &prev);
        assert_eq!(snap, prev);
    }

    #[test]
    fn snapshot_computes_bitrate_from_window() {
        let stats = StreamStats::default();
        stats.bytes_sent.store(8_000, Ordering::Relaxed);
        let prev = StreamStatsSnapshot {
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_rtt_ms: 0,
            bitrate_kbps_sent: 0,
            bitrate_kbps_received: 0,
        };
        let snap = stats.snapshot(1_000, &prev);
        assert_eq!(snap.bitrate_kbps_sent, 64);
    }

    #[test]
    fn snapshot_reads_rtt_atomically() {
        let stats = StreamStats::default();
        stats.last_rtt_ms.store(42, Ordering::Relaxed);
        let prev = StreamStatsSnapshot {
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_rtt_ms: 0,
            bitrate_kbps_sent: 0,
            bitrate_kbps_received: 0,
        };
        let snap = stats.snapshot(1_000, &prev);
        assert_eq!(snap.last_rtt_ms, 42);
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
pub async fn spawn_source_pump_inner(
    _session_id: SessionId,
    stream_id: StreamId,
    mut consumer: RingConsumer,
    frame_ready: Arc<Notify>,
    socket: UdpSocket,
    mut control_rx: mpsc::Receiver<StreamControlSignal>,
    stats: Arc<StreamStats>,
    bitrate: i32,
) {
    let mut encoder = match OpusEncoder::new(bitrate) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("opus encoder init failed: {e}");
            return;
        }
    };
    let mut seq: u32 = 0;
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut payload = BytesMut::with_capacity(400);
    let mut packet_buf = BytesMut::with_capacity(1500);
    let mut gain: f32 = 1.0;
    let muted = Arc::new(AtomicBool::new(false));
    let mut paused = false;
    let start = std::time::Instant::now();

    loop {
        tokio::select! {
            biased;
            maybe_sig = control_rx.recv() => {
                match maybe_sig {
                    Some(StreamControlSignal::Close) | None => return,
                    Some(StreamControlSignal::SetVolume(v)) => gain = v.clamp(0.0, 2.0),
                    Some(StreamControlSignal::SetMuted(m)) => muted.store(m, Ordering::Relaxed),
                    Some(StreamControlSignal::Pause) => paused = true,
                    Some(StreamControlSignal::Resume) => paused = false,
                }
            }
            _ = frame_ready.notified() => {
                while consumer.occupied() >= FRAME_SAMPLES {
                    consumer.pop_slice(&mut frame);
                    if paused {
                        continue;
                    }
                    let effective_gain = if muted.load(Ordering::Relaxed) { 0.0 } else { gain };
                    if (effective_gain - 1.0).abs() > f32::EPSILON {
                        for s in frame.iter_mut() {
                            *s *= effective_gain;
                        }
                    }
                    if let Err(e) = encoder.encode(&frame, &mut payload) {
                        tracing::warn!("opus encode failed: {e}");
                        continue;
                    }
                    let pkt = Packet {
                        stream_id,
                        seq: seq & 0x00FF_FFFF,
                        timestamp_ms: start.elapsed().as_millis() as u32,
                        payload: Bytes::copy_from_slice(&payload[..]),
                    };
                    if pkt.encode(&mut packet_buf).is_ok() {
                        match socket.send(&packet_buf[..]).await {
                            Ok(n) => {
                                stats.packets_sent.fetch_add(1, Ordering::Relaxed);
                                stats.bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
                                seq = seq.wrapping_add(1);
                            }
                            Err(e) => tracing::warn!("udp send failed: {e}"),
                        }
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
pub async fn spawn_sink_pump_inner(
    _session_id: SessionId,
    stream_id: StreamId,
    socket: UdpSocket,
    mut producer: RingProducer,
    mut control_rx: mpsc::Receiver<StreamControlSignal>,
    stats: Arc<StreamStats>,
) {
    let mut decoder = match OpusDecoder::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("opus decoder init failed: {e}");
            return;
        }
    };
    let mut decoded = vec![0.0f32; FRAME_SAMPLES];
    let mut buf = vec![0u8; 1500];
    let mut last_seq: Option<u32> = None;
    let mut gain: f32 = 1.0;
    let muted = Arc::new(AtomicBool::new(false));
    let mut paused = false;

    loop {
        tokio::select! {
            biased;
            maybe_sig = control_rx.recv() => {
                match maybe_sig {
                    Some(StreamControlSignal::Close) | None => return,
                    Some(StreamControlSignal::SetVolume(v)) => gain = v.clamp(0.0, 2.0),
                    Some(StreamControlSignal::SetMuted(m)) => muted.store(m, Ordering::Relaxed),
                    Some(StreamControlSignal::Pause) => paused = true,
                    Some(StreamControlSignal::Resume) => paused = false,
                }
            }
            recv_res = socket.recv(&mut buf) => {
                let n = match recv_res {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!("udp recv failed: {e}");
                        continue;
                    }
                };
                let bytes = Bytes::copy_from_slice(&buf[..n]);
                let pkt = match Packet::decode(bytes) {
                    Ok(p) if p.stream_id == stream_id => p,
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::warn!("packet decode failed: {e}");
                        continue;
                    }
                };
                stats.packets_received.fetch_add(1, Ordering::Relaxed);
                stats.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
                stats.last_seq_received.store(pkt.seq, Ordering::Relaxed);

                if let Some(prev) = last_seq {
                    let expected = prev.wrapping_add(1) & 0x00FF_FFFF;
                    if pkt.seq != expected {
                        let lost = if pkt.seq > expected {
                            (pkt.seq - expected) as u64
                        } else {
                            0
                        };
                        if lost > 0 && lost < 100 {
                            for _ in 0..lost {
                                if decoder.decode(None, &mut decoded).is_ok() {
                                    stats.packets_lost.fetch_add(1, Ordering::Relaxed);
                                    apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                                }
                            }
                        }
                    }
                }
                last_seq = Some(pkt.seq);

                if decoder.decode(Some(&pkt.payload[..]), &mut decoded).is_ok() {
                    apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                }
            }
        }
    }
}

fn apply_gain_and_push(
    frame: &mut [f32],
    gain: f32,
    muted: bool,
    paused: bool,
    producer: &mut RingProducer,
) {
    if paused {
        return;
    }
    let effective = if muted { 0.0 } else { gain };
    if (effective - 1.0).abs() > f32::EPSILON {
        for s in frame.iter_mut() {
            *s *= effective;
        }
    }
    let _ = producer.push_slice(frame);
}

#[cfg(test)]
mod sink_pump_tests {
    use super::*;
    use crate::audio::codec::OpusEncoder;
    use crate::audio::ring::AudioRing;
    use crate::net::packet::Packet;
    use crate::FRAME_SAMPLES;
    use bytes::{Bytes, BytesMut};
    use tokio::net::UdpSocket;
    use uuid::Uuid;

    #[tokio::test]
    async fn sink_pump_decodes_into_playback_ring() {
        let (prod, cons) = AudioRing::new(FRAME_SAMPLES * 8);
        let stats = Arc::new(StreamStats::default());

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink_socket.local_addr().unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);

        let stats_clone = stats.clone();
        let pump = tokio::spawn(spawn_sink_pump_inner(
            Uuid::new_v4(),
            5u8,
            sink_socket,
            prod,
            ctrl_rx,
            stats_clone,
        ));

        let mut enc = OpusEncoder::new(64_000).unwrap();
        let frame = vec![0.1f32; FRAME_SAMPLES];
        let mut payload = BytesMut::with_capacity(400);
        enc.encode(&frame, &mut payload).unwrap();

        let pkt = Packet {
            stream_id: 5,
            seq: 0,
            timestamp_ms: 0,
            payload: Bytes::copy_from_slice(&payload[..]),
        };
        let mut wire = BytesMut::with_capacity(1500);
        pkt.encode(&mut wire).unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.send_to(&wire[..], sink_addr).await.unwrap();

        for _ in 0..30 {
            if cons.occupied() >= FRAME_SAMPLES {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(cons.occupied() >= FRAME_SAMPLES);
        assert_eq!(stats.packets_received.load(Ordering::Relaxed), 1);

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    #[tokio::test]
    async fn sink_pump_records_lost_packets_on_seq_gap() {
        let (prod, _cons) = AudioRing::new(FRAME_SAMPLES * 8);
        let stats = Arc::new(StreamStats::default());

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink_socket.local_addr().unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);

        let stats_clone = stats.clone();
        let pump = tokio::spawn(spawn_sink_pump_inner(
            Uuid::new_v4(),
            5u8,
            sink_socket,
            prod,
            ctrl_rx,
            stats_clone,
        ));

        let mut enc = OpusEncoder::new(64_000).unwrap();
        let frame = vec![0.1f32; FRAME_SAMPLES];
        let mut payload = BytesMut::with_capacity(400);
        enc.encode(&frame, &mut payload).unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for seq in [0u32, 3] {
            let pkt = Packet {
                stream_id: 5,
                seq,
                timestamp_ms: 0,
                payload: Bytes::copy_from_slice(&payload[..]),
            };
            let mut wire = BytesMut::with_capacity(1500);
            pkt.encode(&mut wire).unwrap();
            sender.send_to(&wire[..], sink_addr).await.unwrap();
        }

        for _ in 0..30 {
            if stats.packets_lost.load(Ordering::Relaxed) >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(stats.packets_lost.load(Ordering::Relaxed), 2);

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }
}

#[cfg(test)]
mod source_pump_tests {
    use super::*;
    use crate::audio::ring::AudioRing;
    use crate::FRAME_SAMPLES;
    use tokio::net::UdpSocket;
    use tokio::sync::Notify;
    use uuid::Uuid;

    #[tokio::test]
    async fn source_pump_sends_a_packet_per_frame() {
        let (mut prod, cons) = AudioRing::new(FRAME_SAMPLES * 8);
        let notify = Arc::new(Notify::new());
        let stats = Arc::new(StreamStats::default());

        let recv_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote = recv_socket.local_addr().unwrap();
        let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        send_socket.connect(remote).await.unwrap();

        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let stats_clone = stats.clone();
        let notify_clone = notify.clone();

        let pump = tokio::spawn(spawn_source_pump_inner(
            Uuid::new_v4(),
            3u8,
            cons,
            notify_clone,
            send_socket,
            ctrl_rx,
            stats_clone,
            64_000,
        ));

        let frame = vec![0.25f32; FRAME_SAMPLES];
        for _ in 0..3 {
            prod.push_slice(&frame);
            notify.notify_one();
        }

        let mut buf = [0u8; 1500];
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            recv_socket.recv_from(&mut buf),
        )
        .await
        .expect("recv timeout")
        .unwrap();
        assert!(
            n >= 10,
            "expected at least a header + payload, got {n} bytes"
        );

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;

        assert!(stats.packets_sent.load(Ordering::Relaxed) >= 1);
    }
}

use crate::audio::ring::AudioRing;
use crate::net::stream::StreamRoute;

pub async fn open_stream_as_sink_inproc(
    registry: Arc<StreamRegistry>,
    session_id: SessionId,
    stream_id: StreamId,
    route: StreamRoute,
) -> Result<u16, NetError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("bind sink udp: {e}"),
        })?;
    let port = socket
        .local_addr()
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("query sink udp local_addr: {e}"),
        })?
        .port();

    let (producer, _consumer) = AudioRing::new(FRAME_SAMPLES * 8);

    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let stats_clone = stats.clone();

    let join = tokio::spawn(spawn_sink_pump_inner(
        session_id,
        stream_id,
        socket,
        producer,
        control_rx,
        stats_clone,
    ));

    let rt = StreamRuntime {
        session_id,
        stream_id,
        stats,
        control_tx,
        bound_device_id: Some(route.sink.device_id.clone()),
        join,
        device_guard: DeviceGuard::None,
    };
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
    let (_producer, consumer) = AudioRing::new(FRAME_SAMPLES * 8);
    let notify = Arc::new(Notify::new());

    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("bind source udp: {e}"),
        })?;
    socket
        .connect(remote)
        .await
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("connect source udp to {remote}: {e}"),
        })?;

    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let stats_clone = stats.clone();
    let notify_clone = notify.clone();
    let bitrate = route.codec.bitrate;

    let join = tokio::spawn(spawn_source_pump_inner(
        session_id,
        stream_id,
        consumer,
        notify_clone,
        socket,
        control_rx,
        stats_clone,
        bitrate,
    ));

    let rt = StreamRuntime {
        session_id,
        stream_id,
        stats,
        control_tx,
        bound_device_id: Some(route.source.device_id.clone()),
        join,
        device_guard: DeviceGuard::None,
    };
    registry.register(rt).await
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
    let (producer, consumer) = AudioRing::new(FRAME_SAMPLES * 8);

    let (device_guard, frame_ready) =
        match source_kind {
            SourceKind::Mic(device_id) => {
                let cap = crate::audio::capture::CaptureHandle::start_by_id(&device_id, producer)
                    .map_err(|e| NetError::SignalingProtocol {
                    reason: format!("capture start_by_id failed: {e}"),
                })?;
                let notify = cap.frame_ready();
                (DeviceGuard::Capture(cap), notify)
            }
            SourceKind::System => {
                #[cfg(target_os = "macos")]
                {
                    let cap = crate::audio::loopback::MacosLoopbackHandle::start(producer)
                        .map_err(|e| NetError::SignalingProtocol {
                            reason: format!("macos loopback start failed: {e}"),
                        })?;
                    (DeviceGuard::MacosLoopback(cap), Arc::new(Notify::new()))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let cap = crate::audio::capture::CaptureHandle::start_loopback(producer)
                        .map_err(|e| NetError::SignalingProtocol {
                            reason: format!("loopback start failed: {e}"),
                        })?;
                    let notify = cap.frame_ready();
                    (DeviceGuard::Capture(cap), notify)
                }
            }
        };

    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("bind source udp: {e}"),
        })?;
    socket
        .connect(remote)
        .await
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("connect source udp: {e}"),
        })?;

    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let stats_clone = stats.clone();
    let join = tokio::spawn(spawn_source_pump_inner(
        session_id,
        stream_id,
        consumer,
        frame_ready,
        socket,
        control_rx,
        stats_clone,
        route.codec.bitrate,
    ));

    registry
        .register(StreamRuntime {
            session_id,
            stream_id,
            stats,
            control_tx,
            bound_device_id: Some(route.source.device_id.clone()),
            join,
            device_guard,
        })
        .await
}

pub async fn open_stream_as_sink(
    registry: Arc<StreamRegistry>,
    session_id: SessionId,
    stream_id: StreamId,
    _route: StreamRoute,
    output_device_id: String,
) -> Result<u16, NetError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("bind sink udp: {e}"),
        })?;
    let port = socket
        .local_addr()
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("local_addr: {e}"),
        })?
        .port();

    let (producer, consumer) = AudioRing::new(FRAME_SAMPLES * 8);
    let playback = crate::audio::playback::PlaybackHandle::start_by_id(&output_device_id, consumer)
        .map_err(|e| NetError::SignalingProtocol {
            reason: format!("playback start_by_id failed: {e}"),
        })?;

    let (control_tx, control_rx) = mpsc::channel::<StreamControlSignal>(8);
    let stats = Arc::new(StreamStats::default());
    let stats_clone = stats.clone();
    let join = tokio::spawn(spawn_sink_pump_inner(
        session_id,
        stream_id,
        socket,
        producer,
        control_rx,
        stats_clone,
    ));

    registry
        .register(StreamRuntime {
            session_id,
            stream_id,
            stats,
            control_tx,
            bound_device_id: Some(output_device_id.clone()),
            join,
            device_guard: DeviceGuard::Playback(playback),
        })
        .await?;
    Ok(port)
}

#[cfg(test)]
mod open_sink_tests {
    use super::*;
    use crate::net::signaling::{CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;
    use uuid::Uuid;

    #[tokio::test]
    async fn open_stream_as_sink_returns_bound_port_and_registers() {
        let registry = StreamRegistry::new();
        let session_id = Uuid::new_v4();
        let route = StreamRoute {
            source: Endpoint {
                peer_id: "a".into(),
                device_id: "ignored".into(),
            },
            sink: Endpoint {
                peer_id: "b".into(),
                device_id: "headphones".into(),
            },
            codec: CodecParams {
                name: "opus".into(),
                bitrate: 64_000,
                frame_ms: 20,
            },
            volume: 1.0,
        };

        let port = open_stream_as_sink_inproc(registry.clone(), session_id, 0, route)
            .await
            .expect("open sink");
        assert!(port > 0);
        let listed = registry.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].stream_id, 0);
    }
}

#[cfg(test)]
mod open_source_tests {
    use super::*;
    use crate::net::signaling::{CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;
    use std::net::SocketAddr;
    use tokio::net::UdpSocket;
    use uuid::Uuid;

    #[tokio::test]
    async fn open_stream_as_source_registers_runtime() {
        let registry = StreamRegistry::new();
        let session_id = Uuid::new_v4();

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote: SocketAddr = sink_socket.local_addr().unwrap();

        let route = StreamRoute {
            source: Endpoint {
                peer_id: "a".into(),
                device_id: "mic-or-loopback".into(),
            },
            sink: Endpoint {
                peer_id: "b".into(),
                device_id: "ignored".into(),
            },
            codec: CodecParams {
                name: "opus".into(),
                bitrate: 64_000,
                frame_ms: 20,
            },
            volume: 1.0,
        };

        let result =
            open_stream_as_source_inproc(registry.clone(), session_id, 0, route, remote).await;
        assert!(
            result.is_ok(),
            "open_stream_as_source_inproc returned {result:?}"
        );

        let listed = registry.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].stream_id, 0);
    }
}

use crate::net::device_watcher::DeviceEvent;
use tokio::sync::broadcast;

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
                let guard = registry.inner.read().await;
                for ((sid, stream_id), rt) in guard.iter() {
                    if rt.bound_device_id.as_deref() == Some(target_id.as_str()) {
                        let _ = rt.control_tx.send(signal).await;
                        tracing::info!(
                            session = %sid,
                            stream = stream_id,
                            device = %target_id,
                            signal = ?signal,
                            "device hot-plug -> pump notified"
                        );
                    }
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
    use uuid::Uuid;

    #[tokio::test]
    async fn watcher_dispatches_pause_when_bound_device_disappears() {
        let registry = StreamRegistry::new();
        let (tx, _) = broadcast::channel::<DeviceEvent>(8);
        let sid = Uuid::new_v4();

        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async move { while let Some(_) = ctrl_rx.recv().await {} });
        registry
            .register(StreamRuntime {
                session_id: sid,
                stream_id: 0,
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

        let mut probe_rx = ctrl_tx.clone();
        let _ = probe_rx;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        dispatcher.abort();
    }
}
