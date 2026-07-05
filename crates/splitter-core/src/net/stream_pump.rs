use crate::audio::codec::{OpusDecoder, OpusEncoder};
use crate::audio::ring::{RingConsumer, RingProducer};
use crate::net::fec::FecController;
use crate::net::jitter::{JitterBuffer, JitterOutput};
use crate::net::packet::Packet;
use crate::net::session::SessionId;
use crate::net::stream::StreamId;
use crate::net::stream_runtime::StreamControlSignal;
use crate::net::stream_stats::StreamStats;
use crate::settings::{FecMode, JitterMode};
use crate::FRAME_STEREO_SAMPLES;
use bytes::{Bytes, BytesMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::Notify;

const SEQ_MASK: u32 = 0x00FF_FFFF;

#[cfg(test)]
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

    // WHY: default FecMode mirrors Settings::default() (=Always). The source has
    // no live packet-loss feedback in the P2P path, so Auto would never flip on;
    // evaluating here activates negotiated in-band FEC. Loss-feedback wiring is
    // deferred (see plan 016 Maintenance notes).
    const FEC_REEVAL_FRAMES: u32 = 100;
    let mut fec = FecController::new(FecMode::Always, 1, 0, 10);
    let mut frame_count: u32 = 0;
    {
        let setting = fec.evaluate(std::time::Instant::now());
        if let Err(e) = encoder.set_fec(setting.enable, setting.packet_loss_perc) {
            tracing::warn!("initial set_fec failed: {e}");
        }
    }

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
                    frame_count = frame_count.wrapping_add(1);
                    if frame_count.is_multiple_of(FEC_REEVAL_FRAMES) {
                        let setting = fec.evaluate(std::time::Instant::now());
                        if let Err(e) = encoder.set_fec(setting.enable, setting.packet_loss_perc) {
                            tracing::warn!("set_fec failed: {e}");
                        }
                    }
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
                            let encoded = Packet::encode_from_parts(
                                stream_id.get(),
                                seq & SEQ_MASK,
                                start.elapsed().as_millis() as u32,
                                &payload[..],
                                &mut packet_buf,
                            );
                            if encoded.is_ok() {
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
    let mut gain: f32 = 1.0;
    let muted = Arc::new(AtomicBool::new(false));
    let mut paused = false;

    // WHY: defaults mirror Settings::default(); a follow-up plan threads the
    // real SettingsHandle through open_stream_as_* into the pump.
    const MAX_DEPTH_MS: u32 = 200;
    let mut jitter = JitterBuffer::new(JitterMode::Auto, MAX_DEPTH_MS);
    let mut pending_fec_recover = false;

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
                let pkt = match Packet::decode(Bytes::copy_from_slice(&buf[..n])) {
                    Ok(p) if p.stream_id == stream_id.get() => p,
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::warn!("packet decode failed: {e}");
                        continue;
                    }
                };
                stats.packets_received.fetch_add(1, Ordering::Relaxed);
                stats.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
                stats.last_seq_received.store(pkt.seq, Ordering::Relaxed);

                let now = std::time::Instant::now();
                jitter.push(pkt, now);
                while let Some(out) = jitter.pop_ready(now) {
                    match out {
                        JitterOutput::Lost { .. } => {
                            pending_fec_recover = true;
                            stats.packets_lost.fetch_add(1, Ordering::Relaxed);
                        }
                        JitterOutput::Packet(p) => {
                            if pending_fec_recover {
                                if decoder.decode_with_fec(Some(&p.payload[..]), &mut decoded, true).is_ok() {
                                    apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                                }
                                pending_fec_recover = false;
                            }
                            if decoder.decode_with_fec(Some(&p.payload[..]), &mut decoded, false).is_ok() {
                                apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                            }
                        }
                    }
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
mod tests {
    use super::*;

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
        ));

        let mut enc = OpusEncoder::new(64_000).unwrap();
        let frame = vec![0.1f32; FRAME_STEREO_SAMPLES];
        let mut payload = BytesMut::with_capacity(400);
        enc.encode(&frame, &mut payload).unwrap();

        let send_seq = |seq: u32| {
            let payload = payload.clone();
            async move {
                let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
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

        // seq 0 drains immediately; seq 2 leaves seq 1 missing. After the missing
        // slot ages past MAX_DEPTH_MS a later pop_ready (triggered by seq 3) declares it Lost.
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
            SessionId::new(),
            StreamId(3),
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
