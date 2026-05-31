use crate::commands::audio_pipeline::{make_udp_socket, UdpDirection};
use bytes::Bytes;
use splitter_core::audio::codec::OpusDecoder;
use splitter_core::audio::playback::PlaybackHandle;
use splitter_core::audio::ring::{AudioRing, RingProducer};
use splitter_core::net::jitter::{JitterBuffer, JitterOutput};
use splitter_core::net::packet::Packet;
use splitter_core::FRAME_STEREO_SAMPLES;
use std::net::SocketAddr;
use std::str::FromStr;

pub(crate) async fn run_with_settings(
    output: &str,
    bind: &str,
    jitter_mode: splitter_core::JitterMode,
    jitter_max_depth_ms: u32,
) -> anyhow::Result<()> {
    let bind_addr: SocketAddr = SocketAddr::from_str(bind)?;
    let sock = make_udp_socket(bind_addr, UdpDirection::Recv)?;
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
                    handle_packet(
                        &mut decoder,
                        &mut producer,
                        &p.payload,
                        &mut pending_fec_recover,
                        &mut frame,
                    );
                }
            }
        }
    }
}

fn handle_packet(
    decoder: &mut OpusDecoder,
    producer: &mut RingProducer,
    payload: &[u8],
    pending_fec_recover: &mut bool,
    frame: &mut [f32],
) {
    // Opus in-band FEC: decode_fec=true recovers the PRIOR lost frame from this packet;
    // decode_fec=false decodes THIS frame — two calls, two distinct frames.
    if *pending_fec_recover {
        if decoder.decode_with_fec(Some(payload), frame, true).is_ok() {
            push_frame_to_ring(producer, frame);
        }
        *pending_fec_recover = false;
    }
    match decoder.decode_with_fec(Some(payload), frame, false) {
        Ok(()) => push_frame_to_ring(producer, frame),
        Err(e) => tracing::warn!("malformed audio payload, skipping frame: {e}"),
    }
}

fn push_frame_to_ring(producer: &mut RingProducer, frame: &[f32]) {
    let _ = producer.push_slice(frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::audio::ring::AudioRing;
    use splitter_core::JitterMode;

    #[allow(dead_code)]
    fn _assert_signature_compiles() {
        let _fut = run_with_settings("out", "0.0.0.0:0", JitterMode::Auto, 100);
        drop(_fut);
    }

    #[tokio::test]
    async fn recv_signature_accepts_jitter_args() {
        let _ = JitterMode::Auto;
    }

    #[test]
    fn handle_packet_with_garbage_payload_does_not_panic_or_return_err() {
        let mut decoder = OpusDecoder::new().unwrap();
        let (mut producer, _consumer) = AudioRing::new(FRAME_STEREO_SAMPLES * 4);
        let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
        let mut pending_fec_recover = false;

        let garbage: &[u8] = b"this is not valid opus data at all \xff\xfe\x00";
        handle_packet(
            &mut decoder,
            &mut producer,
            garbage,
            &mut pending_fec_recover,
            &mut frame,
        );
    }

    #[test]
    fn handle_packet_garbage_leaves_fec_state_cleared() {
        let mut decoder = OpusDecoder::new().unwrap();
        let (mut producer, _consumer) = AudioRing::new(FRAME_STEREO_SAMPLES * 4);
        let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
        let mut pending_fec_recover = true;

        let garbage: &[u8] = b"\xde\xad\xbe\xef";
        handle_packet(
            &mut decoder,
            &mut producer,
            garbage,
            &mut pending_fec_recover,
            &mut frame,
        );

        assert!(
            !pending_fec_recover,
            "FEC recovery flag must be cleared even when the payload is malformed"
        );
    }
}
