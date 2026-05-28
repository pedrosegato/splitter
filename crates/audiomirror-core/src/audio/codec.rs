use crate::error::CodecError;
use crate::FRAME_SAMPLES;
use bytes::BytesMut;

use audiopus::{coder, packet::Packet, Application, Bitrate, Channels, MutSignals, SampleRate};
use std::convert::TryFrom;

pub struct OpusEncoder {
    inner: coder::Encoder,

    max_bytes_per_frame: usize,
}

impl OpusEncoder {
    pub fn new(bitrate: i32) -> Result<Self, CodecError> {
        let mut enc = coder::Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Audio)
            .map_err(|e| CodecError::OpusInit {
                source: Box::new(e),
            })?;
        enc.set_bitrate(Bitrate::BitsPerSecond(bitrate))
            .map_err(|e| CodecError::OpusInit {
                source: Box::new(e),
            })?;

        let max_bytes_per_frame = (bitrate as usize).div_ceil(8) / 50;
        Ok(Self {
            inner: enc,
            max_bytes_per_frame,
        })
    }

    pub fn encode(&mut self, input: &[f32], out: &mut BytesMut) -> Result<usize, CodecError> {
        if input.len() != FRAME_SAMPLES {
            return Err(CodecError::InvalidFrame {
                reason: format!("expected {FRAME_SAMPLES} samples, got {}", input.len()),
            });
        }
        out.clear();
        if out.capacity() < self.max_bytes_per_frame {
            out.reserve(self.max_bytes_per_frame - out.capacity());
        }
        out.resize(self.max_bytes_per_frame, 0);
        let n =
            self.inner
                .encode_float(input, &mut out[..])
                .map_err(|e| CodecError::OpusEncode {
                    source: Box::new(e),
                })?;
        out.truncate(n);
        Ok(n)
    }

    pub fn set_fec(&mut self, enable: bool, packet_loss_perc: u8) -> Result<(), CodecError> {
        self.inner
            .set_inband_fec(enable)
            .map_err(|e| CodecError::OpusInit {
                source: Box::new(e),
            })?;
        self.inner
            .set_packet_loss_perc(packet_loss_perc)
            .map_err(|e| CodecError::OpusInit {
                source: Box::new(e),
            })?;
        Ok(())
    }
}

pub struct OpusDecoder {
    inner: coder::Decoder,
}

impl OpusDecoder {
    pub fn new() -> Result<Self, CodecError> {
        let dec = coder::Decoder::new(SampleRate::Hz48000, Channels::Mono).map_err(|e| {
            CodecError::OpusInit {
                source: Box::new(e),
            }
        })?;
        Ok(Self { inner: dec })
    }

    pub fn decode(&mut self, input: Option<&[u8]>, out: &mut [f32]) -> Result<(), CodecError> {
        self.decode_with_fec(input, out, false)
    }

