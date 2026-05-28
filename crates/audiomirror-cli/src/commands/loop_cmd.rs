use crate::Source;
use audiomirror_core::audio::capture::CaptureHandle;
use audiomirror_core::audio::codec::{OpusDecoder, OpusEncoder};
use audiomirror_core::audio::playback::PlaybackHandle;
use audiomirror_core::audio::ring::AudioRing;
use audiomirror_core::FRAME_SAMPLES;
use bytes::BytesMut;
use std::sync::Arc;
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
    output: &str,
    bitrate: i32,
    source: Source,
) -> anyhow::Result<()> {
    let (cap_prod, mut cap_cons) = AudioRing::new(9_600);
    let (play_prod, play_cons) = AudioRing::new(9_600);
    let play_prod = std::sync::Arc::new(std::sync::Mutex::new(play_prod));

    let _capture: CaptureGuard = match source {
        Source::Mic => CaptureGuard::Mic(CaptureHandle::start(input, cap_prod)?),
        Source::System => {
            #[cfg(target_os = "macos")]
            {
                CaptureGuard::MacSystem(MacosLoopbackHandle::start(cap_prod)?)
            }
            #[cfg(not(target_os = "macos"))]
            {
                CaptureGuard::Loopback(CaptureHandle::start_loopback(cap_prod)?)
            }
        }
    };

    let _playback = PlaybackHandle::start(output, play_cons)?;
    tracing::info!(
        "loopback running: source={source:?} input={input} output={output} @ {bitrate}bps"
    );

    let frame_notify = _capture.frame_ready();

    let mut enc = OpusEncoder::new(bitrate)?;
    let mut dec = OpusDecoder::new()?;
    let mut payload = BytesMut::with_capacity(400);
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut out_frame = vec![0.0f32; FRAME_SAMPLES];

    loop {
        tokio::select! {
            _ = frame_notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                tracing::warn!("no audio frame signal in 50ms — capture stalled?");
            }
        }
        while cap_cons.occupied() >= FRAME_SAMPLES {
            cap_cons.pop_slice(&mut frame);
            enc.encode(&frame, &mut payload)?;
            dec.decode(Some(&payload), &mut out_frame)?;
            if let Ok(mut p) = play_prod.lock() {
                let _ = p.push_slice(&out_frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_guard_variants_compile() {
        fn _accept(_g: CaptureGuard) {}
    }
}
