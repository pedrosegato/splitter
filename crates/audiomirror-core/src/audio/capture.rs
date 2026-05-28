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

// The chunk size fed into the resampler — must divide evenly into FRAME_SAMPLES at 48k.
// 441 samples at 44100 Hz → 480 samples at 48000 Hz (10ms slices).
const RESAMPLE_CHUNK: usize = 441;

/// Routes multi-channel interleaved samples → mono → optional resampler → ring.
/// Pre-allocated; no heap activity inside the cpal callback.
struct SampleRouter {
    channels: usize,
    resampler: Option<Resampler>,
    // scratch: accumulates mono samples before feeding the resampler in fixed chunks
    scratch: Vec<f32>,
    resampled: Vec<f32>,
}

impl SampleRouter {
    fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioError> {
        let resampler = if sample_rate != SAMPLE_RATE {
            let r = Resampler::new(sample_rate, SAMPLE_RATE, RESAMPLE_CHUNK).map_err(|e| {
                AudioError::BuildStream {
                    source: Box::new(e),
                }
            })?;
            Some(r)
        } else {
            None
        };

        let scratch_cap = if resampler.is_some() {
            RESAMPLE_CHUNK * 4
        } else {
            1024
        };

        Ok(Self {
            channels: channels as usize,
            resampler,
            scratch: Vec::with_capacity(scratch_cap),
            resampled: Vec::with_capacity(scratch_cap * 2),
        })
    }

    fn push_f32(&mut self, samples: &[f32], prod: &Mutex<RingProducer>, notify: &Notify) {
        self.downmix_and_route(samples, |s| s, prod, notify);
    }

    fn push_i16(&mut self, samples: &[i16], prod: &Mutex<RingProducer>, notify: &Notify) {
        self.downmix_and_route(samples, |s| s as f32 / i16::MAX as f32, prod, notify);
    }

    fn push_u16(&mut self, samples: &[u16], prod: &Mutex<RingProducer>, notify: &Notify) {
        self.downmix_and_route(samples, |s| (s as f32 - 32_768.0) / 32_768.0, prod, notify);
    }

    fn downmix_and_route<T: Copy>(
        &mut self,
        interleaved: &[T],
        to_f32: impl Fn(T) -> f32,
        prod: &Mutex<RingProducer>,
        notify: &Notify,
    ) {
        let ch = self.channels.max(1);
        let frame_count = interleaved.len() / ch;

        if self.resampler.is_none() {
            // Fast path: no resampling, downmix directly to a stack buffer.
            let mut tmp = [0f32; 1024];
            let n = frame_count.min(tmp.len());
            for (i, slot) in tmp.iter_mut().enumerate().take(n) {
                let base = i * ch;
                let mut sum = 0.0f32;
                for c in 0..ch {
                    sum += to_f32(interleaved[base + c]);
                }
                *slot = sum / ch as f32;
            }
            flush_to_ring(&tmp[..n], prod, notify);
            return;
        }

        // Accumulate downmixed mono into scratch, flush resampler in RESAMPLE_CHUNK slices.
        for i in 0..frame_count {
            let base = i * ch;
            let mut sum = 0.0f32;
            for c in 0..ch {
                sum += to_f32(interleaved[base + c]);
            }
            self.scratch.push(sum / ch as f32);

            while self.scratch.len() >= RESAMPLE_CHUNK {
                let chunk: Vec<f32> = self.scratch.drain(..RESAMPLE_CHUNK).collect();
                if let Ok(()) = self
                    .resampler
                    .as_mut()
                    .unwrap()
                    .process(&chunk, &mut self.resampled)
                {
                    flush_to_ring(&self.resampled, prod, notify);
                }
            }
        }
    }
}

fn flush_to_ring(samples: &[f32], prod: &Mutex<RingProducer>, notify: &Notify) {
    use crate::FRAME_SAMPLES;
    if let Ok(mut p) = prod.try_lock() {
        let pushed = p.push_slice(samples);
        if pushed >= FRAME_SAMPLES {
            notify.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::AudioRing;
    use crate::FRAME_SAMPLES;

    #[test]
    fn start_with_unknown_device_returns_error() {
        let (prod, _cons) = AudioRing::new(1024);
        let err = CaptureHandle::start("this-device-does-not-exist-xyz", prod).unwrap_err();
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

    /// Verifies that SampleRouter at 48k (no resampler) correctly downmixes stereo and
    /// delivers samples to the ring, triggering the notify after enough samples accumulate.
    #[test]
    fn sample_router_48k_stereo_downmix_reaches_ring() {
        let (prod, mut cons) = AudioRing::new(4096);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(48_000, 2).expect("router init");

        // Feed FRAME_SAMPLES stereo frames (L=0.4, R=0.8) → mono expected = 0.6
        let stereo: Vec<f32> = (0..FRAME_SAMPLES).flat_map(|_| [0.4f32, 0.8f32]).collect();
        router.push_f32(&stereo, &prod, &notify);

        let mut out = vec![0.0f32; FRAME_SAMPLES];
        let popped = cons.pop_slice(&mut out);
        assert_eq!(popped, FRAME_SAMPLES);
        for s in &out {
            assert!((s - 0.6).abs() < 1e-5, "expected 0.6, got {s}");
        }
    }

    /// Verifies that SampleRouter at 44100 Hz (with resampler) delivers approximately the
    /// right number of output samples (48000/44100 ratio) into the ring.
    #[test]
    fn sample_router_44100_resamples_to_48k() {
        let ring_cap = 8192;
        let (prod, cons) = AudioRing::new(ring_cap);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(44_100, 1).expect("router init");

        // Feed 4410 mono samples at 44100 Hz → expect ~4800 samples at 48000 Hz
        let input = vec![0.5f32; 4410];
        router.push_f32(&input, &prod, &notify);

        let available = cons.occupied();
        // Expect within 5% of the ideal ratio
        let expected = 4800usize;
        let lo = expected * 95 / 100;
        let hi = expected * 105 / 100;
        assert!(
            available >= lo && available <= hi,
            "expected ~{expected} resampled samples, got {available}"
        );
    }

    /// Verifies that flush_to_ring signals the Notify exactly when at least FRAME_SAMPLES
    /// samples have been pushed — sub-frame pushes must NOT signal.
    #[tokio::test]
    async fn flush_to_ring_notifies_at_frame_boundary() {
        let (prod, _cons) = AudioRing::new(4096);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        // Push fewer than FRAME_SAMPLES — Notify must NOT be triggered.
        flush_to_ring(&[0.0f32; FRAME_SAMPLES - 1], &prod, &notify);
        let timed_out =
            tokio::time::timeout(std::time::Duration::from_millis(10), notify.notified())
                .await
                .is_err();
        assert!(timed_out, "notify must NOT fire for sub-frame push");

        // Push exactly FRAME_SAMPLES — Notify must fire.
        flush_to_ring(&[0.0f32; FRAME_SAMPLES], &prod, &notify);
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;
        assert!(
            result.is_ok(),
            "notify must fire after a full frame is pushed"
        );
    }
}
