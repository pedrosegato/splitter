use crate::audio::resampler::Resampler;
use crate::audio::ring::RingProducer;
use crate::error::AudioError;
use crate::SAMPLE_RATE;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub struct CaptureHandle {
    _stream: cpal::Stream,
    frame_notify: Arc<Notify>,
}

impl std::fmt::Debug for CaptureHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureHandle").finish()
    }
}

impl CaptureHandle {
    pub fn frame_ready(&self) -> Arc<Notify> {
        self.frame_notify.clone()
    }

    pub fn start(device_name: &str, producer: RingProducer) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .input_devices()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?
            .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| AudioError::DeviceNotFound(device_name.to_string()))?;
        Self::from_device(device, producer, Arc::new(Notify::new()))
    }

    pub fn start_by_id(device_id: &str, producer: RingProducer) -> Result<Self, AudioError> {
        let resolved = resolve_input_device(device_id)?;
        Self::from_device(resolved, producer, Arc::new(Notify::new()))
    }

    pub fn start_loopback(producer: RingProducer) -> Result<Self, AudioError> {
        #[cfg(target_os = "windows")]
        {
            let host =
                cpal::host_from_id(cpal::HostId::Wasapi).map_err(|e| AudioError::BuildStream {
                    source: Box::new(e),
                })?;
            let device = host
                .default_output_device()
                .ok_or(AudioError::NoDefaultDevice)?;
            return Self::from_device(device, producer, Arc::new(Notify::new()));
        }
        #[cfg(target_os = "linux")]
        {
            let host = cpal::default_host();
            let device = host
                .input_devices()
                .map_err(|e| AudioError::BuildStream {
                    source: Box::new(e),
                })?
                .find(|d| d.name().map(|n| n.ends_with(".monitor")).unwrap_or(false))
                .ok_or_else(|| AudioError::DeviceNotFound("PulseAudio .monitor source".into()))?;
            return Self::from_device(device, producer, Arc::new(Notify::new()));
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = producer;
            Err(AudioError::LoopbackUnsupported)
        }
    }

    pub fn from_device(
        device: cpal::Device,
        producer: RingProducer,
        frame_notify: Arc<Notify>,
    ) -> Result<Self, AudioError> {
        let supported = device
            .default_input_config()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?;
        let channels = supported.channels();
        let sample_rate = supported.sample_rate().0;
        let sample_format = supported.sample_format();

        let config: cpal::StreamConfig = supported.into();

        let producer = Arc::new(Mutex::new(producer));
        let notify = frame_notify.clone();
        let err_fn = |e| tracing::error!("cpal capture stream error: {e}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let prod = producer.clone();
                let n = notify.clone();
                let mut router = SampleRouter::new(sample_rate, channels)?;
                device
                    .build_input_stream(
                        &config,
                        move |samples: &[f32], _| {
                            router.push_f32(samples, &prod, &n);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::BuildStream {
                        source: Box::new(e),
                    })?
            }
            cpal::SampleFormat::I16 => {
                let prod = producer.clone();
                let n = notify.clone();
                let mut router = SampleRouter::new(sample_rate, channels)?;
                device
                    .build_input_stream(
                        &config,
                        move |samples: &[i16], _| {
                            router.push_i16(samples, &prod, &n);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::BuildStream {
                        source: Box::new(e),
                    })?
            }
            cpal::SampleFormat::U16 => {
                let prod = producer.clone();
                let n = notify.clone();
                let mut router = SampleRouter::new(sample_rate, channels)?;
                device
                    .build_input_stream(
                        &config,
                        move |samples: &[u16], _| {
                            router.push_u16(samples, &prod, &n);
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
            frame_notify,
        })
    }
}

fn resolve_input_device(device_id: &str) -> Result<cpal::Device, AudioError> {
    let target_name = device_id
        .splitn(3, ':')
        .nth(2)
        .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()))?;
    let host = cpal::default_host();
    host.input_devices()
        .map_err(|e| AudioError::BuildStream {
            source: Box::new(e),
        })?
        .find(|d| d.name().map(|n| n == target_name).unwrap_or(false))
        .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()))
}

