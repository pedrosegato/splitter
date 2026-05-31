use crate::commands::audio_pipeline::{
    make_udp_socket, map_fec_mode, reeval_fec, start_capture, UdpDirection, FEC_REEVAL_FRAMES,
};
use bytes::{Bytes, BytesMut};
use splitter_core::audio::codec::OpusEncoder;
use splitter_core::audio::ring::AudioRing;
use splitter_core::net::packet::Packet;
use splitter_core::FRAME_STEREO_SAMPLES;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Instant;
use tokio::time::Duration;

pub(crate) async fn run(
    input: &str,
    addr: &str,
    stream_id: u8,
    bitrate: u32,
    source: crate::Source,
    fec_mode: crate::SendFecMode,
    simulated_loss_pct: u8,
) -> anyhow::Result<()> {
    let dest: SocketAddr = SocketAddr::from_str(addr)?;
    let (producer, mut consumer) = AudioRing::new(7_680);
    let _capture = start_capture(source, input, producer)?;
    let frame_notify = _capture.frame_ready();
    let sock = make_udp_socket(SocketAddr::from(([0, 0, 0, 0], 0)), UdpDirection::Send)?;
    tracing::info!(
        "sending stream_id={stream_id} to {dest} at {bitrate} bps fec_mode={fec_mode:?} simulated_loss_pct={simulated_loss_pct}"
    );

    let core_fec_mode = map_fec_mode(fec_mode);
    let mut fec = splitter_core::net::fec::FecController::new(core_fec_mode, 1, 0, 10);

    let mut encoder = OpusEncoder::new(bitrate as i32)?;
    let mut payload_buf = BytesMut::with_capacity(400);
    let mut out_buf = BytesMut::with_capacity(512);
    let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
    let start = Instant::now();
    let mut seq: u32 = 0;
    let mut frame_count: u32 = 0;

    loop {
        tokio::select! {
            _ = frame_notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                tracing::warn!("no audio frame signal in 50ms — capture stalled?");
            }
        }
        while consumer.occupied() >= FRAME_STEREO_SAMPLES {
            let popped = consumer.pop_slice(&mut frame);
            debug_assert_eq!(
                popped, FRAME_STEREO_SAMPLES,
                "ring SPSC invariant: occupied check passed but pop_slice returned less"
            );
            if popped < FRAME_STEREO_SAMPLES {
                continue;
            }

            frame_count = frame_count.wrapping_add(1);
            if frame_count.is_multiple_of(FEC_REEVAL_FRAMES) {
                reeval_fec(&mut fec, &mut encoder, simulated_loss_pct)?;
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
