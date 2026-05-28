use crate::audio::ring::RingConsumer;
use crate::error::AudioError;
use crate::SAMPLE_RATE;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

pub struct PlaybackHandle {
    _stream: cpal::Stream,
}

impl std::fmt::Debug for PlaybackHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackHandle").finish_non_exhaustive()
    }
}

impl PlaybackHandle {
    pub fn start(device_name: &str, consumer: RingConsumer) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .output_devices()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?
            .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| AudioError::DeviceNotFound(device_name.to_string()))?;

        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::BuildStream {
                source: Box::new(e),
            })?;
        let channels = supported.channels();
        let sample_rate = supported.sample_rate().0;
        let sample_format = supported.sample_format();

        if sample_rate != SAMPLE_RATE {
            tracing::warn!(
                "playback device runs at {sample_rate} Hz; resampling not enabled in Phase 1 playback path"
            );
        }
        let config: cpal::StreamConfig = supported.into();
        let consumer = Arc::new(Mutex::new(consumer));
        let err_fn = |e| tracing::error!("cpal playback stream error: {e}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let cons = consumer.clone();
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [f32], _| {
                            fill_from_mono::<f32>(out, channels, &cons, |x| x);
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
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [i16], _| {
                            fill_from_mono::<i16>(out, channels, &cons, |x| {
                                (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                            });
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
                device
                    .build_output_stream(
                        &config,
                        move |out: &mut [u16], _| {
                            fill_from_mono::<u16>(out, channels, &cons, |x| {
                                (x.clamp(-1.0, 1.0) * 32_767.0 + 32_768.0) as u16
                            });
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

fn fill_from_mono<T>(
    out: &mut [T],
    channels: u16,
    cons: &Mutex<RingConsumer>,
    convert: impl Fn(f32) -> T,
) where
    T: Copy + Default,
{
    let ch = channels as usize;
    if ch == 0 || out.is_empty() {
        return;
    }
    let frames = out.len() / ch;
    let mut mono = [0f32; 1024];
    let n = frames.min(mono.len());
    let popped = if let Ok(mut c) = cons.try_lock() {
        c.pop_slice(&mut mono[..n])
    } else {
        0
    };
    for i in 0..frames {
        let sample = if i < popped { mono[i] } else { 0.0 };
        let t = convert(sample);
        for c in 0..ch {
            out[i * ch + c] = t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::AudioRing;

    #[test]
    fn start_with_unknown_device_returns_error() {
        let (_prod, cons) = AudioRing::new(1024);
        let err = PlaybackHandle::start("this-device-does-not-exist-xyz", cons).unwrap_err();
        assert!(matches!(err, AudioError::DeviceNotFound(_)));
    }
}
