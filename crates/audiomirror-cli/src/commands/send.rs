use crate::Source;
use audiomirror_core::audio::capture::CaptureHandle;
use audiomirror_core::audio::codec::OpusEncoder;
use audiomirror_core::audio::ring::AudioRing;
use audiomirror_core::net::packet::Packet;
use audiomirror_core::FRAME_SAMPLES;
use bytes::{Bytes, BytesMut};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::time::Duration;

#[cfg(target_os = "macos")]
use audiomirror_core::MacosLoopbackHandle;

#[allow(dead_code)]
enum CaptureGuard {
    Mic(CaptureHandle),
    #[cfg(target_os = "macos")]
    MacSystem(MacosLoopbackHandle),
    #[cfg(not(target_os = "macos"))]
    Loopback(CaptureHandle),
}

impl CaptureGuard {
    fn frame_ready(&self) -> Arc<Notify> {
        match self {
            CaptureGuard::Mic(h) => h.frame_ready(),
            #[cfg(target_os = "macos")]
            CaptureGuard::MacSystem(h) => h.frame_ready(),
            #[cfg(not(target_os = "macos"))]
            CaptureGuard::Loopback(h) => h.frame_ready(),
        }
    }
}

pub(crate) async fn run(
    input: &str,
    addr: &str,
    stream_id: u8,
    bitrate: i32,
    source: Source,
) -> anyhow::Result<()> {
    let dest: SocketAddr = SocketAddr::from_str(addr)?;
    let (producer, mut consumer) = AudioRing::new(9_600);
    let _capture: CaptureGuard = match source {
        Source::Mic => CaptureGuard::Mic(CaptureHandle::start(input, producer)?),
        Source::System => {
            #[cfg(target_os = "macos")]
            {
                CaptureGuard::MacSystem(MacosLoopbackHandle::start(producer)?)
            }
            #[cfg(not(target_os = "macos"))]
            {
                CaptureGuard::Loopback(CaptureHandle::start_loopback(producer)?)
            }
        }
    };

    let frame_notify = _capture.frame_ready();
    let sock = make_udp_socket(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    tracing::info!("sending stream_id={stream_id} to {dest} at {bitrate} bps");

    let mut encoder = OpusEncoder::new(bitrate)?;
    let mut payload_buf = BytesMut::with_capacity(400);
    let mut out_buf = BytesMut::with_capacity(512);
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let start = Instant::now();
    let mut seq: u32 = 0;

    loop {
        tokio::select! {
            _ = frame_notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                tracing::warn!("no audio frame signal in 50ms — capture stalled?");
            }
        }
        while consumer.occupied() >= FRAME_SAMPLES {
            let popped = consumer.pop_slice(&mut frame);
            debug_assert_eq!(
                popped, FRAME_SAMPLES,
                "ring SPSC invariant: occupied check passed but pop_slice returned less"
            );
            if popped < FRAME_SAMPLES {
                continue;
            }
            encoder.encode(&frame, &mut payload_buf)?;
            let pkt = Packet {
                stream_id,
                seq: seq & 0xFF_FFFF,
                timestamp_ms: start.elapsed().as_millis() as u32,
                payload: Bytes::copy_from_slice(&payload_buf),
            };
            pkt.encode(&mut out_buf)?;
            sock.send_to(&out_buf, dest).await?;
            seq = seq.wrapping_add(1);
        }
    }
}

fn make_udp_socket(bind: SocketAddr) -> anyhow::Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_send_buffer_size(1 << 20)?;
    sock.bind(&SockAddr::from(bind))?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(UdpSocket::from_std(std_sock)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_guard_variants_compile() {
        fn _accept(_g: CaptureGuard) {}
    }
}
