use crate::commands::audio_pipeline::{map_fec_mode, reeval_fec, start_capture, FEC_REEVAL_FRAMES};
use bytes::BytesMut;
use splitter_core::audio::codec::{OpusDecoder, OpusEncoder};
use splitter_core::audio::playback::PlaybackHandle;
use splitter_core::audio::ring::AudioRing;
use splitter_core::FRAME_STEREO_SAMPLES;
use tokio::time::Duration;

pub(crate) async fn run(
    input: &str,
    output: &str,
    bitrate: i32,
    source: crate::Source,
    fec_mode: crate::SendFecMode,
    simulated_loss_pct: u8,
) -> anyhow::Result<()> {
    let (cap_prod, mut cap_cons) = AudioRing::new(7_680);
    let (mut play_prod, play_cons) = AudioRing::new(7_680);

    let _capture = start_capture(source, input, cap_prod)?;
    let _playback = PlaybackHandle::start(output, play_cons)?;
    tracing::info!(
        "loopback running: source={source:?} input={input} output={output} @ {bitrate}bps fec_mode={fec_mode:?} simulated_loss_pct={simulated_loss_pct}"
    );

    let frame_notify = _capture.frame_ready();

    let core_fec_mode = map_fec_mode(fec_mode);
    let mut fec = splitter_core::net::fec::FecController::new(core_fec_mode, 1, 0, 10);

    let mut enc = OpusEncoder::new(bitrate)?;
    let mut dec = OpusDecoder::new()?;
    let mut payload = BytesMut::with_capacity(400);
    let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
    let mut out_frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
    let mut frame_count: u32 = 0;

    loop {
        tokio::select! {
            _ = frame_notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                tracing::warn!("no audio frame signal in 50ms — capture stalled?");
            }
        }
        while cap_cons.occupied() >= FRAME_STEREO_SAMPLES {
            cap_cons.pop_slice(&mut frame);

            frame_count = frame_count.wrapping_add(1);
            if frame_count.is_multiple_of(FEC_REEVAL_FRAMES) {
                reeval_fec(&mut fec, &mut enc, simulated_loss_pct)?;
            }

            enc.encode(&frame, &mut payload)?;
            dec.decode(Some(&payload), &mut out_frame)?;
            let _ = play_prod.push_slice(&out_frame);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn loop_cmd_uses_shared_capture_guard() {
        use crate::commands::audio_pipeline::CaptureGuard;
        fn _accept(_g: CaptureGuard) {}
    }
}
