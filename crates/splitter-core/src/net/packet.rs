use crate::error::NetError;
use crate::net::crypto::{PacketOpener, PacketSealer};
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacketView<'a> {
    pub stream_id: u8,
    pub seq: u32,
    pub timestamp_ms: u32,
    pub payload: &'a [u8],
}

impl Packet {
    pub fn encode_from_parts(
        stream_id: u8,
        seq: u32,
        timestamp_ms: u32,
        payload: &[u8],
        out: &mut BytesMut,
    ) -> Result<usize, NetError> {
        let total = HEADER_LEN + payload.len();
        if total > MAX_PACKET_LEN {
            return Err(NetError::PayloadLenMismatch {
                declared: payload.len(),
                available: MAX_PACKET_LEN - HEADER_LEN,
            });
        }
        if seq > 0xFF_FFFF {
            return Err(NetError::SeqOverflow { seq });
        }
        out.clear();
        out.reserve(total);
        out.put_u8(stream_id);
        out.put_u8(((seq >> 16) & 0xFF) as u8);
        out.put_u8(((seq >> 8) & 0xFF) as u8);
        out.put_u8((seq & 0xFF) as u8);
        out.put_u32(timestamp_ms);
        out.put_u16(payload.len() as u16);
        out.put_slice(payload);
        Ok(total)
    }

    pub fn encode(&self, out: &mut BytesMut) -> Result<usize, NetError> {
        Self::encode_from_parts(
            self.stream_id,
            self.seq,
            self.timestamp_ms,
            &self.payload,
            out,
        )
    }

    pub fn decode_ref(buf: &[u8]) -> Result<PacketView<'_>, NetError> {
        if buf.len() < HEADER_LEN {
            return Err(NetError::HeaderTruncated {
                got: buf.len(),
                need: HEADER_LEN,
            });
        }
        let stream_id = buf[0];
        let seq = ((buf[1] as u32) << 16) | ((buf[2] as u32) << 8) | buf[3] as u32;
        let timestamp_ms = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let payload_len = u16::from_be_bytes([buf[8], buf[9]]) as usize;
        let rest = &buf[HEADER_LEN..];
        if rest.len() < payload_len {
            return Err(NetError::PayloadLenMismatch {
                declared: payload_len,
                available: rest.len(),
            });
        }
        Ok(PacketView {
            stream_id,
            seq,
            timestamp_ms,
            payload: &rest[..payload_len],
        })
    }

    /// What an encrypted payload is bound to, so a rewritten seq or timestamp fails to open.
    pub fn header_aad(stream_id: u8, seq: u32, timestamp_ms: u32) -> [u8; 8] {
        let mut aad = [0u8; 8];
        aad[0] = stream_id;
        aad[1..4].copy_from_slice(&seq.to_be_bytes()[1..]);
        aad[4..].copy_from_slice(&timestamp_ms.to_be_bytes());
        aad
    }

    pub fn open_payload(self, opener: &mut PacketOpener) -> Option<Self> {
        let aad = Self::header_aad(self.stream_id, self.seq, self.timestamp_ms);
        let payload = opener.open(&aad, &self.payload)?;
        Some(Self {
            payload: payload.into(),
            ..self
        })
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

/// Sink-to-source feedback about once a second (like an RTCP receiver report). The echoed
/// timestamp and hold time let the source measure RTT on its own clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverReport {
    pub stream_id: u8,
    pub loss_pct: u8,
    pub echo_timestamp_ms: u32,
    pub hold_ms: u32,
}

const REPORT_TAG: u8 = 0xA5;
const REPORT_LEN: usize = 11;
const REPORT_HEADER_LEN: usize = 2;

