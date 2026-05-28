use crate::audio::ring::RingProducer;
use crate::error::AudioError;
use crate::SAMPLE_RATE;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

pub struct CaptureHandle {
    _stream: cpal::Stream,
}

impl std::fmt::Debug for CaptureHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureHandle").finish()
    }
}

impl CaptureHandle {
    pub fn start(device_name: &str, producer: RingProducer) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .input_devices()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?
            .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| AudioError::DeviceNotFound(device_name.to_string()))?;

        let supported = device
            .default_input_config()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?;
        let channels = supported.channels();
        let sample_rate = supported.sample_rate().0;
        let sample_format = supported.sample_format();

        if sample_rate != SAMPLE_RATE {
            tracing::warn!(
                "capture device runs at {sample_rate} Hz; resampling not enabled in Phase 1 capture path"
            );
        }
        let config: cpal::StreamConfig = supported.into();

        let producer = Arc::new(Mutex::new(producer));
        let err_fn = |e| tracing::error!("cpal capture stream error: {e}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let prod = producer.clone();
                device
                    .build_input_stream(
                        &config,
                        move |samples: &[f32], _| {
                            push_mono(samples, channels, &prod);
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
                device
                    .build_input_stream(
                        &config,
                        move |samples: &[i16], _| {
                            let mut tmp = [0f32; 2048];
                            let n = samples.len().min(tmp.len());
                            for (i, s) in samples.iter().take(n).enumerate() {
                                tmp[i] = *s as f32 / i16::MAX as f32;
                            }
                            push_mono(&tmp[..n], channels, &prod);
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
                device
                    .build_input_stream(
                        &config,
                        move |samples: &[u16], _| {
                            let mut tmp = [0f32; 2048];
                            let n = samples.len().min(tmp.len());
                            for (i, s) in samples.iter().take(n).enumerate() {
                                tmp[i] = (*s as f32 - 32_768.0) / 32_768.0;
                            }
                            push_mono(&tmp[..n], channels, &prod);
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
        Ok(Self { _stream: stream })
    }
}

fn push_mono(samples: &[f32], channels: u16, prod: &Mutex<RingProducer>) {
    let ch = channels as usize;
    if ch == 0 {
        return;
    }
    let mut frame = [0f32; 1024];
    let mono_len = samples.len() / ch;
    let n = mono_len.min(frame.len());
    for (i, slot) in frame.iter_mut().enumerate().take(n) {
        let base = i * ch;
        let mut sum = 0.0;
        for c in 0..ch {
            sum += samples[base + c];
        }
        *slot = sum / ch as f32;
    }
    if let Ok(mut p) = prod.try_lock() {
        let _ = p.push_slice(&frame[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::AudioRing;

    #[test]
    fn start_with_unknown_device_returns_error() {
        let (prod, _cons) = AudioRing::new(1024);
        let err = CaptureHandle::start("this-device-does-not-exist-xyz", prod).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }
}
