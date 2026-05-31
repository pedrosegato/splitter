use bytes::{Bytes, BytesMut};
use splitter_core::audio::codec::OpusEncoder;
use splitter_core::audio::ring::AudioRing;
use splitter_core::net::packet::Packet;
use splitter_core::net::stream_runtime::{
    spawn_sink_pump_inner, spawn_source_pump_inner, StreamControlSignal, StreamStats,
};
use splitter_core::FRAME_STEREO_SAMPLES;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

#[tokio::test]
async fn pcm_round_trip_source_to_sink_over_localhost_udp() {
    let session_id = Uuid::new_v4();
    let stream_id: u8 = 0;

    let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = sink_socket.local_addr().unwrap();
    let send_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    send_socket.connect(sink_addr).await.unwrap();

    let (cap_prod, cap_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 32);
    let (play_prod, play_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 32);

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

    let mut cap_prod_mut = cap_prod;
    let sine: Vec<f32> = (0..FRAME_STEREO_SAMPLES)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * ((i / 2) as f32) / 48_000.0).sin() * 0.5)
        .collect();

    for _ in 0..10 {
        let pushed = cap_prod_mut.push_slice(&sine);
        assert_eq!(pushed, FRAME_STEREO_SAMPLES);
        frame_ready.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }

    for _ in 0..50 {
        if play_cons.occupied() >= FRAME_STEREO_SAMPLES * 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        play_cons.occupied() >= FRAME_STEREO_SAMPLES,
        "sink ring should have received at least one decoded frame, got {} samples",
        play_cons.occupied()
    );
    assert!(src_stats.packets_sent.load(Ordering::Relaxed) >= 1);
    assert!(sink_stats.packets_received.load(Ordering::Relaxed) >= 1);

    let _ = src_tx.send(StreamControlSignal::Close).await;
    let _ = sink_tx.send(StreamControlSignal::Close).await;
    let _ = src_handle.await;
    let _ = sink_handle.await;
}

#[tokio::test]
async fn volume_change_attenuates_decoded_signal() {
    let stream_id: u8 = 1;
    let sink_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = sink_socket.local_addr().unwrap();

    let (play_prod, mut play_cons) = AudioRing::new(FRAME_STEREO_SAMPLES * 16);
    let stats = Arc::new(StreamStats::default());
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
    let handle = tokio::spawn(spawn_sink_pump_inner(
        Uuid::new_v4(),
        stream_id,
        sink_socket,
        play_prod,
        ctrl_rx,
        stats.clone(),
    ));

    let mut enc = OpusEncoder::new(64_000).unwrap();
    let sine: Vec<f32> = (0..FRAME_STEREO_SAMPLES)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * ((i / 2) as f32) / 48_000.0).sin() * 0.5)
        .collect();
    let mut payload = BytesMut::with_capacity(400);
    enc.encode(&sine, &mut payload).unwrap();
    let pkt = Packet {
        stream_id,
        seq: 0,
        timestamp_ms: 0,
        payload: Bytes::copy_from_slice(&payload[..]),
    };
    let mut wire = BytesMut::with_capacity(1500);
    pkt.encode(&mut wire).unwrap();

    let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    ctrl_tx
        .send(StreamControlSignal::SetVolume(0.0))
        .await
        .unwrap();
    sender.send_to(&wire[..], sink_addr).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut out = vec![0.0f32; FRAME_STEREO_SAMPLES];
    let popped = play_cons.pop_slice(&mut out);
    assert_eq!(popped, FRAME_STEREO_SAMPLES);
    let energy: f32 = out.iter().map(|x| x * x).sum();
    assert!(
        energy < 0.01,
        "muted output should be silent, energy was {energy}"
    );

    let _ = ctrl_tx.send(StreamControlSignal::Close).await;
    let _ = handle.await;
}
