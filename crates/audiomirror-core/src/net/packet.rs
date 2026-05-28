use crate::error::NetError;
use bytes::{Buf, BufMut, Bytes, BytesMut};

pub const HEADER_LEN: usize = 10;
pub const MAX_PACKET_LEN: usize = 1500;

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub stream_id: u8,
    pub seq: u32,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}

impl Packet {
    pub fn encode(&self, out: &mut BytesMut) -> Result<usize, NetError> {
        let total = HEADER_LEN + self.payload.len();
        if total > MAX_PACKET_LEN {
            return Err(NetError::PayloadLenMismatch {
                declared: self.payload.len(),
                available: MAX_PACKET_LEN - HEADER_LEN,
            });
        }
        if self.seq > 0xFF_FFFF {
            return Err(NetError::SeqOverflow { seq: self.seq });
        }
        out.clear();
        out.reserve(total);
        out.put_u8(self.stream_id);
        out.put_u8(((self.seq >> 16) & 0xFF) as u8);
        out.put_u8(((self.seq >> 8) & 0xFF) as u8);
        out.put_u8((self.seq & 0xFF) as u8);
        out.put_u32(self.timestamp_ms);
        out.put_u16(self.payload.len() as u16);
        out.put_slice(&self.payload);
        Ok(total)
    }

    pub fn decode(mut buf: Bytes) -> Result<Self, NetError> {
        if buf.len() < HEADER_LEN {
            return Err(NetError::HeaderTruncated {
                got: buf.len(),
                need: HEADER_LEN,
            });
        }
        let stream_id = buf.get_u8();
        let seq_hi = buf.get_u8() as u32;
        let seq_md = buf.get_u8() as u32;
        let seq_lo = buf.get_u8() as u32;
        let seq = (seq_hi << 16) | (seq_md << 8) | seq_lo;
        let timestamp_ms = buf.get_u32();
        let payload_len = buf.get_u16() as usize;
        if buf.len() < payload_len {
            return Err(NetError::PayloadLenMismatch {
                declared: payload_len,
                available: buf.len(),
            });
        }
        let payload = buf.slice(..payload_len);
        Ok(Self {
            stream_id,
            seq,
            timestamp_ms,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn encode_decode_roundtrip_simple() {
        let pkt = Packet {
            stream_id: 7,
            seq: 0x123_456,
            timestamp_ms: 0xDEAD_BEEF,
            payload: Bytes::from_static(b"hello opus"),
        };
        let mut buf = BytesMut::with_capacity(64);
        let n = pkt.encode(&mut buf).unwrap();
        assert_eq!(n, HEADER_LEN + 10);
        let bytes = buf.freeze();
        let decoded = Packet::decode(bytes).unwrap();
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn decode_too_short_errors() {
        let bytes = Bytes::from_static(b"\x01\x02");
        let err = Packet::decode(bytes).unwrap_err();
        assert!(matches!(err, NetError::HeaderTruncated { .. }));
    }

    #[test]
    fn decode_payload_len_mismatch_errors() {
        let mut buf = BytesMut::with_capacity(HEADER_LEN);
        buf.put_u8(0);
        buf.put_slice(&[0, 0, 0]);
        buf.put_u32(0);
        buf.put_u16(100);
        let err = Packet::decode(buf.freeze()).unwrap_err();
        assert!(matches!(err, NetError::PayloadLenMismatch { .. }));
    }

    #[test]
    fn encode_seq_overflow_errors() {
        let pkt = Packet {
            stream_id: 0,
            seq: 0x0100_0000,
            timestamp_ms: 0,
            payload: Bytes::from_static(b"x"),
        };
        let mut buf = BytesMut::with_capacity(64);
        let err = pkt.encode(&mut buf).unwrap_err();
        assert!(matches!(err, NetError::SeqOverflow { .. }));
    }

    #[test]
    fn encode_too_large_errors() {
        let pkt = Packet {
            stream_id: 0,
            seq: 0,
            timestamp_ms: 0,
            payload: Bytes::from(vec![0u8; MAX_PACKET_LEN]),
        };
        let mut buf = BytesMut::with_capacity(MAX_PACKET_LEN * 2);
        let err = pkt.encode(&mut buf).unwrap_err();
        assert!(matches!(err, NetError::PayloadLenMismatch { .. }));
    }

    proptest! {
        #[test]
        fn proptest_roundtrip(
            stream_id in 0u8..=255,
            seq in 0u32..0xFF_FFFF,
            ts in 0u32..u32::MAX,
            payload_data in proptest::collection::vec(any::<u8>(), 0..1400),
        ) {
            let pkt = Packet { stream_id, seq, timestamp_ms: ts, payload: Bytes::from(payload_data) };
            let mut buf = BytesMut::with_capacity(MAX_PACKET_LEN + 8);
            let _ = pkt.encode(&mut buf).unwrap();
            let decoded = Packet::decode(buf.freeze()).unwrap();
            prop_assert_eq!(decoded, pkt);
        }
    }
}