// The chunk size fed into the resampler — must divide evenly into FRAME_SAMPLES at 48k.
// 441 samples at 44100 Hz -> 480 samples at 48000 Hz (10ms slices).
const RESAMPLE_CHUNK: usize = 441;

/// Routes multi-channel interleaved samples -> stereo interleaved (L,R) -> optional resampler -> ring.
/// Mono sources are upmixed to stereo by duplicating the channel.
/// N>2 channel sources are downmixed to stereo using the first two channels.
/// Pre-allocated; no heap activity inside the cpal callback.
struct SampleRouter {
    channels: usize,
    // Separate resamplers for L and R to keep per-channel state independent.
    resampler_l: Option<Resampler>,
    resampler_r: Option<Resampler>,
    // scratch: accumulates stereo-interleaved samples before feeding the resamplers
    scratch: Vec<f32>,
    resampled: Vec<f32>,
}

impl SampleRouter {
    fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioError> {
        let (resampler_l, resampler_r) = if sample_rate != SAMPLE_RATE {
            let l = Resampler::new(sample_rate, SAMPLE_RATE, RESAMPLE_CHUNK).map_err(|e| {
                AudioError::BuildStream {
                    source: Box::new(e),
                }
            })?;
            let r = Resampler::new(sample_rate, SAMPLE_RATE, RESAMPLE_CHUNK).map_err(|e| {
                AudioError::BuildStream {
                    source: Box::new(e),
                }
            })?;
            (Some(l), Some(r))
        } else {
            (None, None)
        };

        let scratch_cap = if resampler_l.is_some() {
            RESAMPLE_CHUNK * 4
        } else {
            1024
        };

        Ok(Self {
            channels: channels as usize,
            resampler_l,
            resampler_r,
            scratch: Vec::with_capacity(scratch_cap),
            resampled: Vec::with_capacity(scratch_cap * 2),
        })
    }

    fn push_f32(&mut self, samples: &[f32], prod: &Mutex<RingProducer>, notify: &Notify) {
        self.convert_and_route(samples, |s| s, prod, notify);
    }

    fn push_i16(&mut self, samples: &[i16], prod: &Mutex<RingProducer>, notify: &Notify) {
        self.convert_and_route(samples, |s| s as f32 / i16::MAX as f32, prod, notify);
    }

    fn push_u16(&mut self, samples: &[u16], prod: &Mutex<RingProducer>, notify: &Notify) {
        self.convert_and_route(samples, |s| (s as f32 - 32_768.0) / 32_768.0, prod, notify);
    }

