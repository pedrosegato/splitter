use audiomirror_core::audio::codec::OpusDecoder;
use audiomirror_core::audio::playback::PlaybackHandle;
use audiomirror_core::audio::ring::AudioRing;
use audiomirror_core::net::packet::Packet;
use audiomirror_core::FRAME_SAMPLES;
use bytes::Bytes;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::net::UdpSocket;

pub(crate) async fn run(output: &str, bind: &str) -> anyhow::Result<()> {
    let bind_addr: SocketAddr = SocketAddr::from_str(bind)?;
    let sock = make_udp_socket(bind_addr)?;
    tracing::info!("receiving on {bind_addr}, playing to {output}");

    let (producer, consumer) = AudioRing::new(9_600);
    let _playback = PlaybackHandle::start(output, consumer)?;
    let producer = std::sync::Arc::new(std::sync::Mutex::new(producer));

    let mut decoder = OpusDecoder::new()?;
    let mut udp_buf = vec![0u8; 1500];
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut dropped_samples: u64 = 0;
    let mut last_drop_log = std::time::Instant::now();

    loop {
        let n = sock.recv(&mut udp_buf).await?;
        let bytes = Bytes::copy_from_slice(&udp_buf[..n]);
        let pkt = match Packet::decode(bytes) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("malformed packet: {e}");
                continue;
            }
        };
        decoder.decode(Some(&pkt.payload), &mut frame)?;
        if let Ok(mut p) = producer.lock() {
            let pushed = p.push_slice(&frame);
            if pushed < frame.len() {
                dropped_samples += (frame.len() - pushed) as u64;
                if last_drop_log.elapsed() >= std::time::Duration::from_secs(1) {
                    tracing::warn!(
                        "playback ring overrun: dropped {dropped_samples} samples in last interval"
                    );
                    dropped_samples = 0;
                    last_drop_log = std::time::Instant::now();
                }
            }
        }
    }
}

fn make_udp_socket(bind: SocketAddr) -> anyhow::Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_recv_buffer_size(1 << 20)?;
    sock.bind(&SockAddr::from(bind))?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(UdpSocket::from_std(std_sock)?)
}
