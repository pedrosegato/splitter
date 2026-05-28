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
        let device = resolve_output_device(device_name)?;
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
    let host = cpal::default_host();
    if let Some(rest) = device_id.strip_prefix("out:") {
        let idx: usize = rest
            .parse()
            .map_err(|_| AudioError::DeviceNotFound(device_id.to_string()))?;
        let mut devices: Vec<cpal::Device> = host
            .output_devices()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?
            .collect();
        devices.sort_by_key(|d| d.name().unwrap_or_default());
        return devices
            .into_iter()
            .nth(idx)
            .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()));
    }
    let target_name = device_id.splitn(3, ':').nth(2).unwrap_or(device_id);
    host.output_devices()
        .map_err(|e| AudioError::BuildStream {
            source: Box::new(e),
        })?
        .find(|d| d.name().map(|n| n == target_name).unwrap_or(false))
        .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()))
}

// Same chunk granularity as capture: 441 input frames at 44100 -> 480 at 48000.
const RESAMPLE_CHUNK: usize = 441;

/// Reads 48k stereo-interleaved (L,R,L,R,...) samples from the ring, resamples to device rate,
/// and fans to output channels. Mono output devices downmix L+R to avg.
/// All buffers are pre-allocated; no heap work inside the cpal callback.
struct PlaybackFiller {
    channels: usize,
    // Separate resamplers for L and R to keep per-channel state independent.
    resampler_l: Option<Resampler>,
    resampler_r: Option<Resampler>,
    // reservoir: holds already-resampled stereo-interleaved samples waiting for the next fill.
    reservoir: Vec<f32>,
    // src_scratch: accumulates 48k stereo-interleaved samples to feed into resampler chunks.
    src_scratch: Vec<f32>,
}

impl PlaybackFiller {
    fn new(device_rate: u32, channels: u16) -> Result<Self, AudioError> {
        let (resampler_l, resampler_r) = if device_rate != SAMPLE_RATE {
            // Resampler converts FROM 48k (ring) TO device_rate (output).
            let l = Resampler::new(SAMPLE_RATE, device_rate, RESAMPLE_CHUNK).map_err(|e| {
                AudioError::BuildStream {
                    source: Box::new(e),
                }
            })?;
            let r = Resampler::new(SAMPLE_RATE, device_rate, RESAMPLE_CHUNK).map_err(|e| {
                AudioError::BuildStream {
                    source: Box::new(e),
                }
            })?;
            (Some(l), Some(r))
        } else {
            (None, None)
        };

        Ok(Self {
            channels: channels as usize,
            resampler_l,
            resampler_r,
            reservoir: Vec::with_capacity(4096),
            src_scratch: Vec::with_capacity(RESAMPLE_CHUNK * 8),
        })
    }

    fn fill_f32(&mut self, out: &mut [f32], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        let stereo = self.produce_stereo(frames, cons, notify);
        for i in 0..frames {
            let (l, r) = if i < stereo.len() / 2 {
                (stereo[i * 2], stereo[i * 2 + 1])
            } else {
                (0.0, 0.0)
            };
            write_stereo_to_frame(out, i, ch, l, r);
        }
    }

