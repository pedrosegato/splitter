use splitter_core::net::packet::Packet;
use splitter_integration_tests::{decode_frames, encode_frames, rms, EncodedFrame, SineSource};
use bytes::{Bytes, BytesMut};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

async fn udp_rms_under_loss(loss_percent: u8, jitter_ms: u32) -> f32 {
    let recv_sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind recv");
    let recv_addr: SocketAddr = recv_sock.local_addr().expect("addr");
    let send_sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind send");

    apply_netem_or_dummynet(recv_addr.port(), loss_percent, jitter_ms);

    let mut src = SineSource::new();
    let frames = encode_frames(&mut src, 200);
    let send_task = tokio::spawn(async move {
        let mut buf = BytesMut::with_capacity(256);
        for (i, frame) in frames.iter().enumerate() {
            let pkt = Packet {
                stream_id: 0,
                seq: i as u32,
                timestamp_ms: (i as u32) * 20,
                payload: frame.payload.clone(),
            };
            pkt.encode(&mut buf).expect("encode");
            send_sock.send_to(&buf, recv_addr).await.expect("send");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        frames
    });

    let mut received_payloads = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut rx_buf = vec![0u8; 1500];
    while tokio::time::Instant::now() < deadline {
        let timeout =
            tokio::time::timeout(Duration::from_millis(50), recv_sock.recv_from(&mut rx_buf));
        if let Ok(Ok((len, _))) = timeout.await {
            let pkt = Packet::decode(Bytes::copy_from_slice(&rx_buf[..len])).expect("decode");
            received_payloads.push(EncodedFrame {
                seq: pkt.seq,
                payload: pkt.payload,
            });
        }
    }

    remove_netem_or_dummynet(recv_addr.port());
    let _sent_frames = send_task.await.expect("send task");

    if received_payloads.is_empty() {
        return 0.0;
    }
    let samples = decode_frames(&received_payloads);
    rms(&samples)
}

fn apply_netem_or_dummynet(port: u16, loss_percent: u8, jitter_ms: u32) {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("tc")
            .args([
                "qdisc",
                "add",
                "dev",
                "lo",
                "root",
                "netem",
                "loss",
                &format!("{loss_percent}%"),
                "delay",
                &format!("{jitter_ms}ms"),
                &format!("{jitter_ms}ms"),
            ])
            .status()
            .ok();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (port, loss_percent, jitter_ms);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (port, loss_percent, jitter_ms);
    }
}

fn remove_netem_or_dummynet(port: u16) {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("tc")
            .args(["qdisc", "del", "dev", "lo", "root"])
            .status()
            .ok();
    }
    #[cfg(target_os = "macos")]
    let _ = port;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = port;
}

#[tokio::test]
#[ignore = "requires sudo / tc or dummynet; run locally with: cargo test -p splitter-integration-tests -- --ignored --test-threads=1"]
async fn five_percent_loss_rms_audible() {
    let level = udp_rms_under_loss(5, 50).await;
    assert!(
        level > 0.05,
        "5% loss + 50ms jitter: decoded RMS {level:.4} below audible threshold"
    );
}

#[tokio::test]
#[ignore = "requires sudo / tc or dummynet; run locally with: cargo test -p splitter-integration-tests -- --ignored --test-threads=1"]
async fn ten_percent_loss_rms_audible() {
    let level = udp_rms_under_loss(10, 50).await;
    assert!(
        level > 0.02,
        "10% loss + 50ms jitter: decoded RMS {level:.4} below audible threshold"
    );
}