    fn convert_and_route<T: Copy>(
        &mut self,
        interleaved: &[T],
        to_f32: impl Fn(T) -> f32,
        prod: &Mutex<RingProducer>,
        notify: &Notify,
    ) {
        let ch = self.channels.max(1);
        let frame_count = interleaved.len() / ch;

        if self.resampler_l.is_none() {
            // Fast path: no resampling, convert to stereo directly into a stack buffer.
            // Output is 2 * frame_count f32 values.
            let mut tmp = [0f32; 2048];
            let stereo_count = (frame_count * 2).min(tmp.len());
            let frames = stereo_count / 2;
            for i in 0..frames {
                let base = i * ch;
                let l = to_f32(interleaved[base]);
                let r = if ch >= 2 {
                    to_f32(interleaved[base + 1])
                } else {
                    l
                };
                tmp[i * 2] = l;
                tmp[i * 2 + 1] = r;
            }
            flush_to_ring(&tmp[..frames * 2], prod, notify);
            return;
        }

        // Resampling path: accumulate stereo-interleaved samples in scratch,
        // flush resampler in RESAMPLE_CHUNK slices.
        // Note: RESAMPLE_CHUNK is per-channel; scratch holds stereo pairs so
        // we flush when we have RESAMPLE_CHUNK frames = RESAMPLE_CHUNK*2 samples.
        for i in 0..frame_count {
            let base = i * ch;
            let l = to_f32(interleaved[base]);
            let r = if ch >= 2 {
                to_f32(interleaved[base + 1])
            } else {
                l
            };
            self.scratch.push(l);
            self.scratch.push(r);

            while self.scratch.len() >= RESAMPLE_CHUNK * 2 {
                let chunk: Vec<f32> = self.scratch.drain(..RESAMPLE_CHUNK * 2).collect();
                // Resampler operates on mono; run L and R channels separately then interleave.
                let l_in: Vec<f32> = chunk.iter().step_by(2).copied().collect();
                let r_in: Vec<f32> = chunk.iter().skip(1).step_by(2).copied().collect();
                let mut l_out = Vec::new();
                let mut r_out = Vec::new();
                let rl = self.resampler_l.as_mut().unwrap();
                let rr = self.resampler_r.as_mut().unwrap();
                if rl.process(&l_in, &mut l_out).is_ok() && rr.process(&r_in, &mut r_out).is_ok() {
                    let stereo: Vec<f32> = l_out
                        .iter()
                        .zip(r_out.iter())
                        .flat_map(|(&lv, &rv)| [lv, rv])
                        .collect();
                    self.resampled.extend_from_slice(&stereo);
                    flush_to_ring(&self.resampled, prod, notify);
                    self.resampled.clear();
                }
            }
        }
    }
}

