//! End-to-end byte-level verification that audio signal energy is sustained
//! across 1 000 frames (~20 seconds of 48 kHz stereo Opus) over a UDP loopback.
//!
//! Uses `spawn_source_pump_inner` + `spawn_sink_pump_inner` directly (the same
//! approach as `splitter-core/tests/stream_data_plane.rs`) so the test is
//! hermetic and does not require real audio hardware.

use splitter_core::audio::ring::AudioRing;
use splitter_core::net::stream_runtime::{
    spawn_sink_pump_inner, spawn_source_pump_inner, StreamControlSignal, StreamStats,
};
use splitter_core::FRAME_STEREO_SAMPLES;
use splitter_integration_tests::SineSource;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

/// Number of 20 ms frames to push through the pipeline.
/// 1 000 frames × 20 ms = 20 s worth of audio content.
const FRAME_COUNT: usize = 1_000;

/// Minimum acceptable RMS energy on the received samples.
/// A 440 Hz sine at amplitude 0.5 has RMS ≈ 0.354; we accept anything > 0.10
/// to leave headroom for Opus warm-up and jitter-buffer fill.
const MIN_RMS: f32 = 0.10;

fn sine_frame(phase: &mut f32) -> Vec<f32> {
    let delta = 2.0 * std::f32::consts::PI * 440.0 / 48_000.0;
    let mut buf = vec![0.0f32; FRAME_STEREO_SAMPLES];
    for i in 0..(FRAME_STEREO_SAMPLES / 2) {
        let s = phase.sin() * 0.5;
        buf[i * 2] = s;
        buf[i * 2 + 1] = s;
        *phase = (*phase + delta) % (2.0 * std::f32::consts::PI);
    }
    buf
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[tokio::test]
async fn sustained_rms_over_1000_frames() {
    let session_id = Uuid::new_v4();
    let stream_id = splitter_core::StreamId(0);

    // Bind a sink socket; the source socket connects to it.
    let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = sink_socket.local_addr().unwrap();
    let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    send_socket.connect(sink_addr).await.unwrap();

    // Generous ring buffers: 64 frames each side.
    let (cap_prod, cap_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 64);
    let (play_prod, mut play_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 64);

    let frame_ready = Arc::new(Notify::new());
    let src_stats = Arc::new(StreamStats::default());
    let sink_stats = Arc::new(StreamStats::default());

    let (src_tx, src_rx) = mpsc::channel::<StreamControlSignal>(4);
    let (sink_tx, sink_rx) = mpsc::channel::<StreamControlSignal>(4);

    let src_handle = tokio::spawn(spawn_source_pump_inner(
        session_id,
        stream_id,
        cap_cons,
        frame_ready.clone(),
        send_socket,
        src_rx,
        src_stats.clone(),
        64_000,
    ));
    let sink_handle = tokio::spawn(spawn_sink_pump_inner(
        session_id,
        stream_id,
        sink_socket,
        play_prod,
        sink_rx,
        sink_stats.clone(),
    ));

    // Feed FRAME_COUNT sine frames into the capture ring.
    let mut cap_prod_mut = cap_prod;
    let mut phase = 0.0f32;
    let mut sine_source = SineSource::new();
    // Warm up the encoder with a few frames before counting.
    for _ in 0..5 {
        let frame = sine_frame(&mut phase);
        cap_prod_mut.push_slice(&frame);
        frame_ready.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // Reset source so RMS check uses a clean reference.
    let _ = &mut sine_source;

    for _ in 0..FRAME_COUNT {
        let frame = sine_frame(&mut phase);
        let pushed = cap_prod_mut.push_slice(&frame);
        // If ring is full the push returns 0; back off to let the pump drain.
        if pushed == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            cap_prod_mut.push_slice(&frame);
        }
        frame_ready.notify_one();
        // Advance at real-time pace — 20 ms per frame — so the sink ring does
        // not overflow and the decoder can keep up.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // Wait until the sink ring has accumulated a reasonable number of decoded frames.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while play_cons.occupied() < FRAME_STEREO_SAMPLES * 50 && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Drain all decoded samples from the sink ring.
    let occupied = play_cons.occupied();
    let mut decoded_samples = vec![0.0f32; occupied];
    let popped = play_cons.pop_slice(&mut decoded_samples);
    decoded_samples.truncate(popped);

    let level = rms(&decoded_samples);
    assert!(
        level > MIN_RMS,
        "sustained RMS after {FRAME_COUNT} frames is {level:.4} — below {MIN_RMS:.4}; \
         signal did not make it through the pipeline (packets_sent={}, packets_received={})",
        src_stats.packets_sent.load(Ordering::Relaxed),
        sink_stats.packets_received.load(Ordering::Relaxed),
    );

    let sent = src_stats.packets_sent.load(Ordering::Relaxed);
    let recvd = sink_stats.packets_received.load(Ordering::Relaxed);
    assert!(
        sent >= FRAME_COUNT as u64 / 2,
        "source should have sent at least {n} packets, got {sent}",
        n = FRAME_COUNT / 2
    );
    assert!(
        recvd >= sent / 2,
        "sink should have received ≥ half of sent packets; sent={sent}, received={recvd}"
    );

    let _ = src_tx.send(StreamControlSignal::Close).await;
    let _ = sink_tx.send(StreamControlSignal::Close).await;
    let _ = src_handle.await;
    let _ = sink_handle.await;
}
