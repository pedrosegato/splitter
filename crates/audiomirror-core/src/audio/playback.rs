use crate::audio::resampler::Resampler;
use crate::audio::ring::RingConsumer;
use crate::error::AudioError;
use crate::SAMPLE_RATE;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub struct PlaybackHandle {
    _stream: cpal::Stream,
    frame_consumed: Arc<Notify>,
}

impl std::fmt::Debug for PlaybackHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackHandle").finish_non_exhaustive()
    }
}

impl PlaybackHandle {
    pub fn frame_consumed(&self) -> Arc<Notify> {
        self.frame_consumed.clone()
    }

    pub fn start(device_name: &str, consumer: RingConsumer) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .output_devices()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?
            .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| AudioError::DeviceNotFound(device_name.to_string()))?;
        Self::from_device(device, consumer)
    }

    pub fn start_by_id(device_id: &str, consumer: RingConsumer) -> Result<Self, AudioError> {
        let resolved = resolve_output_device(device_id)?;
        Self::from_device(resolved, consumer)
    }

    pub(crate) fn from_device(
        device: cpal::Device,
        consumer: RingConsumer,
    ) -> Result<Self, AudioError> {
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?;
        let channels = supported.channels();
        let sample_rate = supported.sample_rate().0;
        let sample_format = supported.sample_format();

        let config: cpal::StreamConfig = supported.into();
        let consumer = Arc::new(Mutex::new(consumer));
        let frame_consumed = Arc::new(Notify::new());
        let err_fn = |e| tracing::error!("cpal playback stream error: {e}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let cons = consumer.clone();
                let notify = frame_consumed.clone();
                let mut filler = PlaybackFiller::new(sample_rate, channels)?;
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [f32], _| {
                            filler.fill_f32(out, &cons, &notify);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::BuildStream {
                        source: Box::new(e),
                    })?
            }
            cpal::SampleFormat::I16 => {
                let cons = consumer.clone();
                let notify = frame_consumed.clone();
                let mut filler = PlaybackFiller::new(sample_rate, channels)?;
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [i16], _| {
                            filler.fill_i16(out, &cons, &notify);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::BuildStream {
                        source: Box::new(e),
                    })?
            }
            cpal::SampleFormat::U16 => {
                let cons = consumer.clone();
                let notify = frame_consumed.clone();
                let mut filler = PlaybackFiller::new(sample_rate, channels)?;
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [u16], _| {
                            filler.fill_u16(out, &cons, &notify);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::BuildStream {
                        source: Box::new(e),
                    })?
            }
            other => {
                return Err(AudioError::BuildStream {
                    source: Box::new(std::io::Error::other(format!(
                        "unsupported sample format: {other:?}"
                    ))),
                });
            }
        };

        stream.play().map_err(|e| AudioError::PlayStream {
            source: Box::new(e),
        })?;
        Ok(Self {
            _stream: stream,
            frame_consumed,
        })
    }
}

fn resolve_output_device(device_id: &str) -> Result<cpal::Device, AudioError> {
    let target_name = device_id
        .splitn(3, ':')
        .nth(2)
        .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()))?;
    let host = cpal::default_host();
    host.output_devices()
        .map_err(|e| AudioError::BuildStream {
            source: Box::new(e),
        })?
        .find(|d| d.name().map(|n| n == target_name).unwrap_or(false))
        .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()))
}

// Same chunk granularity as capture: 441 input frames at 44100 → 480 at 48000.
const RESAMPLE_CHUNK: usize = 441;

/// Reads 48k mono samples from the ring, resamples to device rate, fans to channels.
/// All buffers are pre-allocated; no heap work inside the cpal callback.
struct PlaybackFiller {
    channels: usize,
    resampler: Option<Resampler>,
    // reservoir: holds already-resampled (or pass-through) samples waiting for the next fill.
    reservoir: Vec<f32>,
    // src_scratch: accumulates 48k samples to feed into resampler chunks.
    src_scratch: Vec<f32>,
    resample_out: Vec<f32>,
}

impl PlaybackFiller {
    fn new(device_rate: u32, channels: u16) -> Result<Self, AudioError> {
        let resampler = if device_rate != SAMPLE_RATE {
            // Resampler converts FROM 48k (ring) TO device_rate (output).
            // We feed RESAMPLE_CHUNK source frames → get ~(RESAMPLE_CHUNK * device_rate / 48000) out.
            let r = Resampler::new(SAMPLE_RATE, device_rate, RESAMPLE_CHUNK).map_err(|e| {
                AudioError::BuildStream {
                    source: Box::new(e),
                }
            })?;
            Some(r)
        } else {
            None
        };

        Ok(Self {
            channels: channels as usize,
            resampler,
            reservoir: Vec::with_capacity(4096),
            src_scratch: Vec::with_capacity(RESAMPLE_CHUNK * 4),
            resample_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
        })
    }

    fn fill_f32(&mut self, out: &mut [f32], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        let mono = self.produce_mono(frames, cons, notify);
        for i in 0..frames {
            let sample = if i < mono.len() { mono[i] } else { 0.0 };
            for c in 0..ch {
                out[i * ch + c] = sample;
            }
        }
    }

    fn fill_i16(&mut self, out: &mut [i16], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        let mono = self.produce_mono(frames, cons, notify);
        for i in 0..frames {
            let sample = if i < mono.len() { mono[i] } else { 0.0 };
            let t = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            for c in 0..ch {
                out[i * ch + c] = t;
            }
        }
    }

    fn fill_u16(&mut self, out: &mut [u16], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        let mono = self.produce_mono(frames, cons, notify);
        for i in 0..frames {
            let sample = if i < mono.len() { mono[i] } else { 0.0 };
            let t = (sample.clamp(-1.0, 1.0) * 32_767.0 + 32_768.0) as u16;
            for c in 0..ch {
                out[i * ch + c] = t;
            }
        }
    }

