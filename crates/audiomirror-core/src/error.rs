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
    #[error("device unavailable: {0}")]
    DeviceUnavailable(String),
    #[error("unsupported sample rate {requested} on device {device}")]
    UnsupportedSampleRate { device: String, requested: u32 },
    #[error("unsupported channel count {requested} on device {device}")]
    UnsupportedChannels { device: String, requested: u16 },
    #[error("loopback not available on this platform")]
    LoopbackUnsupported,
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
}
