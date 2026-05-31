#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum CoreError {
    #[error("audio: {0}")]
    Audio(#[from] AudioError),
    #[error("net: {0}")]
    Net(#[from] NetError),
    #[error("codec: {0}")]
    Codec(#[from] CodecError),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum AudioError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("loopback not available on this platform")]
    LoopbackUnsupported,
    #[error("Screen Recording permission denied; enable in System Settings → Privacy & Security → Screen Recording, then relaunch")]
    ScreenRecordingPermissionDenied,
    #[error("cpal default device missing")]
    NoDefaultDevice,
    #[error("cpal build stream failed")]
    BuildStream {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("cpal play stream failed")]
    PlayStream {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("priority promotion failed")]
    PriorityPromotion {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum NetError {
    #[error("packet header truncated: got {got} bytes, need at least {need}")]
    HeaderTruncated { got: usize, need: usize },

    #[error("packet payload length mismatch: header says {declared}, buffer has {available}")]
    PayloadLenMismatch { declared: usize, available: usize },

    #[error("seq {seq} exceeds u24 max (0xFF_FFFF)")]
    SeqOverflow { seq: u32 },

    #[error("udp io: {0}")]
    UdpIo(#[from] std::io::Error),

    #[error("handshake failed: {reason}")]
    Handshake { reason: String },

    #[error("peer {peer_id} rejected: {reason}")]
    PeerRejected { peer_id: String, reason: String },

    #[error("signaling protocol error: {reason}")]
    SignalingProtocol { reason: String },

    #[error("timeout waiting for {what} after {millis}ms")]
    Timeout { what: String, millis: u64 },

    #[error("mdns: {reason}")]
    Mdns { reason: String },

    #[error("config io: {0}")]
    ConfigIo(String),

    /// Session lookup failed; session_id is a Uuid (alias SessionId avoids a dep cycle).
    #[error("unknown session {0}")]
    UnknownSession(uuid::Uuid),

    /// Stream lookup failed within a known session.
    #[error("unknown stream {stream} in session {session}")]
    UnknownStream { session: uuid::Uuid, stream: u8 },

    #[error("channel closed")]
    ChannelClosed,

    #[error("udp bind failed: {0}")]
    UdpBind(String),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum CodecError {
    #[error("opus init failed")]
    OpusInit {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("opus encode failed")]
    OpusEncode {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("opus decode failed")]
    OpusDecode {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("invalid frame: {reason}")]
    InvalidFrame { reason: String },
    #[error("resampler error")]
    Resampler {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_error_display() {
        let e = AudioError::DeviceNotFound("hw:0".to_string());
        assert_eq!(e.to_string(), "device not found: hw:0");
    }

    #[test]
    fn audio_error_into_core_error() {
        let e: CoreError = AudioError::LoopbackUnsupported.into();
        assert!(matches!(
            e,
            CoreError::Audio(AudioError::LoopbackUnsupported)
        ));
    }

    #[test]
    fn codec_error_display() {
        let e = CodecError::InvalidFrame {
            reason: "len mismatch".into(),
        };
        assert!(e.to_string().contains("len mismatch"));
    }

    #[test]
    fn net_error_display() {
        let e = NetError::HeaderTruncated { got: 4, need: 10 };
        assert!(e.to_string().contains("4"));
        assert!(e.to_string().contains("10"));
    }

    #[test]
    fn question_mark_propagates_audio_to_core() {
        fn inner() -> Result<(), CoreError> {
            Err::<(), _>(AudioError::LoopbackUnsupported)?;
            Ok(())
        }
        let err = inner().unwrap_err();
        assert!(matches!(
            err,
            CoreError::Audio(AudioError::LoopbackUnsupported)
        ));
    }

    #[test]
    fn question_mark_propagates_io_to_net_to_core() {
        fn inner() -> Result<(), CoreError> {
            Err::<(), _>(NetError::from(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "test",
            )))?;
            Ok(())
        }
        let err = inner().unwrap_err();
        assert!(matches!(err, CoreError::Net(NetError::UdpIo(_))));
    }

    #[test]
    fn build_stream_preserves_source_chain() {
        use std::error::Error;
        let upstream = std::io::Error::other("cpal said no");
        let e = AudioError::BuildStream {
            source: Box::new(upstream),
        };

        let source = e.source().expect("BuildStream should expose source");
        assert!(source.to_string().contains("cpal said no"));
    }

    #[test]
    fn screen_recording_permission_denied_displays_hint() {
        let e = AudioError::ScreenRecordingPermissionDenied;
        let msg = e.to_string();
        assert!(msg.contains("Screen Recording"));
        assert!(msg.contains("System Settings"));
    }

    #[test]
    fn net_error_handshake_display_mentions_peer() {
        let e = NetError::Handshake {
            reason: "version mismatch".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("handshake"));
        assert!(msg.contains("version mismatch"));
    }

    #[test]
    fn net_error_peer_rejected_display() {
        let e = NetError::PeerRejected {
            peer_id: "abc".into(),
            reason: "untrusted".into(),
        };
        assert!(e.to_string().contains("abc"));
        assert!(e.to_string().contains("untrusted"));
    }

    #[test]
    fn net_error_signaling_protocol_display() {
        let e = NetError::SignalingProtocol {
            reason: "unknown type".into(),
        };
        assert!(e.to_string().contains("signaling"));
    }

    #[test]
    fn net_error_timeout_display() {
        let e = NetError::Timeout {
            what: "hello_ack".into(),
            millis: 5_000,
        };
        let msg = e.to_string();
        assert!(msg.contains("hello_ack"));
        assert!(msg.contains("5000"));
    }

    #[test]
    fn net_error_config_io_propagates_io() {
        fn inner() -> Result<(), NetError> {
            Err(NetError::from(std::io::Error::other("disk full")))
        }
        assert!(matches!(inner().unwrap_err(), NetError::UdpIo(_)));
    }

    #[test]
    fn net_error_unknown_session_display() {
        let id = uuid::Uuid::nil();
        let e = NetError::UnknownSession(id);
        assert!(e.to_string().contains("unknown session"));
        assert!(e.to_string().contains(&id.to_string()));
    }

    #[test]
    fn net_error_unknown_stream_display() {
        let session = uuid::Uuid::nil();
        let e = NetError::UnknownStream { session, stream: 3 };
        assert!(e.to_string().contains("unknown stream 3 in session"));
    }

    #[test]
    fn net_error_channel_closed_display() {
        let e = NetError::ChannelClosed;
        assert_eq!(e.to_string(), "channel closed");
    }

    #[test]
    fn net_error_udp_bind_display() {
        let e = NetError::UdpBind("address in use".into());
        assert!(e.to_string().contains("udp bind failed"));
        assert!(e.to_string().contains("address in use"));
    }
}
