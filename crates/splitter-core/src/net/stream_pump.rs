use crate::audio::codec::{OpusDecoder, OpusEncoder};
use crate::audio::ring::{RingConsumer, RingProducer};
use crate::net::crypto::{PacketOpener, PacketSealer, StreamKeys};
use crate::net::fec::FecController;
use crate::net::jitter::{JitterBuffer, JitterOutput};
use crate::net::packet::{Packet, ReceiverReport};
use crate::net::session::SessionId;
use crate::net::stream::StreamId;
use crate::net::stream_runtime::StreamControlSignal;
use crate::net::stream_stats::StreamStats;
use crate::settings::{FecMode, JitterMode, Settings};
use crate::FRAME_STEREO_SAMPLES;
use bytes::{Bytes, BytesMut};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::Notify;

const SEQ_MASK: u32 = 0x00FF_FFFF;

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

#[cfg(test)]
fn seq_gap(expected: u32, got: u32) -> u32 {
    got.wrapping_sub(expected) & SEQ_MASK
}

#[derive(Debug, Clone, Copy)]
pub struct SourceConfig {
    pub bitrate: u32,
    pub fec_mode: FecMode,
    pub fec_on_threshold_pct: u32,
    pub fec_off_threshold_pct: u32,
    pub fec_hysteresis_secs: u32,
    /// Seals audio and requires sealed receiver reports; `None` only for raw local tools.
    pub keys: Option<StreamKeys>,
}

impl SourceConfig {
    pub fn from_settings(settings: &Settings, bitrate: u32, keys: Option<StreamKeys>) -> Self {
        Self {
            bitrate,
            fec_mode: settings.fec_mode,
            fec_on_threshold_pct: settings.fec_on_threshold_pct,
            fec_off_threshold_pct: settings.fec_off_threshold_pct,
            fec_hysteresis_secs: settings.fec_hysteresis_secs,
            keys,
        }
    }
}

impl Default for SourceConfig {
    fn default() -> Self {
        let settings = Settings::default();
        Self::from_settings(&settings, settings.default_bitrate, None)
    }
}

/// The sink drops datagrams from hosts other than `expected_source`, then locks onto the
/// first sender that delivers a valid packet.
#[derive(Debug, Clone, Copy)]
pub struct SinkConfig {
    pub jitter_mode: JitterMode,
    pub jitter_max_depth_ms: u32,
    pub expected_source: Option<IpAddr>,
    /// Requires sealed audio and seals receiver reports; `None` only for raw local tools.
    pub keys: Option<StreamKeys>,
}

impl SinkConfig {
    pub fn from_settings(
        settings: &Settings,
        expected_source: Option<IpAddr>,
        keys: Option<StreamKeys>,
    ) -> Self {
        Self {
            jitter_mode: settings.jitter_mode,
            jitter_max_depth_ms: settings.jitter_max_depth_ms,
            expected_source,
            keys,
        }
    }
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self::from_settings(&Settings::default(), None, None)
    }
}

enum ControlOutcome {
    Continue,
    Stop,
}

/// Per-packet failures would otherwise log 50 times a second.
#[derive(Default)]
struct LogThrottle {
    last: Option<Instant>,
    suppressed: u64,
}

impl LogThrottle {
    const INTERVAL: Duration = Duration::from_secs(1);

