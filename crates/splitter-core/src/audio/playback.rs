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

// RESAMPLE_CHUNK is per-channel; src_scratch holds stereo pairs (RESAMPLE_CHUNK * 2 interleaved).
// 441 samples at 44100 Hz -> 480 samples at 48000 Hz (10ms slices); must divide evenly into FRAME_SAMPLES at 48k.
const RESAMPLE_CHUNK: usize = 441;

/// Reads 48k stereo-interleaved (L,R,L,R,...) samples from the ring, resamples to device rate,
/// and fans to output channels. Mono output devices downmix L+R to avg.
/// All buffers are pre-allocated; no heap work inside the cpal callback.
struct PlaybackFiller {
    channels: usize,
    resampler_l: Option<Resampler>,
    resampler_r: Option<Resampler>,
    reservoir: Vec<f32>,
    src_scratch: Vec<f32>,
    l_in: Vec<f32>,
    r_in: Vec<f32>,
    l_out: Vec<f32>,
    r_out: Vec<f32>,
    stereo_buf: Vec<f32>,
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
            l_in: Vec::with_capacity(RESAMPLE_CHUNK),
            r_in: Vec::with_capacity(RESAMPLE_CHUNK),
            l_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
            r_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
            stereo_buf: Vec::with_capacity(4096),
        })
    }

    fn fill_f32(&mut self, out: &mut [f32], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        self.produce_stereo(frames, cons, notify);
        for i in 0..frames {
            let (l, r) = if i < self.stereo_buf.len() / 2 {
                (self.stereo_buf[i * 2], self.stereo_buf[i * 2 + 1])
            } else {
                (0.0, 0.0)
            };
            write_stereo_to_frame(out, i, ch, l, r);
        }
    }

    fn fill_i16(&mut self, out: &mut [i16], cons: &Mutex<RingConsumer>, notify: &Notify) {
        let ch = self.channels.max(1);
        let frames = out.len() / ch;
        self.produce_stereo(frames, cons, notify);
        for i in 0..frames {
            let (l, r) = if i < self.stereo_buf.len() / 2 {
                (self.stereo_buf[i * 2], self.stereo_buf[i * 2 + 1])
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
        self.produce_stereo(frames, cons, notify);
        for i in 0..frames {
            let (l, r) = if i < self.stereo_buf.len() / 2 {
                (self.stereo_buf[i * 2], self.stereo_buf[i * 2 + 1])
            } else {
                (0.0, 0.0)
            };
            let l_u = (l.clamp(-1.0, 1.0) * 32_767.0 + 32_768.0) as u16;
            let r_u = (r.clamp(-1.0, 1.0) * 32_767.0 + 32_768.0) as u16;
            write_stereo_to_frame_u16(out, i, ch, l_u, r_u);
        }
    }

    fn produce_stereo(&mut self, frames: usize, cons: &Mutex<RingConsumer>, notify: &Notify) {
        let stereo_needed = frames * 2;
        self.stereo_buf.clear();
        self.stereo_buf.resize(stereo_needed, 0.0);

        if self.resampler_l.is_none() {
            if let Ok(mut c) = cons.try_lock() {
                let popped = c.pop_slice(&mut self.stereo_buf);
                if popped > 0 {
                    notify.notify_one();
                }
                for s in self.stereo_buf[popped..].iter_mut() {
                    *s = 0.0;
                }
            }
            return;
        }

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

            self.l_in.clear();
            self.r_in.clear();
            self.l_in.extend(
                self.src_scratch[..RESAMPLE_CHUNK * 2]
                    .iter()
                    .step_by(2)
                    .copied(),
            );
            self.r_in.extend(
                self.src_scratch[..RESAMPLE_CHUNK * 2]
                    .iter()
                    .skip(1)
                    .step_by(2)
                    .copied(),
            );

            let rl = self.resampler_l.as_mut().unwrap();
            let rr = self.resampler_r.as_mut().unwrap();
            if rl.process(&self.l_in, &mut self.l_out).is_ok()
                && rr.process(&self.r_in, &mut self.r_out).is_ok()
            {
                self.reservoir.extend(
                    self.l_out
                        .iter()
                        .zip(self.r_out.iter())
                        .flat_map(|(&lv, &rv)| [lv, rv]),
                );
            }
        }

        let available = self.reservoir.len().min(stereo_needed);
        self.stereo_buf[..available].copy_from_slice(&self.reservoir[..available]);
        self.reservoir.drain(..available);
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
        assert!(res.is_ok() || res.is_err());
    }

    /// Resampler from 48k to 44.1k produces the right approximate stereo sample count.
    #[test]
    fn playback_filler_44100_resamples_from_48k() {
        let (mut prod, cons) = AudioRing::new(32768);
        prod.push_slice(&vec![0.5f32; 9600]);

        let cons = Arc::new(Mutex::new(cons));
        let notify = Arc::new(Notify::new());
        let mut filler = PlaybackFiller::new(44_100, 2).expect("filler init");

        let mut out = vec![0.0f32; 8820];
        filler.fill_f32(&mut out, &cons, &notify);

        let non_zero = out.iter().filter(|&&s| s.abs() > 0.01).count();
        assert!(
            non_zero > 8000,
            "expected most samples to be non-zero after resampling 48k->44.1k stereo, got {non_zero}"
        );
    }

    #[test]
    fn playback_filler_resampler_buffers_reused_produce_identical_output() {
        let (mut prod1, cons1) = AudioRing::new(32768);
        let (mut prod2, cons2) = AudioRing::new(32768);
        prod1.push_slice(&vec![0.5f32; 9600]);
        prod2.push_slice(&vec![0.5f32; 9600]);

        let cons1 = Arc::new(Mutex::new(cons1));
        let cons2 = Arc::new(Mutex::new(cons2));
        let notify1 = Arc::new(Notify::new());
        let notify2 = Arc::new(Notify::new());

        let mut filler1 = PlaybackFiller::new(44_100, 2).expect("filler init");
        let mut filler2 = PlaybackFiller::new(44_100, 2).expect("filler init");

        let mut out1 = vec![0.0f32; 8820];
        let mut out2 = vec![0.0f32; 8820];
        filler1.fill_f32(&mut out1, &cons1, &notify1);
        filler2.fill_f32(&mut out2, &cons2, &notify2);

        for (i, (&a, &b)) in out1.iter().zip(out2.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {i} mismatch: {a} vs {b}");
        }

        let (mut prod3, cons3) = AudioRing::new(32768);
        prod3.push_slice(&vec![0.5f32; 9600]);
        let cons3 = Arc::new(Mutex::new(cons3));
        let notify3 = Arc::new(Notify::new());
        let mut out3 = vec![0.0f32; 8820];
        filler1.fill_f32(&mut out3, &cons3, &notify3);

        let (mut prod_ref, cons_ref) = AudioRing::new(32768);
        prod_ref.push_slice(&vec![0.5f32; 9600]);
        let cons_ref = Arc::new(Mutex::new(cons_ref));
        let notify_ref = Arc::new(Notify::new());
        let mut out_ref = vec![0.0f32; 8820];
        filler2.fill_f32(&mut out_ref, &cons_ref, &notify_ref);

        for (i, (&a, &b)) in out3.iter().zip(out_ref.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "sample {i}: reused filler ({a}) differs from reference second-call ({b}) — stale buffer contamination"
            );
        }
    }
}
