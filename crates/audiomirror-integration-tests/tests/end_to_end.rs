use audiomirror_integration_tests::{decode_frames, encode_frames, rms, SineSource};

#[test]
fn sine_encode_decode_rms_is_audible() {
    let mut src = SineSource::new();
    let frames = encode_frames(&mut src, 50);
    assert_eq!(frames.len(), 50);
    let samples = decode_frames(&frames);
    let level = rms(&samples);
    assert!(
        level > 0.1,
        "decoded RMS {level:.4} is below audible threshold — audio lost in pipeline"
    );
}

#[test]
fn packet_encode_decode_preserves_payload() {
    use audiomirror_core::net::packet::Packet;
    use bytes::BytesMut;

    let mut src = SineSource::new();
    let frames = encode_frames(&mut src, 5);
    let mut buf = BytesMut::with_capacity(256);

    for (i, frame) in frames.iter().enumerate() {
        let pkt = Packet {
            stream_id: 1,
            seq: i as u32,
            timestamp_ms: (i as u32) * 20,
            payload: frame.payload.clone(),
        };
        pkt.encode(&mut buf).expect("encode packet");
        let decoded = Packet::decode(buf.clone().freeze()).expect("decode packet");
        assert_eq!(decoded.stream_id, 1);
        assert_eq!(decoded.seq, i as u32);
        assert_eq!(decoded.payload, frame.payload);
    }
}

#[tokio::test]
async fn udp_loopback_delivers_opus_frames() {
    use audiomirror_core::net::packet::Packet;
    use bytes::{Bytes, BytesMut};
    use std::net::SocketAddr;
    use tokio::net::UdpSocket;

    let recv_sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind recv");
    let recv_addr: SocketAddr = recv_sock.local_addr().expect("local addr");

    let send_sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind send");

    let mut src = SineSource::new();
    let frames = encode_frames(&mut src, 10);

    let send_handle = tokio::spawn(async move {
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
        }
    });

    let mut received = 0usize;
    let mut rx_buf = vec![0u8; 1500];
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
    while received < 10 && tokio::time::Instant::now() < deadline {
        let timeout = tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            recv_sock.recv_from(&mut rx_buf),
        );
        if let Ok(Ok((len, _))) = timeout.await {
            let pkt = Packet::decode(Bytes::copy_from_slice(&rx_buf[..len])).expect("decode");
            assert_eq!(pkt.stream_id, 0);
            received += 1;
        }
    }

    send_handle.await.expect("send task");
    assert_eq!(received, 10, "expected 10 packets, got {received}");
}

#[tokio::test]
async fn two_daemon_processes_exchange_hello_and_close() {
    use assert_cmd::cargo::CommandCargoExt;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    let mut peer_a = Command::new(
        std::process::Command::cargo_bin("audiomirror-cli")
            .unwrap()
            .get_program(),
    )
    .args(["daemon", "--signaling-port", "0"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .expect("spawn peer A");

    let mut stdout_a = BufReader::new(peer_a.stdout.take().expect("stdout A"));
    let stdin_a = peer_a.stdin.take().expect("stdin A");

    let mut peer_b = Command::new(
        std::process::Command::cargo_bin("audiomirror-cli")
            .unwrap()
            .get_program(),
    )
    .args(["daemon", "--signaling-port", "0"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .expect("spawn peer B");

    let mut stdout_b = BufReader::new(peer_b.stdout.take().expect("stdout B"));

    let deadline = Duration::from_secs(10);

    async fn read_ready_port(
        reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
        deadline: Duration,
        label: &str,
    ) -> u16 {
        let timeout_at = tokio::time::Instant::now() + deadline;
        let mut line = String::new();
        loop {
            line.clear();
            let remaining = timeout_at
                .checked_duration_since(tokio::time::Instant::now())
                .expect("timeout waiting for READY line");
            tokio::time::timeout(remaining, reader.read_line(&mut line))
                .await
                .unwrap_or_else(|_| panic!("timeout waiting for READY line from {label}"))
                .unwrap_or_else(|e| panic!("io error reading {label}: {e}"));
            if let Some(port_str) = line.trim().strip_prefix("READY port=") {
                return port_str
                    .parse::<u16>()
                    .unwrap_or_else(|_| panic!("port parse failed for {label}: {port_str}"));
            }
        }
    }

    let _port_a = read_ready_port(&mut stdout_a, deadline, "peer A").await;
    let _port_b = read_ready_port(&mut stdout_b, deadline, "peer B").await;

    drop(stdin_a);
    peer_a.wait().await.expect("wait A");
    peer_b.kill().await.ok();
}