    fn allow(&mut self, now: Instant) -> Option<u64> {
        match self.last {
            Some(last) if now.duration_since(last) < Self::INTERVAL => {
                self.suppressed += 1;
                None
            }
            _ => {
                self.last = Some(now);
                Some(std::mem::take(&mut self.suppressed))
            }
        }
    }
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

fn apply_fec(encoder: &mut OpusEncoder, fec: &mut FecController, now: Instant) {
    let setting = fec.evaluate(now);
    if let Err(e) = encoder.set_fec(setting.enable, setting.packet_loss_perc) {
        tracing::warn!("set_fec failed: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn_source_pump_inner(
    _session_id: SessionId,
    stream_id: StreamId,
    mut consumer: RingConsumer,
    frame_ready: Arc<Notify>,
    socket: UdpSocket,
    mut control_rx: mpsc::Receiver<StreamControlSignal>,
    stats: Arc<StreamStats>,
    config: SourceConfig,
) {
    let mut encoder = match OpusEncoder::new(config.bitrate as i32) {
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
    let mut report_buf = [0u8; 64];
    let mut gain: f32 = 1.0;
    let muted = Arc::new(AtomicBool::new(false));
    let mut paused = false;
    let start = Instant::now();
    let mut encode_log = LogThrottle::default();
    let mut send_log = LogThrottle::default();
    let mut media_sealer = config.keys.map(|k| PacketSealer::new(&k.media));
    let mut report_opener = config.keys.map(|k| PacketOpener::new(&k.report));

    let mut fec = FecController::new(
        config.fec_mode,
        config.fec_on_threshold_pct,
        config.fec_off_threshold_pct,
        config.fec_hysteresis_secs,
    );
    apply_fec(&mut encoder, &mut fec, Instant::now());

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
                            if let Some(suppressed) = encode_log.allow(Instant::now()) {
                                tracing::warn!(suppressed, "opus encode failed: {e}");
                            }
                        } else {
                            let wire_seq = seq & SEQ_MASK;
                            let timestamp_ms = start.elapsed().as_millis() as u32;
                            let sealed;
                            let body = match media_sealer.as_mut() {
                                Some(sealer) => {
                                    let aad = Packet::header_aad(stream_id.get(), wire_seq, timestamp_ms);
                                    sealed = sealer.seal(&aad, &payload[..]);
                                    &sealed[..]
                                }
                                None => &payload[..],
                            };
                            let encoded = Packet::encode_from_parts(
                                stream_id.get(),
                                wire_seq,
                                timestamp_ms,
                                body,
                                &mut packet_buf,
                            );
                            if encoded.is_ok() {
                                match socket.send(&packet_buf[..]).await {
                                    Ok(n) => {
                                        stats.packets_sent.fetch_add(1, Ordering::Relaxed);
                                        stats.bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
                                        seq = seq.wrapping_add(1);
                                    }
                                    Err(e) => {
                                        if let Some(suppressed) = send_log.allow(Instant::now()) {
                                            tracing::warn!(suppressed, "udp send failed: {e}");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if consumer.occupied() >= FRAME_STEREO_SAMPLES {
                        frame_ready.notify_one();
                    }
                }
            }
            report = socket.recv(&mut report_buf) => {
                let Ok(n) = report else { continue };
                let report = match report_opener.as_mut() {
                    Some(opener) => ReceiverReport::open(&report_buf[..n], opener),
                    None => ReceiverReport::decode(&report_buf[..n]),
                };
                let Some(report) = report.filter(|r| r.stream_id == stream_id.get()) else {
                    continue;
                };
                let now_ms = start.elapsed().as_millis() as u32;
                let rtt = now_ms
                    .wrapping_sub(report.echo_timestamp_ms)
                    .saturating_sub(report.hold_ms);
                if rtt < 60_000 {
                    stats.last_rtt_ms.store(rtt, Ordering::Relaxed);
                }
                let loss = u32::from(report.loss_pct);
                let now = Instant::now();
                fec.record_report(now, 100 - loss, loss);
                apply_fec(&mut encoder, &mut fec, now);
            }
        }
    }
}

pub async fn spawn_sink_pump_inner(
    _session_id: SessionId,
    stream_id: StreamId,
    socket: UdpSocket,
    mut producer: RingProducer,
    mut control_rx: mpsc::Receiver<StreamControlSignal>,
    stats: Arc<StreamStats>,
    config: SinkConfig,
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
    let mut gain: f32 = 1.0;
    let muted = Arc::new(AtomicBool::new(false));
    let mut paused = false;
    let mut recv_log = LogThrottle::default();
    let mut decode_log = LogThrottle::default();
    let mut auth_log = LogThrottle::default();
    let mut media_opener = config.keys.map(|k| PacketOpener::new(&k.media));
    let mut report_sealer = config.keys.map(|k| PacketSealer::new(&k.report));

    let mut jitter = JitterBuffer::new(config.jitter_mode, config.jitter_max_depth_ms);

    // Playout follows the device draining the ring, not packet arrival. Two frames ride out
    // the coarse (~15.6ms) default timer resolution on Windows.
    const PLAYOUT_TICK_MS: u64 = 10;
    const RING_TARGET_SAMPLES: usize = 2 * FRAME_STEREO_SAMPLES;
    let mut playout = tokio::time::interval(Duration::from_millis(PLAYOUT_TICK_MS));
    playout.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut report_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + REPORT_INTERVAL,
        REPORT_INTERVAL,
    );
    report_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut source: Option<SocketAddr> = None;
    let mut newest: Option<(u32, Instant)> = None;
    let mut reported = (0u64, 0u64);

    loop {
        tokio::select! {
            biased;
            maybe_sig = control_rx.recv() => {
                match maybe_sig {
                    None => return,
                    Some(sig) => if matches!(apply_control(sig, &mut gain, &muted, &mut paused), ControlOutcome::Stop) { return; },
                }
            }
            _ = playout.tick() => {
                let now = Instant::now();
                while producer.occupied() < RING_TARGET_SAMPLES {
                    let Some(out) = jitter.pop_ready(now) else { break };
                    let decoded_ok = match out {
                        JitterOutput::Packet(p) => decoder.decode(Some(&p.payload[..]), &mut decoded).is_ok(),
                        JitterOutput::Lost { .. } => {
                            stats.packets_lost.fetch_add(1, Ordering::Relaxed);
                            // The successor's in-band FEC can rebuild the lost frame; else PLC.
                            match jitter.peek_next() {
                                Some(next) => decoder.decode_with_fec(Some(&next.payload[..]), &mut decoded, true).is_ok(),
                                None => decoder.decode(None, &mut decoded).is_ok(),
                            }
                        }
                    };
                    if decoded_ok {
                        apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                    }
                }
            }
            _ = report_tick.tick() => {
                let Some(addr) = source else { continue };
                let received = stats.packets_received.load(Ordering::Relaxed);
                let lost = stats.packets_lost.load(Ordering::Relaxed);
                let delta_received = received.saturating_sub(reported.0);
                let delta_lost = lost.saturating_sub(reported.1);
                reported = (received, lost);
                let loss_pct = (delta_lost * 100)
                    .checked_div(delta_received + delta_lost)
                    .map_or(0, |pct| pct.min(100) as u8);
                let (echo_timestamp_ms, hold_ms) = newest
                    .map(|(ts, at)| (ts, at.elapsed().as_millis() as u32))
                    .unwrap_or((0, 0));
                let report = ReceiverReport {
                    stream_id: stream_id.get(),
                    loss_pct,
                    echo_timestamp_ms,
                    hold_ms,
                };
                let wire = match report_sealer.as_mut() {
                    Some(sealer) => report.seal(sealer),
                    None => report.encode().to_vec(),
                };
                let _ = socket.send_to(&wire, addr).await;
            }
            recv_res = socket.recv_from(&mut buf) => {
                let (n, from) = match recv_res {
                    Ok(v) => v,
                    Err(e) => {
                        if let Some(suppressed) = recv_log.allow(Instant::now()) {
                            tracing::warn!(suppressed, "udp recv failed: {e}");
                        }
                        continue;
                    }
                };
                let accepted_sender = match source {
                    Some(locked) => locked == from,
                    None => config.expected_source.is_none_or(|ip| ip == from.ip()),
                };
                if !accepted_sender {
                    continue;
                }
                let pkt = match Packet::decode(Bytes::copy_from_slice(&buf[..n])) {
                    Ok(p) if p.stream_id == stream_id.get() => p,
                    Ok(_) => continue,
                    Err(e) => {
                        if let Some(suppressed) = decode_log.allow(Instant::now()) {
                            tracing::warn!(suppressed, "packet decode failed: {e}");
                        }
                        continue;
                    }
                };
                let pkt = match media_opener.as_mut() {
                    None => pkt,
                    Some(opener) => match pkt.open_payload(opener) {
                        Some(pkt) => pkt,
                        None => {
                            if let Some(suppressed) = auth_log.allow(Instant::now()) {
                                tracing::warn!(suppressed, %from, "dropping unauthenticated audio packet");
                            }
                            continue;
                        }
                    },
                };
                source.get_or_insert(from);
                let now = Instant::now();
                stats.packets_received.fetch_add(1, Ordering::Relaxed);
                stats.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
                stats.last_seq_received.store(pkt.seq, Ordering::Relaxed);
                newest = Some((pkt.timestamp_ms, now));
                jitter.push(pkt, now);
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
mod tests {
    use super::*;

    #[test]
    fn log_throttle_reports_first_then_once_per_interval_with_suppressed_count() {
        use std::time::{Duration, Instant};
        let mut t = LogThrottle::default();
        let t0 = Instant::now();
        assert_eq!(t.allow(t0), Some(0));
        assert_eq!(t.allow(t0 + Duration::from_millis(20)), None);
        assert_eq!(t.allow(t0 + Duration::from_millis(40)), None);
        assert_eq!(t.allow(t0 + Duration::from_secs(1)), Some(2));
        assert_eq!(t.allow(t0 + Duration::from_millis(1_020)), None);
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

#[cfg(test)]
mod sink_pump_tests {
    use super::*;
    use crate::audio::codec::OpusEncoder;
    use crate::audio::ring::AudioRing;
    use crate::net::packet::Packet;
    use crate::{FRAME_SAMPLES, FRAME_STEREO_SAMPLES};
    use bytes::{Bytes, BytesMut};
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn sink_pump_decodes_into_playback_ring() {
        let (prod, cons) = AudioRing::new(FRAME_SAMPLES * 20);
        let stats = Arc::new(StreamStats::default());

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink_socket.local_addr().unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);

        let stats_clone = stats.clone();
        let pump = tokio::spawn(spawn_sink_pump_inner(
            SessionId::new(),
            StreamId(5),
            sink_socket,
            prod,
            ctrl_rx,
            stats_clone,
            SinkConfig::default(),
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
    async fn sink_pump_records_lost_packet_after_max_depth() {
        let (prod, _cons) = AudioRing::new(FRAME_SAMPLES * 20);
        let stats = Arc::new(StreamStats::default());

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink_socket.local_addr().unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);

        let stats_clone = stats.clone();
        let pump = tokio::spawn(spawn_sink_pump_inner(
            SessionId::new(),
            StreamId(5),
            sink_socket,
            prod,
            ctrl_rx,
            stats_clone,
            SinkConfig::default(),
        ));

        let mut enc = OpusEncoder::new(64_000).unwrap();
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
        let mut payload = BytesMut::with_capacity(400);
        enc.encode(&frame, &mut payload).unwrap();

        // One socket for the whole stream, like a real source: the sink locks onto it.
        let sender = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let send_seq = |seq: u32| {
            let payload = payload.clone();
            let sender = sender.clone();
            async move {
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
        };

        // seq 2 leaves seq 1 missing; its playout slot declares it Lost.
        send_seq(0).await;
        send_seq(2).await;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        send_seq(3).await;

        for _ in 0..30 {
            if stats.packets_lost.load(Ordering::Relaxed) >= 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(stats.packets_lost.load(Ordering::Relaxed) >= 1);

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    fn opus_wire(enc: &mut OpusEncoder, stream_id: u8, seq: u32, timestamp_ms: u32) -> BytesMut {
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
        let mut payload = BytesMut::with_capacity(400);
        enc.encode(&frame, &mut payload).unwrap();
        let mut wire = BytesMut::with_capacity(1500);
        Packet::encode_from_parts(stream_id, seq, timestamp_ms, &payload[..], &mut wire).unwrap();
        wire
    }

    async fn spawn_sink(
        config: SinkConfig,
    ) -> (
        std::net::SocketAddr,
        Arc<StreamStats>,
        mpsc::Sender<StreamControlSignal>,
        tokio::task::JoinHandle<()>,
    ) {
        let (prod, _cons) = AudioRing::new(FRAME_SAMPLES * 20);
        let stats = Arc::new(StreamStats::default());
        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink_socket.local_addr().unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let pump = tokio::spawn(spawn_sink_pump_inner(
            SessionId::new(),
            StreamId(5),
            sink_socket,
            prod,
            ctrl_rx,
            stats.clone(),
            config,
        ));
        (sink_addr, stats, ctrl_tx, pump)
    }

    #[tokio::test]
    async fn sink_pump_ignores_packets_from_unexpected_hosts() {
        let config = SinkConfig {
            expected_source: Some("192.0.2.1".parse().unwrap()),
            ..SinkConfig::default()
        };
        let (sink_addr, stats, ctrl_tx, pump) = spawn_sink(config).await;
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(&opus_wire(&mut enc, 5, 0, 0)[..], sink_addr)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(stats.packets_received.load(Ordering::Relaxed), 0);
        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    #[tokio::test]
    async fn sink_pump_locks_onto_the_first_sender() {
        let config = SinkConfig {
            expected_source: Some("127.0.0.1".parse().unwrap()),
            ..SinkConfig::default()
        };
        let (sink_addr, stats, ctrl_tx, pump) = spawn_sink(config).await;
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let intruder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        first
            .send_to(&opus_wire(&mut enc, 5, 0, 0)[..], sink_addr)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        intruder
            .send_to(&opus_wire(&mut enc, 5, 1, 20)[..], sink_addr)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(stats.packets_received.load(Ordering::Relaxed), 1);
        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    #[tokio::test]
    async fn sink_pump_sends_receiver_reports_back_to_the_source() {
        let (sink_addr, _stats, ctrl_tx, pump) = spawn_sink(SinkConfig::default()).await;
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let source = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for (seq, ts) in [(0u32, 1_234u32), (2, 1_274)] {
            source
                .send_to(&opus_wire(&mut enc, 5, seq, ts)[..], sink_addr)
                .await
                .unwrap();
        }

        let mut buf = [0u8; 64];
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            source.recv_from(&mut buf),
        )
        .await
        .expect("no receiver report within 3s")
        .unwrap();
        let report = ReceiverReport::decode(&buf[..n]).expect("datagram must be a receiver report");
        assert_eq!(report.stream_id, 5);
        assert_eq!(
            report.echo_timestamp_ms, 1_274,
            "echoes the newest packet's timestamp"
        );
        assert!(report.loss_pct > 0, "seq 1 never arrived");

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    fn sealed_wire(
        enc: &mut OpusEncoder,
        sealer: &mut PacketSealer,
        seq: u32,
        timestamp_ms: u32,
    ) -> BytesMut {
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
        let mut payload = BytesMut::with_capacity(400);
        enc.encode(&frame, &mut payload).unwrap();
        let sealed = sealer.seal(&Packet::header_aad(5, seq, timestamp_ms), &payload[..]);
        let mut wire = BytesMut::with_capacity(1500);
        Packet::encode_from_parts(5, seq, timestamp_ms, &sealed, &mut wire).unwrap();
        wire
    }

    #[tokio::test]
    async fn keyed_sink_plays_only_sealed_audio_and_seals_its_reports() {
        let keys = StreamKeys {
            media: [1u8; 32],
            report: [2u8; 32],
        };
        let config = SinkConfig {
            keys: Some(keys),
            ..SinkConfig::default()
        };
        let (sink_addr, stats, ctrl_tx, pump) = spawn_sink(config).await;
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let intruder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let source = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let mut wrong_key = PacketSealer::new(&[9u8; 32]);
        for wire in [
            opus_wire(&mut enc, 5, 0, 0),
            sealed_wire(&mut enc, &mut wrong_key, 1, 20),
        ] {
            intruder.send_to(&wire[..], sink_addr).await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(stats.packets_received.load(Ordering::Relaxed), 0);

        let mut sealer = PacketSealer::new(&keys.media);
        for (seq, ts) in [(0u32, 1_000u32), (1, 1_020)] {
            source
                .send_to(&sealed_wire(&mut enc, &mut sealer, seq, ts)[..], sink_addr)
                .await
                .unwrap();
        }

        let mut buf = [0u8; 64];
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            source.recv_from(&mut buf),
        )
        .await
        .expect("the sink locks onto the authenticated sender and reports to it")
        .unwrap();
        assert_eq!(
            ReceiverReport::decode(&buf[..n]),
            None,
            "reports travel sealed"
        );
        let report = ReceiverReport::open(&buf[..n], &mut PacketOpener::new(&keys.report))
            .expect("sealed with the report key");
        assert_eq!(report.echo_timestamp_ms, 1_020);
        assert_eq!(stats.packets_received.load(Ordering::Relaxed), 2);

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    #[tokio::test]
    async fn sink_pump_conceals_single_loss_without_stalling() {
        let (prod, cons) = AudioRing::new(FRAME_SAMPLES * 20);
        let stats = Arc::new(StreamStats::default());

        let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink_socket.local_addr().unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);

        let pump = tokio::spawn(spawn_sink_pump_inner(
            SessionId::new(),
            StreamId(5),
            sink_socket,
            prod,
            ctrl_rx,
            stats.clone(),
            SinkConfig::default(),
        ));

        let mut enc = OpusEncoder::new(64_000).unwrap();
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for seq in [0u32, 2, 3] {
            let mut payload = BytesMut::with_capacity(400);
            enc.encode(&frame, &mut payload).unwrap();
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

        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(stats.packets_lost.load(Ordering::Relaxed), 1);
        assert!(
            cons.occupied() >= 2 * FRAME_STEREO_SAMPLES,
            "seq 0 plus the concealed seq 1 must already be in the playback ring"
        );

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
            SessionId::new(),
            StreamId(3),
            cons,
            notify_clone,
            send_socket,
            ctrl_rx,
            stats_clone,
            SourceConfig::default(),
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
    async fn source_pump_measures_rtt_from_receiver_report() {
        let (mut prod, cons) = AudioRing::new(FRAME_SAMPLES * 20);
        let notify = Arc::new(Notify::new());
        let stats = Arc::new(StreamStats::default());

        let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        send_socket
            .connect(sink.local_addr().unwrap())
            .await
            .unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let pump = tokio::spawn(spawn_source_pump_inner(
            SessionId::new(),
            StreamId(3),
            cons,
            notify.clone(),
            send_socket,
            ctrl_rx,
            stats.clone(),
            SourceConfig::default(),
        ));

        prod.push_slice(&vec![0.25f32; FRAME_STEREO_SAMPLES]);
        notify.notify_one();
        let mut buf = [0u8; 1500];
        let (n, source_addr) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sink.recv_from(&mut buf),
        )
        .await
        .expect("no packet from source")
        .unwrap();
        let sent_ts = Packet::decode_ref(&buf[..n]).unwrap().timestamp_ms;

        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let report = ReceiverReport {
            stream_id: 3,
            loss_pct: 0,
            echo_timestamp_ms: sent_ts,
            hold_ms: 0,
        };
        sink.send_to(&report.encode(), source_addr).await.unwrap();

        let rtt = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let rtt = stats.last_rtt_ms.load(Ordering::Relaxed);
                if rtt > 0 {
                    return rtt;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("source never derived an RTT from the report");
        assert!((25..1_000).contains(&rtt), "rtt was {rtt}ms");

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
    }

    #[tokio::test]
    async fn keyed_source_seals_audio_and_ignores_plaintext_reports() {
        let keys = StreamKeys {
            media: [1u8; 32],
            report: [2u8; 32],
        };
        let (mut prod, cons) = AudioRing::new(FRAME_SAMPLES * 20);
        let notify = Arc::new(Notify::new());
        let stats = Arc::new(StreamStats::default());
        let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        send_socket
            .connect(sink.local_addr().unwrap())
            .await
            .unwrap();
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let pump = tokio::spawn(spawn_source_pump_inner(
            SessionId::new(),
            StreamId(3),
            cons,
            notify.clone(),
            send_socket,
            ctrl_rx,
            stats.clone(),
            SourceConfig {
                keys: Some(keys),
                ..SourceConfig::default()
            },
        ));

        prod.push_slice(&vec![0.25f32; FRAME_STEREO_SAMPLES]);
        notify.notify_one();
        let mut buf = [0u8; 1500];
        let (n, source_addr) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sink.recv_from(&mut buf),
        )
        .await
        .expect("no packet from source")
        .unwrap();
        let pkt = Packet::decode(Bytes::copy_from_slice(&buf[..n])).unwrap();
        let sent_ts = pkt.timestamp_ms;
        let opened = pkt
            .open_payload(&mut PacketOpener::new(&keys.media))
            .expect("audio must be sealed with the media key");
        assert!(!opened.payload.is_empty());

        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let report = ReceiverReport {
            stream_id: 3,
            loss_pct: 0,
            echo_timestamp_ms: sent_ts,
            hold_ms: 0,
        };
        sink.send_to(&report.encode(), source_addr).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            stats.last_rtt_ms.load(Ordering::Relaxed),
            0,
            "a plaintext report must be ignored"
        );

        let sealed = report.seal(&mut PacketSealer::new(&keys.report));
        sink.send_to(&sealed, source_addr).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while stats.last_rtt_ms.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("a sealed report must be used");

        let _ = ctrl_tx.send(StreamControlSignal::Close).await;
        let _ = pump.await;
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
            SessionId::new(),
            StreamId(3),
            cons,
            notify_clone,
            send_socket,
            ctrl_rx,
            stats.clone(),
            SourceConfig::default(),
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