impl ReceiverReport {
    pub fn encode(&self) -> [u8; REPORT_LEN] {
        let mut out = [0u8; REPORT_LEN];
        out[0] = REPORT_TAG;
        out[1] = self.stream_id;
        out[2] = self.loss_pct;
        out[3..7].copy_from_slice(&self.echo_timestamp_ms.to_be_bytes());
        out[7..11].copy_from_slice(&self.hold_ms.to_be_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() != REPORT_LEN || buf[0] != REPORT_TAG || buf[2] > 100 {
            return None;
        }
        Some(Self {
            stream_id: buf[1],
            loss_pct: buf[2],
            echo_timestamp_ms: u32::from_be_bytes([buf[3], buf[4], buf[5], buf[6]]),
            hold_ms: u32::from_be_bytes([buf[7], buf[8], buf[9], buf[10]]),
        })
    }

    /// Tag and stream id stay readable (and authenticated); the measurements are sealed.
    pub fn seal(&self, sealer: &mut PacketSealer) -> Vec<u8> {
        let plain = self.encode();
        let (header, body) = plain.split_at(REPORT_HEADER_LEN);
        [header, &sealer.seal(header, body)].concat()
    }

    pub fn open(buf: &[u8], opener: &mut PacketOpener) -> Option<Self> {
        let (header, sealed) = buf.split_at_checked(REPORT_HEADER_LEN)?;
        let body = opener.open(header, sealed)?;
        Self::decode(&[header, &body].concat())
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

    #[test]
    fn encode_from_parts_matches_struct_encode() {
        let pkt = Packet {
            stream_id: 7,
            seq: 0x123_456,
            timestamp_ms: 0xDEAD_BEEF,
            payload: Bytes::from_static(b"hello opus"),
        };
        let mut buf_a = BytesMut::with_capacity(64);
        pkt.encode(&mut buf_a).unwrap();
        let mut buf_b = BytesMut::with_capacity(64);
        Packet::encode_from_parts(
            pkt.stream_id,
            pkt.seq,
            pkt.timestamp_ms,
            &pkt.payload,
            &mut buf_b,
        )
        .unwrap();
        assert_eq!(buf_a, buf_b);
        assert_eq!(Packet::decode(buf_a.freeze()).unwrap(), pkt);
        assert_eq!(Packet::decode(buf_b.freeze()).unwrap(), pkt);
    }

    #[test]
    fn decode_ref_matches_decode() {
        let pkt = Packet {
            stream_id: 3,
            seq: 0x0A_BCDE,
            timestamp_ms: 0x1234_5678,
            payload: Bytes::from_static(b"payload bytes"),
        };
        let mut buf = BytesMut::with_capacity(64);
        pkt.encode(&mut buf).unwrap();
        let owned = Packet::decode(buf.clone().freeze()).unwrap();
        let view = Packet::decode_ref(&buf[..]).unwrap();
        assert_eq!(view.stream_id, owned.stream_id);
        assert_eq!(view.seq, owned.seq);
        assert_eq!(view.timestamp_ms, owned.timestamp_ms);
        assert_eq!(view.payload, &owned.payload[..]);
    }

    #[test]
    fn decode_ref_too_short_errors() {
        let err = Packet::decode_ref(b"\x01\x02").unwrap_err();
        assert!(matches!(err, NetError::HeaderTruncated { .. }));
    }

    #[test]
    fn decode_ref_payload_len_mismatch_errors() {
        let mut buf = BytesMut::with_capacity(HEADER_LEN);
        buf.put_u8(0);
        buf.put_slice(&[0, 0, 0]);
        buf.put_u32(0);
        buf.put_u16(100);
        let err = Packet::decode_ref(&buf[..]).unwrap_err();
        assert!(matches!(err, NetError::PayloadLenMismatch { .. }));
    }

    #[test]
    fn encode_from_parts_seq_overflow_errors() {
        let mut buf = BytesMut::with_capacity(64);
        let err = Packet::encode_from_parts(0, 0x0100_0000, 0, b"x", &mut buf).unwrap_err();
        assert!(matches!(err, NetError::SeqOverflow { .. }));
    }

    #[test]
    fn encode_from_parts_too_large_errors() {
        let payload = vec![0u8; MAX_PACKET_LEN];
        let mut buf = BytesMut::with_capacity(MAX_PACKET_LEN * 2);
        let err = Packet::encode_from_parts(0, 0, 0, &payload, &mut buf).unwrap_err();
        assert!(matches!(err, NetError::PayloadLenMismatch { .. }));
    }

    #[test]
    fn sealed_payload_opens_only_with_its_original_header() {
        let key = [5u8; 32];
        let mut sealer = PacketSealer::new(&key);
        let sealed = sealer.seal(&Packet::header_aad(5, 10, 100), b"opus");
        let pkt = Packet {
            stream_id: 5,
            seq: 10,
            timestamp_ms: 100,
            payload: Bytes::from(sealed),
        };
        let forged = Packet {
            seq: 11,
            ..pkt.clone()
        };

        assert!(forged.open_payload(&mut PacketOpener::new(&key)).is_none());
        let opened = pkt.open_payload(&mut PacketOpener::new(&key)).unwrap();
        assert_eq!(&opened.payload[..], b"opus");
        assert_eq!((opened.stream_id, opened.seq), (5, 10));
    }

    #[test]
    fn sealed_report_round_trips_and_plaintext_is_refused() {
        let key = [6u8; 32];
        let report = ReceiverReport {
            stream_id: 3,
            loss_pct: 12,
            echo_timestamp_ms: 99,
            hold_ms: 7,
        };
        let sealed = report.seal(&mut PacketSealer::new(&key));
        let mut opener = PacketOpener::new(&key);

        assert_eq!(ReceiverReport::open(&sealed, &mut opener), Some(report));
        assert_eq!(ReceiverReport::open(&report.encode(), &mut opener), None);
        assert_eq!(ReceiverReport::decode(&sealed), None);
    }

    #[test]
    fn receiver_report_round_trips() {
        let report = ReceiverReport {
            stream_id: 3,
            loss_pct: 12,
            echo_timestamp_ms: 0xDEAD_BEEF,
            hold_ms: 250,
        };
        assert_eq!(ReceiverReport::decode(&report.encode()), Some(report));
    }

    #[test]
    fn receiver_report_rejects_audio_packets_and_junk() {
        let mut audio = BytesMut::with_capacity(64);
        Packet::encode_from_parts(0, 1, 2, b"opus", &mut audio).unwrap();
        assert_eq!(ReceiverReport::decode(&audio), None);
        assert_eq!(ReceiverReport::decode(b"\xA5\x01"), None);
        assert_eq!(ReceiverReport::decode(&[0u8; 12]), None);
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
            let decoded = Packet::decode(buf.clone().freeze()).unwrap();
            prop_assert_eq!(&decoded, &pkt);

            let mut buf_parts = BytesMut::with_capacity(MAX_PACKET_LEN + 8);
            Packet::encode_from_parts(
                pkt.stream_id,
                pkt.seq,
                pkt.timestamp_ms,
                &pkt.payload,
                &mut buf_parts,
            )
            .unwrap();
            prop_assert_eq!(&buf, &buf_parts);

            let view = Packet::decode_ref(&buf[..]).unwrap();
            prop_assert_eq!(view.stream_id, decoded.stream_id);
            prop_assert_eq!(view.seq, decoded.seq);
            prop_assert_eq!(view.timestamp_ms, decoded.timestamp_ms);
            prop_assert_eq!(view.payload, &decoded.payload[..]);
        }
    }
}