fn flush_to_ring(samples: &[f32], prod: &Mutex<RingProducer>, notify: &Notify) {
    use crate::FRAME_STEREO_SAMPLES;
    if let Ok(mut p) = prod.try_lock() {
        let pushed = p.push_slice(samples);
        if pushed >= FRAME_STEREO_SAMPLES {
            notify.notify_one();
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
        let (prod, _cons) = AudioRing::new(1024);
        let err = CaptureHandle::start("this-device-does-not-exist-xyz", prod).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }

    #[test]
    fn capture_start_by_id_with_unknown_id_returns_error() {
        let (prod, _cons) = AudioRing::new(1024);
        let err = CaptureHandle::start_by_id("nonexistent-id", prod).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }

    #[test]
    fn from_device_with_default_input_starts() {
        use cpal::traits::HostTrait;
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else {
            return;
        };
        let (prod, _cons) = AudioRing::new(1024);
        let res = CaptureHandle::from_device(device, prod, Arc::new(Notify::new()));
        assert!(res.is_ok() || matches!(res, Err(AudioError::BuildStream { .. })));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn start_loopback_returns_handle_on_windows() {
        let (prod, _cons) = AudioRing::new(48_000);
        let res = CaptureHandle::start_loopback(prod);
        match res {
            Ok(_) => {}
            Err(AudioError::BuildStream { .. }) => {}
            Err(AudioError::DeviceNotFound(_)) => {}
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_loopback_returns_unsupported_on_macos() {
        let (prod, _cons) = AudioRing::new(48_000);
        let res = CaptureHandle::start_loopback(prod);
        assert!(matches!(res, Err(AudioError::LoopbackUnsupported)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_loopback_returns_handle_or_missing_monitor_on_linux() {
        let (prod, _cons) = AudioRing::new(48_000);
        let res = CaptureHandle::start_loopback(prod);
        match res {
            Ok(_) => {}
            Err(AudioError::DeviceNotFound(_)) => {}
            Err(AudioError::BuildStream { .. }) => {}
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    /// Verifies that SampleRouter at 48k (no resampler) correctly converts stereo to stereo
    /// and delivers interleaved L,R pairs to the ring.
    #[test]
    fn sample_router_48k_stereo_passthrough_reaches_ring() {
        let (prod, mut cons) = AudioRing::new(8192);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(48_000, 2).expect("router init");

        // Feed FRAME_STEREO_SAMPLES/2 stereo frames (L=0.4, R=0.8)
        use crate::FRAME_SAMPLES;
        let stereo: Vec<f32> = (0..FRAME_SAMPLES).flat_map(|_| [0.4f32, 0.8f32]).collect();
        router.push_f32(&stereo, &prod, &notify);

        let mut out = vec![0.0f32; FRAME_STEREO_SAMPLES];
        let popped = cons.pop_slice(&mut out);
        assert_eq!(popped, FRAME_STEREO_SAMPLES);
        for i in 0..FRAME_SAMPLES {
            assert!(
                (out[i * 2] - 0.4).abs() < 1e-5,
                "L ch expected 0.4, got {}",
                out[i * 2]
            );
            assert!(
                (out[i * 2 + 1] - 0.8).abs() < 1e-5,
                "R ch expected 0.8, got {}",
                out[i * 2 + 1]
            );
        }
    }

    /// Mono input is upmixed to stereo by duplicating the channel.
    #[test]
    fn sample_router_48k_mono_upmix_to_stereo() {
        let (prod, mut cons) = AudioRing::new(8192);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(48_000, 1).expect("router init");

        use crate::FRAME_SAMPLES;
        let mono: Vec<f32> = vec![0.5f32; FRAME_SAMPLES];
        router.push_f32(&mono, &prod, &notify);

        let mut out = vec![0.0f32; FRAME_STEREO_SAMPLES];
        let popped = cons.pop_slice(&mut out);
        assert_eq!(popped, FRAME_STEREO_SAMPLES);
        for i in 0..FRAME_SAMPLES {
            assert!(
                (out[i * 2] - 0.5).abs() < 1e-5,
                "L ch expected 0.5, got {}",
                out[i * 2]
            );
            assert!(
                (out[i * 2 + 1] - 0.5).abs() < 1e-5,
                "R ch expected 0.5, got {}",
                out[i * 2 + 1]
            );
        }
    }

    /// Verifies that SampleRouter at 44100 Hz (with resampler) delivers approximately the
    /// right number of output stereo samples (48000/44100 ratio * 2 for stereo) into the ring.
    #[test]
    fn sample_router_44100_resamples_to_48k() {
        let ring_cap = 16384;
        let (prod, cons) = AudioRing::new(ring_cap);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(44_100, 1).expect("router init");

        // Feed 4410 mono samples at 44100 Hz -> expect ~4800 stereo pairs = ~9600 samples at 48000 Hz
        let input = vec![0.5f32; 4410];
        router.push_f32(&input, &prod, &notify);

        let available = cons.occupied();
        // Expect within 5% of the ideal ratio (stereo output)
        let expected = 9600usize;
        let lo = expected * 95 / 100;
        let hi = expected * 105 / 100;
        assert!(
            available >= lo && available <= hi,
            "expected ~{expected} resampled stereo samples, got {available}"
        );
    }

    /// Verifies that flush_to_ring signals the Notify exactly when at least FRAME_STEREO_SAMPLES
    /// samples have been pushed -- sub-frame pushes must NOT signal.
    #[tokio::test]
    async fn flush_to_ring_notifies_at_frame_boundary() {
        let (prod, _cons) = AudioRing::new(8192);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        // Push fewer than FRAME_STEREO_SAMPLES -- Notify must NOT be triggered.
        flush_to_ring(&[0.0f32; FRAME_STEREO_SAMPLES - 1], &prod, &notify);
        let timed_out =
            tokio::time::timeout(std::time::Duration::from_millis(10), notify.notified())
                .await
                .is_err();
        assert!(timed_out, "notify must NOT fire for sub-frame push");

        // Push exactly FRAME_STEREO_SAMPLES -- Notify must fire.
        flush_to_ring(&[0.0f32; FRAME_STEREO_SAMPLES], &prod, &notify);
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;
        assert!(
            result.is_ok(),
            "notify must fire after a full stereo frame is pushed"
        );
    }
}
