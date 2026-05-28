use crate::error::NetError;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    pub codecs: Vec<String>,
    pub max_streams: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub peer_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodecParams {
    pub name: String,
    pub bitrate: i32,
    pub frame_ms: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamAction {
    Pause,
    Resume,
    Close,
    SetVolume,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeartbeatStreamStats {
    pub stream_id: u8,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalingMessage {
    Hello {
        protocol_version: u32,
        peer_id: String,
        peer_name: String,
        app_version: String,
        capabilities: Capabilities,
        auth_token: String,
    },
    HelloAck {
        accepted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    SessionRequest {
        session_id: String,
        requested_by: String,
    },
    SessionResponse {
        session_id: String,
        accepted: bool,
    },
    StreamOpen {
        session_id: String,
        stream_id: u8,
        source: Endpoint,
        sink: Endpoint,
        codec: CodecParams,
        udp_port: u16,
    },
    StreamOpenAck {
        stream_id: u8,
        accepted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        udp_port: Option<u16>,
    },
    StreamControl {
        stream_id: u8,
        action: StreamAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        volume: Option<f32>,
    },
    Heartbeat {
        timestamp_ms: u64,
        streams_stats: Vec<HeartbeatStreamStats>,
    },
}

impl SignalingMessage {
    pub fn encode_to_bytes(&self) -> Result<Bytes, NetError> {
        let vec = serde_json::to_vec(self).map_err(|e| NetError::SignalingProtocol {
            reason: format!("encode: {e}"),
        })?;
        Ok(Bytes::from(vec))
    }

    pub fn decode_from_slice(buf: &[u8]) -> Result<Self, NetError> {
        serde_json::from_slice(buf).map_err(|e| NetError::SignalingProtocol {
            reason: format!("decode: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hello() -> SignalingMessage {
        SignalingMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            peer_id: "peer-a".into(),
            peer_name: "Mac".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            capabilities: Capabilities {
                codecs: vec!["opus".into()],
                max_streams: 4,
            },
            auth_token: "tok".into(),
        }
    }

    #[test]
    fn hello_round_trip() {
        let msg = sample_hello();
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn tag_is_lowercase_with_underscores() {
        let msg = sample_hello();
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"hello\""));
    }

    #[test]
    fn unknown_type_field_yields_protocol_error() {
        let bad = br#"{"type":"made_up","x":1}"#;
        let err = SignalingMessage::decode_from_slice(bad).unwrap_err();
        assert!(matches!(err, NetError::SignalingProtocol { .. }));
    }

    #[test]
    fn stream_open_round_trip_preserves_codec_and_port() {
        let msg = SignalingMessage::StreamOpen {
            session_id: "sess-1".into(),
            stream_id: 7,
            source: Endpoint {
                peer_id: "a".into(),
                device_id: "dev-a".into(),
            },
            sink: Endpoint {
                peer_id: "b".into(),
                device_id: "dev-b".into(),
            },
            codec: CodecParams {
                name: "opus".into(),
                bitrate: 64_000,
                frame_ms: 20,
            },
            udp_port: 5004,
        };
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn heartbeat_round_trip_with_optional_rtt() {
        let msg = SignalingMessage::Heartbeat {
            timestamp_ms: 1_234_567,
            streams_stats: vec![HeartbeatStreamStats {
                stream_id: 1,
                packets_sent: 100,
                packets_received: 90,
                packets_lost: 10,
                rtt_ms: Some(42),
            }],
        };
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn stream_control_set_volume_serializes_snake_case() {
        let msg = SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::SetVolume,
            volume: Some(0.5),
        };
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(raw.contains("\"action\":\"set_volume\""));
    }
}
