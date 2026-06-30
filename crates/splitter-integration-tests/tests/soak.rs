//! Opt-in 30-minute soak test.
//!
//! Run with:
//!   cargo test --test soak --ignored --release -- --test-threads=1
//!
//! Asserts:
//!   - No panics throughout the run.
//!   - `packets_sent` monotonically increases every 60 s checkpoint.
//!   - `packets_received` stays within 1 % of `packets_sent`.
//!   - No process RSS growth exceeding 1 MB during the run.

use splitter_core::audio::ring::AudioRing;
use splitter_core::net::stream_runtime::{
    spawn_sink_pump_inner, spawn_source_pump_inner, StreamControlSignal, StreamStats,
};
use splitter_core::FRAME_STEREO_SAMPLES;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};

/// Total soak duration — 30 minutes.
const SOAK_DURATION: Duration = Duration::from_secs(30 * 60);

/// How often to check invariants.
const CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Maximum allowed RSS growth in bytes (1 MiB).
const MAX_RSS_GROWTH_BYTES: i64 = 1024 * 1024;

/// Maximum tolerated loss ratio (1 %).
const MAX_LOSS_RATIO: f64 = 0.01;

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

/// Read current process RSS in bytes.  Returns 0 if unavailable.
fn rss_bytes() -> i64 {
    use sysinfo::{ProcessesToUpdate, System};
    let pid = match sysinfo::get_current_pid() {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
    if let Some(proc) = sys.process(pid) {
        proc.memory() as i64
    } else {
        0
    }
}

#[tokio::test]
#[ignore = "30-minute soak; run with: cargo test --test soak --ignored --release -- --test-threads=1"]
async fn soak_30_minutes_no_leak_no_loss() {
    let session_id = splitter_core::SessionId::new();
    let stream_id = splitter_core::StreamId(0);

    let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = sink_socket.local_addr().unwrap();
    let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    send_socket.connect(sink_addr).await.unwrap();

    let (cap_prod, cap_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 64);
    let (play_prod, _play_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 64);

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

    // Background feeder task pushes sine frames at real-time pace for the whole soak.
    let frame_ready_feed = frame_ready.clone();
    let feeder = tokio::spawn(async move {
        let mut cap_prod_mut = cap_prod;
        let mut phase = 0.0f32;
        let soak_end = Instant::now() + SOAK_DURATION + Duration::from_secs(5);
        while Instant::now() < soak_end {
            let frame = sine_frame(&mut phase);
            if cap_prod_mut.push_slice(&frame) == 0 {
                // Ring full — back off briefly.
                tokio::time::sleep(Duration::from_millis(5)).await;
                cap_prod_mut.push_slice(&frame);
            }
            frame_ready_feed.notify_one();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let baseline_rss = rss_bytes();
    let mut prev_sent: u64 = 0;
    let mut next_check = Instant::now() + CHECK_INTERVAL;
    let soak_end = Instant::now() + SOAK_DURATION;

    while Instant::now() < soak_end {
        tokio::time::sleep(Duration::from_secs(1)).await;

        if Instant::now() >= next_check {
            let sent = src_stats.packets_sent.load(Ordering::Relaxed);
            let recvd = sink_stats.packets_received.load(Ordering::Relaxed);
            let rss = rss_bytes();

            // packets_sent must be monotonically increasing.
            assert!(
                sent > prev_sent,
                "packets_sent did not increase: was {prev_sent}, now {sent}"
            );

            // Loss must be within tolerance.
            if sent > 0 {
                let loss = sent.saturating_sub(recvd) as f64 / sent as f64;
                assert!(
                    loss <= MAX_LOSS_RATIO,
                    "packet loss {:.2}% exceeds {:.0}% threshold (sent={sent}, recv={recvd})",
                    loss * 100.0,
                    MAX_LOSS_RATIO * 100.0
                );
            }

            // RSS growth must stay below 1 MiB.
            let growth = rss - baseline_rss;
            assert!(
                growth <= MAX_RSS_GROWTH_BYTES,
                "RSS grew by {growth} bytes (>{MAX_RSS_GROWTH_BYTES}) — possible memory leak"
            );

            prev_sent = sent;
            next_check = Instant::now() + CHECK_INTERVAL;
        }
    }

    feeder.abort();
    let _ = src_tx.send(StreamControlSignal::Close).await;
    let _ = sink_tx.send(StreamControlSignal::Close).await;
    let _ = src_handle.await;
    let _ = sink_handle.await;
}
