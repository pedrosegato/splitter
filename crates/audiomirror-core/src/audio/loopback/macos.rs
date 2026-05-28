#![cfg(target_os = "macos")]

use crate::audio::ring::RingProducer;
use crate::error::AudioError;
use crate::FRAME_SAMPLES;
use crate::SAMPLE_RATE;
use screencapturekit::{
    cm::{CMSampleBuffer, CMTime},
    error::SCError,
    shareable_content::SCShareableContent,
    stream::{
        configuration::SCStreamConfiguration, content_filter::SCContentFilter,
        output_trait::SCStreamOutputTrait, output_type::SCStreamOutputType, sc_stream::SCStream,
    },
};
use std::mem::size_of;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub struct MacosLoopbackHandle {
    stream: SCStream,
    frame_notify: Arc<Notify>,
}

impl std::fmt::Debug for MacosLoopbackHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacosLoopbackHandle")
            .finish_non_exhaustive()
    }
}

impl Drop for MacosLoopbackHandle {
    fn drop(&mut self) {
        if let Err(e) = self.stream.stop_capture() {
            tracing::error!("MacosLoopbackHandle: stop_capture failed: {e}");
        }
    }
}

impl MacosLoopbackHandle {
    pub fn frame_ready(&self) -> Arc<Notify> {
        self.frame_notify.clone()
    }
}

struct AudioHandler {
    producer: Arc<Mutex<RingProducer>>,
    frame_notify: Arc<Notify>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample_buffer: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type != SCStreamOutputType::Audio {
            return;
        }

        let Some(abl) = sample_buffer.audio_buffer_list() else {
            return;
        };

        let mut mono_buf = [0f32; 4096];
        let mut mono_len = 0usize;

        // SCK with channel_count=2 emits interleaved single-buffer ABL; planar would mis-downmix here.
        for buf in abl.iter() {
            let channels = buf.number_channels as usize;
            if channels == 0 {
                continue;
            }
            let bytes = buf.data();
            if bytes.len() % (size_of::<f32>() * channels) != 0 {
                continue;
            }
            let frame_count = bytes.len() / (size_of::<f32>() * channels);

            // SAFETY: CoreAudio always allocates AudioBuffer.mData on ≥4-byte boundaries for
            // Float32 LPCM (CoreAudioTypes.h); SCK guarantees Float32 LPCM for audio output.
            let samples: &[f32] = unsafe {
                std::slice::from_raw_parts(bytes.as_ptr().cast::<f32>(), frame_count * channels)
            };

            let available = mono_buf.len() - mono_len;
            let frames_to_copy = frame_count.min(available);

            if channels == 1 {
                mono_buf[mono_len..mono_len + frames_to_copy]
                    .copy_from_slice(&samples[..frames_to_copy]);
                mono_len += frames_to_copy;
            } else {
                for (i, chunk) in samples
                    .chunks_exact(channels)
                    .take(frames_to_copy)
                    .enumerate()
                {
                    let sum: f32 = chunk.iter().sum();
                    mono_buf[mono_len + i] = sum / channels as f32;
                }
                mono_len += frames_to_copy;
            }
        }

        if mono_len == 0 {
            return;
        }

        if let Ok(mut p) = self.producer.try_lock() {
            let pushed = p.push_slice(&mono_buf[..mono_len]);
            if pushed >= FRAME_SAMPLES {
                self.frame_notify.notify_one();
            }
        }
    }
}

fn is_permission_error(e: &SCError) -> bool {
    match e {
        SCError::PermissionDenied(_) | SCError::NoShareableContent(_) => true,
        SCError::SCStreamError { code, .. } => {
            matches!(
                code,
                screencapturekit::error::SCStreamErrorCode::UserDeclined
                    | screencapturekit::error::SCStreamErrorCode::MissingEntitlements
            )
        }
        _ => {
            let msg = e.to_string().to_lowercase();
            msg.contains("permission")
                || msg.contains("declined")
                || msg.contains("tcc")
                || msg.contains("entitlement")
        }
    }
}

fn map_sck_error(e: SCError) -> AudioError {
    if is_permission_error(&e) {
        AudioError::ScreenRecordingPermissionDenied
    } else {
        AudioError::BuildStream {
            source: Box::new(e),
        }
    }
}

impl MacosLoopbackHandle {
    pub fn start(producer: RingProducer) -> Result<Self, AudioError> {
        Self::start_with_notify(producer, Arc::new(Notify::new()))
    }

    pub fn start_with_notify(
        producer: RingProducer,
        frame_notify: Arc<Notify>,
    ) -> Result<Self, AudioError> {
        let content = SCShareableContent::get().map_err(map_sck_error)?;

        let displays = content.displays();
        if displays.is_empty() {
            return Err(AudioError::BuildStream {
                source: Box::new(std::io::Error::other(
                    "ScreenCaptureKit: no displays available",
                )),
            });
        }
        let display = &displays[0];

        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build();

        // Low-latency tuning per SCK BENCHMARKS.md: minimal video overhead (2x2 @ 1fps)
        // since SCK always co-delivers video frames; queue_depth=3 cuts buffer-induced lag.
        let one_fps = CMTime::new(1, 1);
        let config = SCStreamConfiguration::new()
            .with_captures_audio(true)
            .with_sample_rate(SAMPLE_RATE as i32)
            .with_channel_count(1i32)
            .with_excludes_current_process_audio(true)
            .with_width(2)
            .with_height(2)
            .with_minimum_frame_interval(&one_fps)
            .with_queue_depth(3);

        let handler = AudioHandler {
            producer: Arc::new(Mutex::new(producer)),
            frame_notify: frame_notify.clone(),
        };

        let mut stream = SCStream::new(&filter, &config);
        stream.add_output_handler(handler, SCStreamOutputType::Audio);
        stream.start_capture().map_err(map_sck_error)?;

        Ok(Self {
            stream,
            frame_notify,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::AudioRing;

    #[test]
    fn start_returns_handle_or_known_error() {
        let (prod, _cons) = AudioRing::new(48_000);
        let res = MacosLoopbackHandle::start(prod);
        match res {
            Ok(_) => {}
            Err(AudioError::ScreenRecordingPermissionDenied) => {}
            Err(AudioError::BuildStream { .. }) => {}
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }
}
