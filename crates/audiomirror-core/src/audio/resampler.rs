use crate::error::CodecError;
use rubato::{FftFixedIn, Resampler as RubatoResampler};

pub struct Resampler {
    inner: FftFixedIn<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
    in_rate: u32,
    out_rate: u32,
    chunk: usize,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, chunk_size: usize) -> Result<Self, CodecError> {
        let inner = FftFixedIn::<f32>::new(in_rate as usize, out_rate as usize, chunk_size, 2, 1)
            .map_err(|e| CodecError::Resampler {
            source: Box::new(e),
        })?;
        let in_buf = vec![vec![0.0f32; chunk_size]];
        let out_capacity = inner.output_frames_max();
        let out_buf = vec![vec![0.0f32; out_capacity]];
        Ok(Self {
            inner,
            in_buf,
            out_buf,
            in_rate,
            out_rate,
            chunk: chunk_size,
        })
    }

    pub fn output_frames_max(&self) -> usize {
        if self.in_rate == self.out_rate {
            self.chunk
        } else {
            self.inner.output_frames_max()
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), CodecError> {
        if self.in_rate == self.out_rate {
            output.clear();
            output.reserve(input.len());
            output.extend_from_slice(input);
            return Ok(());
        }
        if input.len() != self.chunk {
            return Err(CodecError::Resampler {
                source: Box::new(std::io::Error::other(format!(
                    "input chunk must be {}, got {}",
                    self.chunk,
                    input.len()
                ))),
            });
        }
        self.in_buf[0].copy_from_slice(input);
        let (_in_used, out_written) = self
            .inner
            .process_into_buffer(&self.in_buf, &mut self.out_buf, None)
            .map_err(|e| CodecError::Resampler {
                source: Box::new(e),
            })?;
        output.clear();
        output.reserve(out_written);
        output.extend_from_slice(&self.out_buf[0][..out_written]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FRAME_SAMPLES;

    #[test]
    fn passthrough_48k_to_48k_is_identity_length() {
        let mut r = Resampler::new(48_000, 48_000, FRAME_SAMPLES).unwrap();
        let input = vec![0.5f32; FRAME_SAMPLES];
        let mut output = Vec::new();
        r.process(&input, &mut output).unwrap();
        assert_eq!(output.len(), FRAME_SAMPLES);
    }

    #[test]
    fn resample_44100_to_48000_changes_length_proportionally() {
        let in_size = 882;
        let mut r = Resampler::new(44_100, 48_000, in_size).unwrap();
        let input = vec![0.0f32; in_size];
        let mut output = Vec::new();
        r.process(&input, &mut output).unwrap();
        let expected = (in_size as f64 * 48_000.0 / 44_100.0) as usize;
        assert!(
            output.len().abs_diff(expected) <= 4,
            "got {} samples, expected ~{}",
            output.len(),
            expected
        );
    }
}
