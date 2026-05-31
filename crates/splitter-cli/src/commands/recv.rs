use bytes::Bytes;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use splitter_core::audio::codec::OpusDecoder;
use splitter_core::audio::playback::PlaybackHandle;
use splitter_core::audio::ring::{AudioRing, RingProducer};
use splitter_core::net::jitter::{JitterBuffer, JitterOutput};
use splitter_core::net::packet::Packet;
use splitter_core::FRAME_STEREO_SAMPLES;
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::net::UdpSocket;

pub(crate) async fn run_with_settings(
    output: &str,
    bind: &str,
    jitter_mode: splitter_core::JitterMode,
    jitter_max_depth_ms: u32,
) -> anyhow::Result<()> {
    let bind_addr: SocketAddr = SocketAddr::from_str(bind)?;
    let sock = make_udp_socket(bind_addr)?;
    tracing::info!("receiving on {bind_addr}, playing to {output}");

    let (mut producer, consumer) = AudioRing::new(7_680);
    let _playback = PlaybackHandle::start(output, consumer)?;

    let mut decoder = OpusDecoder::new()?;
    let mut udp_buf = vec![0u8; 1500];
    let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
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
                            push_frame_to_ring(&mut producer, &frame);
                        }
                        pending_fec_recover = false;
                    }
                    decoder.decode_with_fec(Some(&p.payload), &mut frame, false)?;
                    push_frame_to_ring(&mut producer, &frame);
                }
            }
        }
    }
}

fn push_frame_to_ring(producer: &mut RingProducer, frame: &[f32]) {
    let _ = producer.push_slice(frame);
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
    use splitter_core::JitterMode;

    /// Compile-time check: run_with_settings must accept (output, bind, JitterMode, u32).
    #[allow(dead_code)]
    fn _assert_signature_compiles() {
        let _fut = run_with_settings("out", "0.0.0.0:0", JitterMode::Auto, 100);
        drop(_fut);
    }

    #[tokio::test]
    async fn recv_signature_accepts_jitter_args() {
        // Verified at compile time by _assert_signature_compiles above.
        let _ = JitterMode::Auto;
    }
}
