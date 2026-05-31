use crate::audio::devices::DeviceKind;
use crate::error::NetError;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct DeviceDescriptor {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceKind {
    Mic { device_id: String },
    System,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamAction {
    Pause,
    Resume,
    Close,
    SetVolume { volume: f32 },
    SetMuted { muted: bool },
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_token: Option<String>,
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
    },
    Heartbeat {
        timestamp_ms: u64,
        streams_stats: Vec<HeartbeatStreamStats>,
    },
    DeviceListRequest {},
    DeviceListResponse {
        devices: Vec<DeviceDescriptor>,
    },
    PeerRenamed {
        peer_id: String,
        peer_name: String,
    },
    StreamRequest {
        session_id: String,
        source: SourceKind,
        sink_device: String,
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
    fn hello_ack_with_token_round_trips() {
        let msg = SignalingMessage::HelloAck {
            accepted: true,
            reason: None,
            auth_token: Some("secret-tok".into()),
        };
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
        let raw = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(raw.contains("\"auth_token\":\"secret-tok\""));
    }

    #[test]
    fn hello_ack_without_token_omits_field() {
        let msg = SignalingMessage::HelloAck {
            accepted: false,
            reason: Some("rejected".into()),
            auth_token: None,
        };
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(
            !raw.contains("auth_token"),
            "auth_token must be absent when None"
        );
        let back = SignalingMessage::decode_from_slice(raw.as_bytes()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn hello_ack_without_token_field_decodes_as_none() {
        let legacy = br#"{"type":"hello_ack","accepted":true}"#;
        let msg = SignalingMessage::decode_from_slice(legacy).unwrap();
        assert!(
            matches!(
                msg,
                SignalingMessage::HelloAck {
                    auth_token: None,
                    ..
                }
            ),
            "older HelloAck without auth_token must decode with auth_token=None"
        );
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
    fn stream_control_set_volume_carries_volume_in_variant() {
        let msg = SignalingMessage::StreamControl {
            stream_id: 0,
            action: StreamAction::SetVolume { volume: 0.5 },
        };
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"set_volume\""));
        assert!(raw.contains("\"volume\":0.5"));
        let back = SignalingMessage::decode_from_slice(raw.as_bytes()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn stream_control_pause_has_no_volume() {
        let msg = SignalingMessage::StreamControl {
            stream_id: 3,
            action: StreamAction::Pause,
        };
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"pause\""));
        assert!(!raw.contains("volume"));
        let back = SignalingMessage::decode_from_slice(raw.as_bytes()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn stream_control_set_muted_carries_muted_in_variant() {
        let msg = SignalingMessage::StreamControl {
            stream_id: 1,
            action: StreamAction::SetMuted { muted: true },
        };
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"set_muted\""));
        assert!(raw.contains("\"muted\":true"));
        let back = SignalingMessage::decode_from_slice(raw.as_bytes()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn stream_request_round_trips_with_system_source() {
        let msg = SignalingMessage::StreamRequest {
            session_id: "sess-1".into(),
            source: SourceKind::System,
            sink_device: "dev-b".into(),
        };
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
        let raw = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"system\""));
    }

    #[test]
    fn stream_request_round_trips_with_mic_source() {
        let msg = SignalingMessage::StreamRequest {
            session_id: "sess-1".into(),
            source: SourceKind::Mic {
                device_id: "Input:0:Built-in".into(),
            },
            sink_device: "dev-b".into(),
        };
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
        let raw = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"mic\""));
        assert!(raw.contains("\"device_id\":\"Input:0:Built-in\""));
    }

    #[test]
    fn device_descriptor_round_trips_each_kind() {
        for kind in [
            DeviceKind::Input,
            DeviceKind::Output,
            DeviceKind::SystemAudio,
        ] {
            let msg = SignalingMessage::DeviceListResponse {
                devices: vec![DeviceDescriptor {
                    id: "id".into(),
                    name: "name".into(),
                    kind,
                }],
            };
            let bytes = msg.encode_to_bytes().unwrap();
            let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn device_kind_serializes_to_pascal_case_strings() {
        let msg = SignalingMessage::DeviceListResponse {
            devices: vec![
                DeviceDescriptor {
                    id: "a".into(),
                    name: "a".into(),
                    kind: DeviceKind::Input,
                },
                DeviceDescriptor {
                    id: "b".into(),
                    name: "b".into(),
                    kind: DeviceKind::SystemAudio,
                },
            ],
        };
        let raw = String::from_utf8(msg.encode_to_bytes().unwrap().to_vec()).unwrap();
        assert!(raw.contains("\"kind\":\"Input\""));
        assert!(raw.contains("\"kind\":\"SystemAudio\""));
    }

    #[test]
    fn peer_renamed_round_trip() {
        let msg = SignalingMessage::PeerRenamed {
            peer_id: "11111111-1111-1111-1111-111111111111".into(),
            peer_name: "Novo Nome".into(),
        };
        let bytes = msg.encode_to_bytes().unwrap();
        let back = SignalingMessage::decode_from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
        let raw = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(raw.contains("\"type\":\"peer_renamed\""));
    }
}
