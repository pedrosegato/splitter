use audiomirror_core::audio::codec::OpusDecoder;
use audiomirror_core::audio::playback::PlaybackHandle;
use audiomirror_core::audio::ring::AudioRing;
use audiomirror_core::net::jitter::{JitterBuffer, JitterOutput};
use audiomirror_core::net::packet::Packet;
use audiomirror_core::FRAME_SAMPLES;
use bytes::Bytes;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::net::UdpSocket;

pub(crate) async fn run_with_settings(
    output: &str,
    bind: &str,
    jitter_mode: audiomirror_core::JitterMode,
    jitter_max_depth_ms: u32,
) -> anyhow::Result<()> {
    let bind_addr: SocketAddr = SocketAddr::from_str(bind)?;
    let sock = make_udp_socket(bind_addr)?;
    tracing::info!("receiving on {bind_addr}, playing to {output}");

    let (producer, consumer) = AudioRing::new(9_600);
    let _playback = PlaybackHandle::start(output, consumer)?;
    let producer = std::sync::Arc::new(std::sync::Mutex::new(producer));

    let mut decoder = OpusDecoder::new()?;
    let mut udp_buf = vec![0u8; 1500];
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut jitter = JitterBuffer::new(jitter_mode, jitter_max_depth_ms);
    let mut pending_fec_recover = false;

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
        let now = std::time::Instant::now();
        jitter.push(pkt, now);

        while let Some(out) = jitter.pop_ready(now) {
            match out {
                JitterOutput::Lost { seq } => {
                    pending_fec_recover = true;
                    tracing::debug!(seq, "jitter buffer declared lost");
                }
                JitterOutput::Packet(p) => {
                    if pending_fec_recover {
                        if decoder
                            .decode_with_fec(Some(&p.payload), &mut frame, true)
                            .is_ok()
                        {
                            push_frame_to_ring(&producer, &frame);
                        }
                        pending_fec_recover = false;
                    }
                    decoder.decode_with_fec(Some(&p.payload), &mut frame, false)?;
                    push_frame_to_ring(&producer, &frame);
                }
            }
        }
    }
}

fn push_frame_to_ring(
    producer: &std::sync::Arc<std::sync::Mutex<audiomirror_core::audio::ring::RingProducer>>,
    frame: &[f32],
) {
    if let Ok(mut p) = producer.lock() {
        let _ = p.push_slice(frame);
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

#[cfg(test)]
mod tests {
    use super::*;
    use audiomirror_core::JitterMode;

    /// Compile-time check: run_with_settings must accept (output, bind, JitterMode, u32).
    #[allow(dead_code)]
    fn _assert_signature_compiles() {
        let _ = run_with_settings("out", "0.0.0.0:0", JitterMode::Auto, 100);
    }

    #[tokio::test]
    async fn recv_signature_accepts_jitter_args() {
        // Verified at compile time by _assert_signature_compiles above.
        let _ = JitterMode::Auto;
    }
}