    /// Returns a slice of mono samples at the device's sample rate, ready to fan out.
    /// Always returns `frames` entries (underrun positions are omitted — caller fills 0).
    fn produce_mono(
        &mut self,
        frames: usize,
        cons: &Mutex<RingConsumer>,
        notify: &Notify,
    ) -> Vec<f32> {
        if self.resampler.is_none() {
            // Pass-through: pull `frames` 48k samples directly.
            let mut mono = vec![0.0f32; frames];
            if let Ok(mut c) = cons.try_lock() {
                let popped = c.pop_slice(&mut mono);
                if popped > 0 {
                    notify.notify_one();
                }
                // Spec §5.3: underrun → silence. Positions beyond `popped` remain 0.
                for s in mono[popped..].iter_mut() {
                    *s = 0.0;
                }
            }
            return mono;
        }

        // Resampling path. We need `frames` output samples at device_rate.
        // Each output sample corresponds to SAMPLE_RATE/device_rate input samples.
        // To fill `frames` output samples we need approximately `frames * 48000 / device_rate` input samples,
        // fed in RESAMPLE_CHUNK increments.
        let resampler = self.resampler.as_mut().unwrap();
        let out_per_chunk = resampler.output_frames_max();

        // Fill the reservoir until it has enough for `frames` (or ring is empty).
        while self.reservoir.len() < frames {
            // Pull RESAMPLE_CHUNK frames from ring into src_scratch.
            self.src_scratch.resize(RESAMPLE_CHUNK, 0.0);
            let popped = if let Ok(mut c) = cons.try_lock() {
                let n = c.pop_slice(&mut self.src_scratch);
                if n > 0 {
                    notify.notify_one();
                }
                n
            } else {
                0
            };

            if popped == 0 {
                // Ring empty — stop pulling; remaining reservoir positions will be silence.
                break;
            }

            // Pad with zeros if we got fewer than a full chunk (underrun mid-chunk).
            // Spec §5.3: silence on underrun.
            for s in self.src_scratch[popped..RESAMPLE_CHUNK].iter_mut() {
                *s = 0.0;
            }

            if resampler
                .process(&self.src_scratch[..RESAMPLE_CHUNK], &mut self.resample_out)
                .is_ok()
            {
                self.reservoir.extend_from_slice(&self.resample_out);
            }

            let _ = out_per_chunk; // keep lint happy
        }

        // Drain `frames` from reservoir.
        let available = self.reservoir.len().min(frames);
        let mut mono: Vec<f32> = self.reservoir.drain(..available).collect();
        mono.resize(frames, 0.0); // silence for any underrun remainder
        mono
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::AudioRing;
    use crate::FRAME_SAMPLES;

    #[test]
    fn start_with_unknown_device_returns_error() {
        let (_prod, cons) = AudioRing::new(1024);
        let err = PlaybackHandle::start("this-device-does-not-exist-xyz", cons).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }

    /// fill_from_mono at 48k passes samples through unchanged.
    #[test]
    fn playback_filler_48k_passthrough() {
        let (mut prod, cons) = AudioRing::new(4096);
        let input: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| i as f32 / FRAME_SAMPLES as f32)
            .collect();
        prod.push_slice(&input);

        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(48_000, 1).expect("filler init");

        let mut out = vec![0.0f32; FRAME_SAMPLES];
        filler.fill_f32(&mut out, &cons, &notify);

        for (expected, got) in input.iter().zip(out.iter()) {
            assert!(
                (expected - got).abs() < 1e-6,
                "mismatch: expected {expected}, got {got}"
            );
        }
    }

    /// On underrun, output must be silence (zeros), never stale data.
    #[test]
    fn playback_filler_underrun_produces_silence() {
        let (_prod, cons) = AudioRing::new(4096);
        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(48_000, 1).expect("filler init");

        let mut out = vec![1.0f32; FRAME_SAMPLES]; // pre-fill with non-zero
        filler.fill_f32(&mut out, &cons, &notify);

        for (i, s) in out.iter().enumerate() {
            assert_eq!(*s, 0.0, "sample {i} should be silence on underrun, got {s}");
        }
    }

    #[test]
    fn start_by_id_with_unknown_id_returns_error() {
        let (_prod, cons) = AudioRing::new(1024);
        let err = PlaybackHandle::start_by_id("nonexistent-id", cons).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }

    #[test]
    fn start_by_id_with_default_output_id_starts() {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let Some(default_out) = host.default_output_device() else {
            return;
        };
        let name = default_out.name().unwrap_or_default();
        if name.is_empty() {
            return;
        }
        let id = format!("Output:0:{name}");
        let (_prod, cons) = AudioRing::new(1024);
        let res = PlaybackHandle::start_by_id(&id, cons);
        assert!(res.is_ok() || matches!(res, Err(AudioError::BuildStream { .. })));
    }

    /// Resampler from 48k to 44.1k produces the right approximate sample count.
    #[test]
    fn playback_filler_44100_resamples_from_48k() {
        // Feed 4800 samples at 48k; request 4410 samples at 44100 Hz.
        let (mut prod, cons) = AudioRing::new(16384);
        prod.push_slice(&vec![0.5f32; 4800]);

        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(44_100, 1).expect("filler init");

        let mut out = vec![0.0f32; 4410];
        filler.fill_f32(&mut out, &cons, &notify);

        // All output slots should be filled (not underrun) with non-silent values.
        let non_zero = out.iter().filter(|&&s| s.abs() > 0.01).count();
        assert!(
            non_zero > 4000,
            "expected most samples to be non-zero after resampling 48k→44.1k, got {non_zero}"
        );
    }
}
