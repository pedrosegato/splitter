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
        Self::start_by_id(device_name, producer)
    }

    pub fn start_by_id(device_id: &str, producer: RingProducer) -> Result<Self, AudioError> {
        let device = resolve_input_device(device_id)?;
        Self::from_device(device, producer, Arc::new(Notify::new()))
    }

    pub fn start_loopback(producer: RingProducer) -> Result<Self, AudioError> {
        #[cfg(target_os = "windows")]
        {
            Self::start_loopback_wasapi(producer)
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
            Self::from_device(device, producer, Arc::new(Notify::new()))
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = producer;
            Err(AudioError::LoopbackUnsupported)
        }
    }

    // WHY: WASAPI loopback capture requires using the *output* device as the capture
    // source, but build_input_stream must be called with a SampleFormat that the WASAPI
    // driver will actually accept.  The previous approach called default_input_config()
    // and fell back to default_output_config(), which worked on Pedro's machine (F32
    // shared-mode output) but silently breaks on systems whose output is configured for
    // I32 or I24 — cpal would then try to build a stream with the wrong format and
    // return a BuildStream error.
    //
    // This dedicated path:
    //   1. Gets the default WASAPI output device.
    //   2. Iterates supported_output_configs() to find a format that build_input_stream
    //      can accept (F32 preferred, then I16/U16 for compatibility).
    //   3. Falls back to F32 if no supported range is found, letting the driver perform
    //      automatic format conversion (shared-mode WASAPI always supports F32).
    #[cfg(target_os = "windows")]
    fn start_loopback_wasapi(producer: RingProducer) -> Result<Self, AudioError> {
        use cpal::traits::DeviceTrait;

        let host =
            cpal::host_from_id(cpal::HostId::Wasapi).map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?;
        let device = host
            .default_output_device()
            .ok_or(AudioError::NoDefaultDevice)?;

        // Preferred formats in order: F32 (lossless), I16 (common), U16.
        let preferred = [
            cpal::SampleFormat::F32,
            cpal::SampleFormat::I16,
            cpal::SampleFormat::U16,
        ];

        let supported_cfg = device
            .supported_output_configs()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?
            .filter_map(|r| {
                let fmt = r.sample_format();
                if preferred.contains(&fmt) {
                    Some(r.with_max_sample_rate())
                } else {
                    None
                }
            })
            .min_by_key(|c| {
                // Lower index = higher preference.
                preferred
                    .iter()
                    .position(|&f| f == c.sample_format())
                    .unwrap_or(usize::MAX)
            });

        // Fall back to F32 at 48 kHz stereo — WASAPI shared-mode always supports this
        // via its built-in resampler/converter.
        let config = supported_cfg.unwrap_or_else(|| {
            cpal::SupportedStreamConfigRange::new(
                2,
                cpal::SampleRate(48_000),
                cpal::SampleRate(48_000),
                cpal::SupportedBufferSize::Range {
                    min: 128,
                    max: 4096,
                },
                cpal::SampleFormat::F32,
            )
            .with_max_sample_rate()
        });

        Self::from_device_with_config(device, config, producer, Arc::new(Notify::new()))
    }

    #[cfg(target_os = "windows")]
    fn from_device_with_config(
        device: cpal::Device,
        supported: cpal::SupportedStreamConfig,
        producer: RingProducer,
        frame_notify: Arc<Notify>,
    ) -> Result<Self, AudioError> {
        let stream = build_capture_stream(device, supported, producer, frame_notify.clone())?;
        Ok(Self {
            _stream: stream,
            frame_notify,
        })
    }

    pub fn from_device(
        device: cpal::Device,
        producer: RingProducer,
        frame_notify: Arc<Notify>,
    ) -> Result<Self, AudioError> {
        let supported = device.default_input_config().or_else(|_| {
            device
                .default_output_config()
                .map_err(|e| AudioError::BuildStream {
                    source: Box::new(e),
                })
        })?;
        let stream = build_capture_stream(device, supported, producer, frame_notify.clone())?;
        Ok(Self {
            _stream: stream,
            frame_notify,
        })
    }
}

