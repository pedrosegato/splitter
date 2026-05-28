use audiomirror_core::audio::codec::{OpusDecoder, OpusEncoder};
use audiomirror_core::{FRAME_SAMPLES, SAMPLE_RATE};
use bytes::{Bytes, BytesMut};

pub struct SineSource {
    phase: f32,
}

impl SineSource {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }

    pub fn fill(&mut self, buf: &mut [f32]) {
        let delta = 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;
        for s in buf.iter_mut() {
            *s = self.phase.sin() * 0.5;
            self.phase = (self.phase + delta) % (2.0 * std::f32::consts::PI);
        }
    }
}

impl Default for SineSource {
    fn default() -> Self {
        Self::new()
    }
}

pub struct EncodedFrame {
    pub seq: u32,
    pub payload: Bytes,
}

pub fn encode_frames(source: &mut SineSource, count: usize) -> Vec<EncodedFrame> {
    let mut enc = OpusEncoder::new(64_000).expect("encoder init");
    let mut frame = vec![0.0f32; FRAME_SAMPLES];
    let mut out = BytesMut::with_capacity(400);
    (0..count)
        .map(|seq| {
            source.fill(&mut frame);
            enc.encode(&frame, &mut out).expect("encode");
            EncodedFrame {
                seq: seq as u32,
                payload: Bytes::copy_from_slice(&out),
            }
        })
        .collect()
}

pub fn decode_frames(frames: &[EncodedFrame]) -> Vec<f32> {
    let mut dec = OpusDecoder::new().expect("decoder init");
    let mut out_frame = vec![0.0f32; FRAME_SAMPLES];
    let mut samples = Vec::with_capacity(frames.len() * FRAME_SAMPLES);
    for f in frames {
        dec.decode(Some(&f.payload), &mut out_frame)
            .expect("decode");
        samples.extend_from_slice(&out_frame);
    }
    samples
}

pub fn rms(samples: &[f32]) -> f32 {
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_source_default_constructs() {
        let src = SineSource::default();
        assert_eq!(src.phase, 0.0);
    }

    #[test]
    fn encode_decode_smoke() {
        let mut src = SineSource::new();
        let frames = encode_frames(&mut src, 1);
        assert_eq!(frames.len(), 1);
        let samples = decode_frames(&frames);
        assert_eq!(samples.len(), FRAME_SAMPLES);
    }
}