    fn fill_i16(&mut self, out: &mut [i16], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        let stereo = self.produce_stereo(frames, cons, notify);
        for i in 0..frames {
            let (l, r) = if i < stereo.len() / 2 {
                (stereo[i * 2], stereo[i * 2 + 1])
            } else {
                (0.0, 0.0)
            };
            let l_i = (l.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            let r_i = (r.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            write_stereo_to_frame_i16(out, i, ch, l_i, r_i);
        }
    }

    fn fill_u16(&mut self, out: &mut [u16], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        let stereo = self.produce_stereo(frames, cons, notify);
        for i in 0..frames {
            let (l, r) = if i < stereo.len() / 2 {
                (stereo[i * 2], stereo[i * 2 + 1])
            } else {
                (0.0, 0.0)
            };
            let l_u = (l.clamp(-1.0, 1.0) * 32_767.0 + 32_768.0) as u16;
            let r_u = (r.clamp(-1.0, 1.0) * 32_767.0 + 32_768.0) as u16;
            write_stereo_to_frame_u16(out, i, ch, l_u, r_u);
        }
    }

    /// Returns a vec of interleaved stereo samples at the device's sample rate.
    /// The returned vec has `frames * 2` entries (L,R pairs); underrun positions are 0.
    fn produce_stereo(
        &mut self,
        frames: usize,
        cons: &Mutex<RingConsumer>,
        notify: &Notify,
    ) -> Vec<f32> {
        let stereo_needed = frames * 2;

        if self.resampler_l.is_none() {
            // Pass-through: pull stereo_needed samples directly.
            let mut stereo = vec![0.0f32; stereo_needed];
            if let Ok(mut c) = cons.try_lock() {
                let popped = c.pop_slice(&mut stereo);
                if popped > 0 {
                    notify.notify_one();
                }
                for s in stereo[popped..].iter_mut() {
                    *s = 0.0;
                }
            }
            return stereo;
        }

        // Resampling path. We need `frames` output frames at device_rate.
        // Ring holds stereo at 48k, so we pull RESAMPLE_CHUNK*2 at a time (stereo pairs),
        // resample each channel separately (independent state), then re-interleave.
        while self.reservoir.len() < stereo_needed {
            self.src_scratch.resize(RESAMPLE_CHUNK * 2, 0.0);
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
                break;
            }

            for s in self.src_scratch[popped..RESAMPLE_CHUNK * 2].iter_mut() {
                *s = 0.0;
            }

            let l_in: Vec<f32> = self.src_scratch[..RESAMPLE_CHUNK * 2]
                .iter()
                .step_by(2)
                .copied()
                .collect();
            let r_in: Vec<f32> = self.src_scratch[..RESAMPLE_CHUNK * 2]
                .iter()
                .skip(1)
                .step_by(2)
                .copied()
                .collect();

            let mut l_out = Vec::new();
            let mut r_out = Vec::new();
            let rl = self.resampler_l.as_mut().unwrap();
            let rr = self.resampler_r.as_mut().unwrap();
            if rl.process(&l_in, &mut l_out).is_ok() && rr.process(&r_in, &mut r_out).is_ok() {
                let stereo_out: Vec<f32> = l_out
                    .iter()
                    .zip(r_out.iter())
                    .flat_map(|(&lv, &rv)| [lv, rv])
                    .collect();
                self.reservoir.extend_from_slice(&stereo_out);
            }
        }

        let available = self.reservoir.len().min(stereo_needed);
        let mut stereo: Vec<f32> = self.reservoir.drain(..available).collect();
        stereo.resize(stereo_needed, 0.0);
        stereo
    }
}

/// Write a stereo frame (L,R) to an f32 output buffer at frame index `i` with `ch` channels.
/// ch=1: downmix to mono; ch=2: L->0, R->1; ch>2: L->0, R->1, silence elsewhere.
#[inline]
fn write_stereo_to_frame(out: &mut [f32], i: usize, ch: usize, l: f32, r: f32) {
    let base = i * ch;
    match ch {
        1 => out[base] = (l + r) * 0.5,
        2 => {
            out[base] = l;
            out[base + 1] = r;
        }
        _ => {
            out[base] = l;
            out[base + 1] = r;
            for c in 2..ch {
                out[base + c] = 0.0;
            }
        }
    }
}

#[inline]
fn write_stereo_to_frame_i16(out: &mut [i16], i: usize, ch: usize, l: i16, r: i16) {
    let base = i * ch;
    match ch {
        1 => out[base] = ((l as i32 + r as i32) / 2) as i16,
        2 => {
            out[base] = l;
            out[base + 1] = r;
        }
        _ => {
            out[base] = l;
            out[base + 1] = r;
            for c in 2..ch {
                out[base + c] = 0;
            }
        }
    }
}

#[inline]
fn write_stereo_to_frame_u16(out: &mut [u16], i: usize, ch: usize, l: u16, r: u16) {
    let base = i * ch;
    match ch {
        1 => out[base] = ((l as u32 + r as u32) / 2) as u16,
        2 => {
            out[base] = l;
            out[base + 1] = r;
        }
        _ => {
            out[base] = l;
            out[base + 1] = r;
            for c in 2..ch {
                out[base + c] = 32_768u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::AudioRing;
    use crate::FRAME_STEREO_SAMPLES;

    #[test]
    fn start_with_unknown_device_returns_error() {
        let (_prod, cons) = AudioRing::new(1024);
        let err = PlaybackHandle::start("this-device-does-not-exist-xyz", cons).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }

    /// Ring contains stereo-interleaved samples; 2-channel output gets them correctly mapped.
    #[test]
    fn playback_filler_48k_stereo_passthrough() {
        let (mut prod, cons) = AudioRing::new(8192);
        // Push stereo: L=0.3, R=0.7
        let input: Vec<f32> = (0..crate::FRAME_SAMPLES)
            .flat_map(|_| [0.3f32, 0.7f32])
            .collect();
        prod.push_slice(&input);

        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(48_000, 2).expect("filler init");

        let mut out = vec![0.0f32; FRAME_STEREO_SAMPLES];
        filler.fill_f32(&mut out, &cons, &notify);

        use crate::FRAME_SAMPLES;
        for i in 0..FRAME_SAMPLES {
            assert!((out[i * 2] - 0.3).abs() < 1e-6, "L mismatch at {i}");
            assert!((out[i * 2 + 1] - 0.7).abs() < 1e-6, "R mismatch at {i}");
        }
    }

    /// Mono output device (ch=1) gets L+R averaged.
    #[test]
    fn playback_filler_mono_output_downmixes() {
        let (mut prod, cons) = AudioRing::new(8192);
        let input: Vec<f32> = (0..crate::FRAME_SAMPLES)
            .flat_map(|_| [0.4f32, 0.8f32])
            .collect();
        prod.push_slice(&input);

        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(48_000, 1).expect("filler init");

        let mut out = vec![0.0f32; crate::FRAME_SAMPLES];
        filler.fill_f32(&mut out, &cons, &notify);

        for (i, s) in out.iter().enumerate() {
            assert!(
                (s - 0.6).abs() < 1e-5,
                "mono sample {i} expected 0.6, got {s}"
            );
        }
    }

    /// On underrun, output must be silence (zeros), never stale data.
    #[test]
    fn playback_filler_underrun_produces_silence() {
        let (_prod, cons) = AudioRing::new(4096);
        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(48_000, 2).expect("filler init");

        let mut out = vec![1.0f32; FRAME_STEREO_SAMPLES]; // pre-fill with non-zero
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

    /// Resampler from 48k to 44.1k produces the right approximate stereo sample count.
    #[test]
    fn playback_filler_44100_resamples_from_48k() {
        // Feed 4800 stereo frames (9600 samples) at 48k; request 4410 frames (8820 samples) at 44100.
        let (mut prod, cons) = AudioRing::new(32768);
        prod.push_slice(&vec![0.5f32; 9600]);

        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(44_100, 2).expect("filler init");

        let mut out = vec![0.0f32; 8820]; // 4410 stereo frames
        filler.fill_f32(&mut out, &cons, &notify);

        let non_zero = out.iter().filter(|&&s| s.abs() > 0.01).count();
        assert!(
            non_zero > 8000,
            "expected most samples to be non-zero after resampling 48k->44.1k stereo, got {non_zero}"
        );
    }
}