fn build_capture_stream(
    device: cpal::Device,
    supported: cpal::SupportedStreamConfig,
    producer: RingProducer,
    frame_notify: Arc<Notify>,
) -> Result<cpal::Stream, AudioError> {
    let channels = supported.channels();
    let sample_rate = supported.sample_rate().0;
    let sample_format = supported.sample_format();
    let max_frames = max_callback_frames(supported.buffer_size());
    let config: cpal::StreamConfig = supported.into();

    let producer = Arc::new(Mutex::new(producer));
    let notify = frame_notify.clone();
    let err_fn = |e| tracing::error!("cpal capture stream error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let prod = producer.clone();
            let n = notify.clone();
            let mut router = SampleRouter::new(sample_rate, channels, max_frames)?;
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
            let mut router = SampleRouter::new(sample_rate, channels, max_frames)?;
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
            let mut router = SampleRouter::new(sample_rate, channels, max_frames)?;
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
    Ok(stream)
}

fn resolve_input_device(device_id: &str) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    if let Some(rest) = device_id.strip_prefix("in:") {
        let idx: usize = rest
            .parse()
            .map_err(|_| AudioError::DeviceNotFound(device_id.to_string()))?;
        let mut devices: Vec<cpal::Device> = host
            .input_devices()
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
    host.input_devices()
        .map_err(|e| AudioError::BuildStream {
            source: Box::new(e),
        })?
        .find(|d| d.name().map(|n| n == target_name).unwrap_or(false))
        .ok_or_else(|| AudioError::DeviceNotFound(device_id.to_string()))
}

// RESAMPLE_CHUNK is per-channel; scratch holds stereo pairs (RESAMPLE_CHUNK * 2 interleaved).
// 441 samples at 44100 Hz -> 480 samples at 48000 Hz (10ms slices); must divide evenly into FRAME_SAMPLES at 48k.
const RESAMPLE_CHUNK: usize = 441;

// SAFETY.md #1 forbids callback-time allocation, so scratch is pre-sized to the largest
// buffer the driver can hand us. Drivers that report Unknown give no bound; 4096 matches
// the max already assumed for WASAPI shared mode and acts as a clamped safety net.
const FALLBACK_MAX_CALLBACK_FRAMES: usize = 4096;

fn max_callback_frames(buffer_size: &cpal::SupportedBufferSize) -> usize {
    match buffer_size {
        cpal::SupportedBufferSize::Range { max, .. } => {
            (*max as usize).max(FALLBACK_MAX_CALLBACK_FRAMES)
        }
        cpal::SupportedBufferSize::Unknown => FALLBACK_MAX_CALLBACK_FRAMES,
    }
}

fn make_resampler(input_rate: u32) -> Result<Resampler, AudioError> {
    Resampler::new(input_rate, SAMPLE_RATE, RESAMPLE_CHUNK).map_err(|e| AudioError::BuildStream {
        source: Box::new(e),
    })
}

fn deinterleave_stereo_frame<T: Copy>(
    interleaved: &[T],
    base: usize,
    channels: usize,
    to_f32: &impl Fn(T) -> f32,
) -> (f32, f32) {
    let l = to_f32(interleaved[base]);
    let r = if channels >= 2 {
        to_f32(interleaved[base + 1])
    } else {
        l
    };
    (l, r)
}

/// Routes multi-channel interleaved samples -> stereo interleaved (L,R) -> optional resampler -> ring.
/// Mono sources are upmixed to stereo by duplicating the channel.
/// N>2 channel sources are downmixed to stereo using the first two channels.
/// Pre-allocated; no heap activity inside the cpal callback.
struct SampleRouter {
    channels: usize,
    resampler_l: Option<Resampler>,
    resampler_r: Option<Resampler>,
    scratch: Vec<f32>,
    resampled: Vec<f32>,
    l_in: Vec<f32>,
    r_in: Vec<f32>,
    l_out: Vec<f32>,
    r_out: Vec<f32>,
}

impl SampleRouter {
    fn new(sample_rate: u32, channels: u16, max_frames: usize) -> Result<Self, AudioError> {
        let (resampler_l, resampler_r) = if sample_rate != SAMPLE_RATE {
            let l = make_resampler(sample_rate)?;
            let r = make_resampler(sample_rate)?;
            (Some(l), Some(r))
        } else {
            (None, None)
        };

        let scratch_cap = if resampler_l.is_some() {
            RESAMPLE_CHUNK * 4
        } else {
            max_frames * 2
        };

        Ok(Self {
            channels: channels as usize,
            resampler_l,
            resampler_r,
            scratch: Vec::with_capacity(scratch_cap),
            resampled: Vec::with_capacity(scratch_cap * 2),
            l_in: Vec::with_capacity(RESAMPLE_CHUNK),
            r_in: Vec::with_capacity(RESAMPLE_CHUNK),
            l_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
            r_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
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
            self.scratch.clear();
            debug_assert!(
                frame_count * 2 <= self.scratch.capacity(),
                "capture scratch too small: need {} have {}",
                frame_count * 2,
                self.scratch.capacity()
            );
            for i in 0..frame_count {
                let (l, r) = deinterleave_stereo_frame(interleaved, i * ch, ch, &to_f32);
                self.scratch.push(l);
                self.scratch.push(r);
            }
            flush_to_ring(&self.scratch, prod, notify);
            return;
        }

        for i in 0..frame_count {
            let (l, r) = deinterleave_stereo_frame(interleaved, i * ch, ch, &to_f32);
            self.scratch.push(l);
            self.scratch.push(r);

            while self.scratch.len() >= RESAMPLE_CHUNK * 2 {
                self.l_in.clear();
                self.r_in.clear();
                self.l_in.extend(
                    self.scratch[..RESAMPLE_CHUNK * 2]
                        .iter()
                        .step_by(2)
                        .copied(),
                );
                self.r_in.extend(
                    self.scratch[..RESAMPLE_CHUNK * 2]
                        .iter()
                        .skip(1)
                        .step_by(2)
                        .copied(),
                );
                self.scratch.drain(..RESAMPLE_CHUNK * 2);
                let rl = self.resampler_l.as_mut().unwrap();
                let rr = self.resampler_r.as_mut().unwrap();
                if rl.process(&self.l_in, &mut self.l_out).is_ok()
                    && rr.process(&self.r_in, &mut self.r_out).is_ok()
                {
                    self.resampled.extend(
                        self.l_out
                            .iter()
                            .zip(self.r_out.iter())
                            .flat_map(|(&lv, &rv)| [lv, rv]),
                    );
                    flush_to_ring(&self.resampled, prod, notify);
                    self.resampled.clear();
                }
            }
        }
    }
}

fn flush_to_ring(samples: &[f32], prod: &Mutex<RingProducer>, notify: &Notify) {
    if let Ok(mut p) = prod.try_lock() {
        let pushed = p.push_slice(samples);
        if pushed > 0 {
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

    #[test]
    fn make_resampler_returns_ok_for_valid_rate() {
        assert!(make_resampler(44_100).is_ok());
        assert!(make_resampler(22_050).is_ok());
        assert!(make_resampler(96_000).is_ok());
    }

    #[test]
    fn deinterleave_stereo_frame_mono_upmix() {
        let samples = [0.5f32];
        let (l, r) = deinterleave_stereo_frame(&samples, 0, 1, &|s| s);
        assert!((l - 0.5).abs() < 1e-6);
        assert!((r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn deinterleave_stereo_frame_stereo_passthrough() {
        let samples = [0.3f32, 0.7f32];
        let (l, r) = deinterleave_stereo_frame(&samples, 0, 2, &|s| s);
        assert!((l - 0.3).abs() < 1e-6);
        assert!((r - 0.7).abs() < 1e-6);
    }

    #[test]
    fn deinterleave_stereo_frame_6ch_uses_first_two() {
        let samples = [0.1f32, 0.2f32, 0.3f32, 0.4f32, 0.5f32, 0.6f32];
        let (l, r) = deinterleave_stereo_frame(&samples, 0, 6, &|s| s);
        assert!((l - 0.1).abs() < 1e-6);
        assert!((r - 0.2).abs() < 1e-6);
    }

    #[test]
    fn deinterleave_stereo_frame_6ch_nonzero_base() {
        let samples = [
            0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.9f32, 0.8f32, 0.7f32, 0.6f32, 0.5f32,
            0.4f32,
        ];
        let (l, r) = deinterleave_stereo_frame(&samples, 6, 6, &|s| s);
        assert!((l - 0.9).abs() < 1e-6);
        assert!((r - 0.8).abs() < 1e-6);
    }

    /// Verifies that SampleRouter at 48k (no resampler) correctly converts stereo to stereo
    /// and delivers interleaved L,R pairs to the ring.
    #[test]
    fn sample_router_48k_stereo_passthrough_reaches_ring() {
        let (prod, mut cons) = AudioRing::new(8192);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(48_000, 2, 4096).expect("router init");

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

        let mut router = SampleRouter::new(48_000, 1, 4096).expect("router init");

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
                out[i * 2 + 1]
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

        let mut router = SampleRouter::new(44_100, 1, 4096).expect("router init");

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

    /// Verifies that flush_to_ring signals the Notify on any non-empty push so the
    /// consumer wakes promptly even when cpal callbacks deliver less than one full frame.
    #[tokio::test]
    async fn flush_to_ring_notifies_on_any_non_empty_push() {
        let (prod, _cons) = AudioRing::new(8192);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        flush_to_ring(&[0.0f32; 64], &prod, &notify);
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;
        assert!(result.is_ok(), "notify must fire on a non-empty push");
    }

    #[test]
    fn sample_router_resampler_buffers_reused_produce_identical_output() {
        let ring_cap = 65536;
        let (prod1, mut cons1) = AudioRing::new(ring_cap);
        let (prod2, mut cons2) = AudioRing::new(ring_cap);
        let prod1 = Arc::new(Mutex::new(prod1));
        let prod2 = Arc::new(Mutex::new(prod2));
        let notify1 = Arc::new(Notify::new());
        let notify2 = Arc::new(Notify::new());

        let mut router = SampleRouter::new(44_100, 1, 4096).expect("router init");

        let input = vec![0.6f32; 4410];
        router.push_f32(&input, &prod1, &notify1);

        let mut router2 = SampleRouter::new(44_100, 1, 4096).expect("router init");
        router2.push_f32(&input, &prod2, &notify2);

        let avail1 = cons1.occupied();
        let avail2 = cons2.occupied();
        assert_eq!(
            avail1, avail2,
            "both routers must produce the same sample count"
        );
        assert!(avail1 > 0, "must produce output");

        let mut out1 = vec![0.0f32; avail1];
        let mut out2 = vec![0.0f32; avail2];
        cons1.pop_slice(&mut out1);
        cons2.pop_slice(&mut out2);

        for (i, (&a, &b)) in out1.iter().zip(out2.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {i} mismatch: {a} vs {b}");
        }

        let (prod3, mut cons3) = AudioRing::new(ring_cap);
        let prod3 = Arc::new(Mutex::new(prod3));
        let notify3 = Arc::new(Notify::new());
        router.push_f32(&input, &prod3, &notify3);
        let avail3 = cons3.occupied();
        assert!(
            avail3 > 0,
            "second call must also produce output (buffers correctly reused)"
        );

        let (prod_ref_a, _) = AudioRing::new(ring_cap);
        let prod_ref_a = Arc::new(Mutex::new(prod_ref_a));
        let notify_ref_a = Arc::new(Notify::new());
        let (prod_ref_b, mut cons_ref_b) = AudioRing::new(ring_cap);
        let prod_ref_b = Arc::new(Mutex::new(prod_ref_b));
        let notify_ref_b = Arc::new(Notify::new());
        let mut router_ref = SampleRouter::new(44_100, 1, 4096).expect("router init");
        router_ref.push_f32(&input, &prod_ref_a, &notify_ref_a);
        router_ref.push_f32(&input, &prod_ref_b, &notify_ref_b);
        let avail_ref = cons_ref_b.occupied();
        assert_eq!(
            avail3, avail_ref,
            "reused router must produce same sample count as reference router on second call"
        );
        let mut out3 = vec![0.0f32; avail3];
        let mut out_ref = vec![0.0f32; avail_ref];
        cons3.pop_slice(&mut out3);
        cons_ref_b.pop_slice(&mut out_ref);
        for (i, (&a, &b)) in out3.iter().zip(out_ref.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "sample {i}: reused router ({a}) differs from reference second-call ({b}) — stale buffer contamination"
            );
        }
    }

    #[test]
    fn sample_router_fast_path_no_truncation_large_callback() {
        let frame_count = 4096;
        let ring_cap = frame_count * 2 * 2;
        let (prod, mut cons) = AudioRing::new(ring_cap);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(48_000, 2, 4096).expect("router init");

        let input: Vec<f32> = (0..frame_count).flat_map(|_| [0.3f32, 0.7f32]).collect();
        router.push_f32(&input, &prod, &notify);

        let available = cons.occupied();
        assert_eq!(
            available,
            frame_count * 2,
            "all {frame_count} stereo frames must reach the ring; got {available} samples"
        );

        let mut out = vec![0.0f32; frame_count * 2];
        cons.pop_slice(&mut out);
        for i in 0..frame_count {
            assert!(
                (out[i * 2] - 0.3).abs() < 1e-5,
                "L ch at frame {i}: expected 0.3, got {}",
                out[i * 2]
            );
            assert!(
                (out[i * 2 + 1] - 0.7).abs() < 1e-5,
                "R ch at frame {i}: expected 0.7, got {}",
                out[i * 2 + 1]
            );
        }
    }

    #[test]
    fn sample_router_fast_path_no_realloc_within_max() {
        let frame_count = 4096;
        let ring_cap = frame_count * 2 * 2;
        let (prod, _cons) = AudioRing::new(ring_cap);
        let prod = Arc::new(Mutex::new(prod));
        let notify = Arc::new(Notify::new());

        let mut router = SampleRouter::new(48_000, 2, 4096).expect("router init");
        let cap_before = router.scratch.capacity();

        let input: Vec<f32> = (0..frame_count).flat_map(|_| [0.3f32, 0.7f32]).collect();
        router.push_f32(&input, &prod, &notify);

        assert_eq!(
            router.scratch.capacity(),
            cap_before,
            "fast-path scratch must not reallocate within max_frames"
        );
    }
}