    pub fn decode_with_fec(
        &mut self,
        input: Option<&[u8]>,
        out: &mut [f32],
        use_fec: bool,
    ) -> Result<(), CodecError> {
        if out.len() < FRAME_SAMPLES {
            return Err(CodecError::InvalidFrame {
                reason: format!("output buffer must be at least {FRAME_SAMPLES}"),
            });
        }
        let pkt = match input {
            Some(bytes) => Some(Packet::try_from(bytes).map_err(|e| CodecError::OpusDecode {
                source: Box::new(e),
            })?),
            None => None,
        };
        let signals = MutSignals::try_from(&mut out[..FRAME_SAMPLES]).map_err(|e| {
            CodecError::OpusDecode {
                source: Box::new(e),
            }
        })?;
        let n = self
            .inner
            .decode_float(pkt, signals, use_fec)
            .map_err(|e| CodecError::OpusDecode {
                source: Box::new(e),
            })?;
        if n != FRAME_SAMPLES {
            return Err(CodecError::InvalidFrame {
                reason: format!("decoder produced {n} samples, expected {FRAME_SAMPLES}"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_frame(freq_hz: f32) -> Vec<f32> {
        (0..FRAME_SAMPLES)
            .map(|i| (2.0 * std::f32::consts::PI * freq_hz * (i as f32) / 48_000.0).sin() * 0.5)
            .collect()
    }

    #[test]
    fn encode_then_decode_recovers_signal_within_snr() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let mut dec = OpusDecoder::new().unwrap();

        let input = sine_frame(440.0);
        let mut payload = BytesMut::with_capacity(512);
        let n = enc.encode(&input, &mut payload).unwrap();
        assert!(
            n > 0 && n < 200,
            "expected typical opus 20ms@64kbps payload, got {n}"
        );

        let mut output = vec![0.0f32; FRAME_SAMPLES];
        dec.decode(Some(&payload[..n]), &mut output).unwrap();

        let in_energy: f32 = input.iter().map(|x| x * x).sum();
        let out_energy: f32 = output.iter().map(|x| x * x).sum();
        let ratio = out_energy / in_energy;
        assert!(
            ratio > 0.5 && ratio < 1.5,
            "energy ratio {ratio} out of plausible range"
        );
    }

    #[test]
    fn decode_plc_returns_silence_or_close() {
        let mut dec = OpusDecoder::new().unwrap();
        let mut output = vec![0.0f32; FRAME_SAMPLES];
        dec.decode(None, &mut output).unwrap();
        let energy: f32 = output.iter().map(|x| x * x).sum();
        assert!(energy < 1.0, "PLC frame should be low energy, got {energy}");
    }

    #[test]
    fn encode_rejects_wrong_frame_size() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let bad_input = vec![0.0f32; FRAME_SAMPLES + 1];
        let mut payload = BytesMut::with_capacity(512);
        let result = enc.encode(&bad_input, &mut payload);
        assert!(result.is_err());
    }

    #[test]
    fn encode_into_oversized_buffer_respects_byte_budget() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let input = sine_frame(440.0);

        let mut payload = BytesMut::with_capacity(4096);
        let n = enc.encode(&input, &mut payload).unwrap();

        assert!(
            n <= 200,
            "encode wrote {n} bytes, expected <= 200 for 64kbps/20ms"
        );
        assert_eq!(
            payload.len(),
            n,
            "payload length should match returned size after truncate"
        );
    }

    #[test]
    fn set_fec_enables_inband_fec_on_encoder() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        enc.set_fec(true, 5).unwrap();
        let input = sine_frame(440.0);
        let mut payload = BytesMut::with_capacity(512);
        let n = enc.encode(&input, &mut payload).unwrap();
        assert!(n > 0);
    }

    #[test]
    fn set_fec_disable_then_re_enable_round_trip() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        enc.set_fec(true, 10).unwrap();
        enc.set_fec(false, 0).unwrap();
        enc.set_fec(true, 3).unwrap();
    }

    #[test]
    fn decode_with_fec_true_against_real_packet_succeeds() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        enc.set_fec(true, 10).unwrap();
        let mut payload = BytesMut::with_capacity(512);
        let input = sine_frame(440.0);
        let _ = enc.encode(&input, &mut payload).unwrap();

        let mut dec = OpusDecoder::new().unwrap();
        let mut out = vec![0.0f32; FRAME_SAMPLES];
        dec.decode_with_fec(Some(&payload), &mut out, true).unwrap();
    }

    #[test]
    fn decode_with_fec_false_matches_legacy_decode() {
        let mut enc = OpusEncoder::new(64_000).unwrap();
        let mut payload = BytesMut::with_capacity(512);
        let input = sine_frame(880.0);
        enc.encode(&input, &mut payload).unwrap();

        let mut a = OpusDecoder::new().unwrap();
        let mut b = OpusDecoder::new().unwrap();
        let mut out_a = vec![0.0f32; FRAME_SAMPLES];
        let mut out_b = vec![0.0f32; FRAME_SAMPLES];
        a.decode(Some(&payload), &mut out_a).unwrap();
        b.decode_with_fec(Some(&payload), &mut out_b, false)
            .unwrap();
        assert_eq!(out_a, out_b);
    }
}
