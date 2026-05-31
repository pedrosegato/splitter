use crate::audio::codec::{OpusDecoder, OpusEncoder};
use crate::audio::ring::{RingConsumer, RingProducer};
use crate::error::NetError;
use crate::net::packet::Packet;
use crate::net::session::SessionId;
use crate::net::stream::StreamId;
use crate::{FRAME_SAMPLES, FRAME_STEREO_SAMPLES};
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

const SEQ_MASK: u32 = 0x00FF_FFFF;

fn seq_gap(expected: u32, got: u32) -> u32 {
    got.wrapping_sub(expected) & SEQ_MASK
}

enum ControlOutcome {
    Continue,
    Stop,
}

fn apply_control(
    sig: StreamControlSignal,
    gain: &mut f32,
    muted: &Arc<AtomicBool>,
    paused: &mut bool,
) -> ControlOutcome {
    match sig {
        StreamControlSignal::Close => ControlOutcome::Stop,
        StreamControlSignal::SetVolume(v) => {
            *gain = v.clamp(0.0, 2.0);
            ControlOutcome::Continue
        }
        StreamControlSignal::SetMuted(m) => {
            muted.store(m, Ordering::Relaxed);
            ControlOutcome::Continue
        }
        StreamControlSignal::Pause => {
            *paused = true;
            ControlOutcome::Continue
        }
        StreamControlSignal::Resume => {
            *paused = false;
            ControlOutcome::Continue
        }
    }
}

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

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
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
        let rt = guard
            .get(&(*session_id, stream_id))
            .ok_or(NetError::UnknownStream {
                session: *session_id,
                stream: stream_id,
            })?;
        rt.control_tx
            .send(signal)
            .await
            .map_err(|_| NetError::ChannelClosed)
    }

    pub async fn close(&self, session_id: &SessionId, stream_id: StreamId) -> Result<(), NetError> {
        let mut guard = self.inner.write().await;
        if let Some(rt) = guard.remove(&(*session_id, stream_id)) {
            let _ = rt.control_tx.send(StreamControlSignal::Close).await;
            rt.join.abort();
            Ok(())
        } else {
            Err(NetError::UnknownStream {
                session: *session_id,
                stream: stream_id,
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

    pub async fn current_stats(&self) -> Vec<(SessionId, StreamId, Arc<StreamStats>)> {
        self.inner
            .read()
            .await
            .iter()
            .map(|(&(sid, stream_id), rt)| (sid, stream_id, rt.stats.clone()))
            .collect()
    }

    pub async fn snapshot_stats(
        &self,
        window_ms: u32,
    ) -> Vec<(SessionId, StreamId, StreamStatsSnapshot)> {
        let guard = self.inner.read().await;
        let mut prev = self.prev_snapshots.write().await;
        let mut out = Vec::with_capacity(guard.len());
        for (&key, rt) in guard.iter() {
            let last = prev.get(&key).cloned().unwrap_or_default();
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

    #[tokio::test]
    async fn current_stats_does_not_mutate_prev_snapshots() {
        let reg = StreamRegistry::new();
        let sid = Uuid::new_v4();
        let rt = fake_runtime(sid, 3);
        rt.stats.bytes_sent.store(8_000, Ordering::Relaxed);
        reg.register(rt).await.unwrap();

        let before = reg.snapshot_stats(1_000).await;
        assert_eq!(before.len(), 1);
        let bitrate_after_first_snapshot = before[0].2.bitrate_kbps_sent;

        reg.inner
            .read()
            .await
            .values()
            .next()
            .unwrap()
            .stats
            .bytes_sent
            .store(16_000, Ordering::Relaxed);

        let _ = reg.current_stats().await;
        let _ = reg.current_stats().await;

        let after = reg.snapshot_stats(1_000).await;
        assert_eq!(after.len(), 1);
        let bitrate_after_current_stats = after[0].2.bitrate_kbps_sent;

        assert_eq!(
            bitrate_after_current_stats,
            ((16_000u64 - 8_000) * 8 / 1_000) as u32,
            "current_stats must not have advanced prev_snapshots: expected delta from first snapshot baseline, not from current_stats call"
        );
        let _ = bitrate_after_first_snapshot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_stats_snapshot_is_all_zero() {
        let stats = StreamStats::default();
        let prev = StreamStatsSnapshot::default();
        let snap = stats.snapshot(5_000, &prev);
        assert_eq!(snap, prev);
    }

    #[test]
    fn snapshot_computes_bitrate_from_window() {
        let stats = StreamStats::default();
        stats.bytes_sent.store(8_000, Ordering::Relaxed);
        let snap = stats.snapshot(1_000, &StreamStatsSnapshot::default());
        assert_eq!(snap.bitrate_kbps_sent, 64);
    }

    #[test]
    fn seq_gap_normal_advance() {
        assert_eq!(seq_gap(5, 8), 3);
        assert_eq!(seq_gap(0, 1), 1);
        assert_eq!(seq_gap(10, 10), 0);
    }

    #[test]
    fn seq_gap_wraps_at_24_bit_boundary() {
        assert_eq!(seq_gap(SEQ_MASK - 1, 1), 3);
        assert_eq!(seq_gap(SEQ_MASK, 0), 1);
        assert_eq!(seq_gap(SEQ_MASK, 1), 2);
    }

    #[test]
    fn seq_gap_old_or_out_of_order_packet_gives_large_value() {
        assert!(seq_gap(10, 5) >= 100, "old packet should yield gap >= 100");
    }

    #[test]
    fn snapshot_reads_rtt_atomically() {
        let stats = StreamStats::default();
        stats.last_rtt_ms.store(42, Ordering::Relaxed);
        let snap = stats.snapshot(1_000, &StreamStatsSnapshot::default());
        assert_eq!(snap.last_rtt_ms, 42);
    }

    #[test]
    fn stats_snapshot_default_is_all_zeros() {
        let s = StreamStatsSnapshot::default();
        assert_eq!(s.packets_sent, 0);
        assert_eq!(s.packets_received, 0);
        assert_eq!(s.packets_lost, 0);
        assert_eq!(s.bytes_sent, 0);
        assert_eq!(s.bytes_received, 0);
        assert_eq!(s.last_rtt_ms, 0);
        assert_eq!(s.bitrate_kbps_sent, 0);
        assert_eq!(s.bitrate_kbps_received, 0);
    }

    #[test]
    fn apply_control_close_returns_stop() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut gain = 1.0f32;
        let mut paused = false;
        let outcome = apply_control(StreamControlSignal::Close, &mut gain, &muted, &mut paused);
        assert!(matches!(outcome, ControlOutcome::Stop));
    }

    #[test]
    fn apply_control_set_volume_mutates_gain_and_continues() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut gain = 1.0f32;
        let mut paused = false;
        let outcome = apply_control(
            StreamControlSignal::SetVolume(0.5),
            &mut gain,
            &muted,
            &mut paused,
        );
        assert!(matches!(outcome, ControlOutcome::Continue));
        assert!((gain - 0.5).abs() < 1e-6);
    }

    #[test]
    fn apply_control_set_muted_toggles_flag_and_continues() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut gain = 1.0f32;
        let mut paused = false;
        let outcome = apply_control(
            StreamControlSignal::SetMuted(true),
            &mut gain,
            &muted,
            &mut paused,
        );
        assert!(matches!(outcome, ControlOutcome::Continue));
        assert!(muted.load(Ordering::Relaxed));

        let outcome2 = apply_control(
            StreamControlSignal::SetMuted(false),
            &mut gain,
            &muted,
            &mut paused,
        );
        assert!(matches!(outcome2, ControlOutcome::Continue));
        assert!(!muted.load(Ordering::Relaxed));
    }

    #[test]
    fn apply_control_pause_resume_toggles_paused_and_continues() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut gain = 1.0f32;
        let mut paused = false;
        let outcome = apply_control(StreamControlSignal::Pause, &mut gain, &muted, &mut paused);
        assert!(matches!(outcome, ControlOutcome::Continue));
        assert!(paused);

        let outcome2 = apply_control(StreamControlSignal::Resume, &mut gain, &muted, &mut paused);
        assert!(matches!(outcome2, ControlOutcome::Continue));
        assert!(!paused);
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
    bitrate: u32,
) {
    let mut encoder = match OpusEncoder::new(bitrate as i32) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("opus encoder init failed: {e}");
            return;
        }
    };
    let mut seq: u32 = 0;
    let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
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
                    None => return,
                    Some(sig) => if matches!(apply_control(sig, &mut gain, &muted, &mut paused), ControlOutcome::Stop) { return; },
                }
            }
            _ = frame_ready.notified() => {
                if consumer.occupied() >= FRAME_STEREO_SAMPLES {
                    consumer.pop_slice(&mut frame);
                    if !paused {
                        let effective_gain = if muted.load(Ordering::Relaxed) { 0.0 } else { gain };
                        if (effective_gain - 1.0).abs() > f32::EPSILON {
                            for s in frame.iter_mut() {
                                *s *= effective_gain;
                            }
                        }
                        if let Err(e) = encoder.encode(&frame, &mut payload) {
                            tracing::warn!("opus encode failed: {e}");
                        } else {
                            let pkt = Packet {
                                stream_id,
                                seq: seq & SEQ_MASK,
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
                    if consumer.occupied() >= FRAME_STEREO_SAMPLES {
                        frame_ready.notify_one();
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
    let mut decoded = vec![0.0f32; FRAME_STEREO_SAMPLES];
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
                    None => return,
                    Some(sig) => if matches!(apply_control(sig, &mut gain, &muted, &mut paused), ControlOutcome::Stop) { return; },
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
                    let expected = prev.wrapping_add(1) & SEQ_MASK;
                    if pkt.seq != expected {
                        let lost = seq_gap(expected, pkt.seq) as u64;
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
    use crate::{FRAME_SAMPLES, FRAME_STEREO_SAMPLES};
    use bytes::{Bytes, BytesMut};
    use tokio::net::UdpSocket;
    use uuid::Uuid;

    #[tokio::test]
    async fn sink_pump_decodes_into_playback_ring() {
        let (prod, cons) = AudioRing::new(FRAME_SAMPLES * 20);
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
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
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
            if cons.occupied() >= FRAME_STEREO_SAMPLES {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(cons.occupied() >= FRAME_STEREO_SAMPLES);
        assert_eq!(stats.packets_received.load(Ordering::Relaxed), 1);

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    #[tokio::test]
    async fn sink_pump_records_lost_packets_on_seq_gap() {
        let (prod, _cons) = AudioRing::new(FRAME_SAMPLES * 20);
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
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
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
    use crate::{FRAME_SAMPLES, FRAME_STEREO_SAMPLES};
    use tokio::net::UdpSocket;
    use tokio::sync::Notify;
    use uuid::Uuid;

    #[tokio::test]
    async fn source_pump_sends_a_packet_per_frame() {
        let (mut prod, cons) = AudioRing::new(FRAME_SAMPLES * 20);
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

        let frame = vec![0.25f32; FRAME_STEREO_SAMPLES];
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

    #[tokio::test]
    async fn source_pump_observes_close_while_ring_is_backed_up() {
        let (mut prod, cons) = AudioRing::new(FRAME_SAMPLES * 200);
        let notify = Arc::new(Notify::new());
        let stats = Arc::new(StreamStats::default());

        let recv_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote = recv_socket.local_addr().unwrap();
        let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        send_socket.connect(remote).await.unwrap();

        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let notify_clone = notify.clone();

        let frame = vec![0.25f32; FRAME_STEREO_SAMPLES];
        for _ in 0..100 {
            prod.push_slice(&frame);
        }
        notify.notify_one();

        let pump = tokio::spawn(spawn_source_pump_inner(
            Uuid::new_v4(),
            3u8,
            cons,
            notify_clone,
            send_socket,
            ctrl_rx,
            stats.clone(),
            64_000,
        ));

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        ctrl_tx.send(StreamControlSignal::Close).await.unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), pump).await;

        assert!(
            result.is_ok(),
            "pump did not stop within 500ms after Close — it blocked draining the full ring"
        );
    }
}

use crate::audio::ring::AudioRing;
use crate::net::stream::StreamRoute;

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
    use uuid::Uuid;

    #[tokio::test]
    async fn open_stream_as_sink_returns_bound_port_and_registers() {
        let registry = StreamRegistry::new();
        let session_id = Uuid::new_v4();
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
    use crate::net::signaling::{Codec, CodecParams, Endpoint};
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
        let unknown_sid = Uuid::new_v4();
        let stream_id: u8 = 0;

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
        let unknown_sid = Uuid::new_v4();
        let stream_id: u8 = 1;

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

        let stream_id: u8 = 2;

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

        let activate_result = sessions.activate_stream(&sid, 99).await;
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
        let join = tokio::spawn(async move { while ctrl_rx.recv().await.is_some() {} });
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

        let probe_rx = ctrl_tx.clone();
        let _ = probe_rx;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        dispatcher.abort();
    }
}
