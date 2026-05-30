use crate::error::NetError;
use crate::net::signaling::{CodecParams, Endpoint};
use serde::{Deserialize, Serialize};

pub type StreamId = u8;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum StreamState {
    Negotiating,
    Active,
    Paused,
    Error,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamRoute {
    pub source: Endpoint,
    pub sink: Endpoint,
    pub codec: CodecParams,
    pub volume: f32,
}

#[derive(Debug, Clone)]
pub struct Stream {
    pub id: StreamId,
    pub route: StreamRoute,
    pub udp_port: u16,
    pub state: StreamState,
}

impl Stream {
    pub fn new_negotiating(id: StreamId, route: StreamRoute, udp_port: u16) -> Self {
        Self {
            id,
            route,
            udp_port,
            state: StreamState::Negotiating,
        }
    }

    pub fn activate(&mut self) -> Result<(), NetError> {
        match self.state {
            StreamState::Negotiating | StreamState::Paused => {
                self.state = StreamState::Active;
                Ok(())
            }
            other => Err(NetError::SignalingProtocol {
                reason: format!("cannot activate from {other:?}"),
            }),
        }
    }

    pub fn pause(&mut self) -> Result<(), NetError> {
        if self.state == StreamState::Active {
            self.state = StreamState::Paused;
            Ok(())
        } else {
            Err(NetError::SignalingProtocol {
                reason: format!("cannot pause from {:?}", self.state),
            })
        }
    }

    pub fn close(&mut self) {
        self.state = StreamState::Closed;
    }

    pub fn fail(&mut self) {
        self.state = StreamState::Error;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.route.volume = volume.clamp(0.0, 2.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_route() -> StreamRoute {
        StreamRoute {
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
            volume: 1.0,
        }
    }

    #[test]
    fn new_starts_negotiating() {
        let s = Stream::new_negotiating(0, sample_route(), 5004);
        assert_eq!(s.state, StreamState::Negotiating);
    }

    #[test]
    fn activate_from_negotiating_goes_active() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        s.activate().unwrap();
        assert_eq!(s.state, StreamState::Active);
    }

    #[test]
    fn activate_from_closed_errors() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        s.close();
        let err = s.activate().unwrap_err();
        assert!(matches!(err, NetError::SignalingProtocol { .. }));
    }

    #[test]
    fn pause_then_resume_via_activate() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        s.activate().unwrap();
        s.pause().unwrap();
        assert_eq!(s.state, StreamState::Paused);
        s.activate().unwrap();
        assert_eq!(s.state, StreamState::Active);
    }

    #[test]
    fn pause_from_negotiating_errors() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        assert!(s.pause().is_err());
    }

    #[test]
    fn set_volume_clamps_into_range() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        s.set_volume(5.0);
        assert_eq!(s.route.volume, 2.0);
        s.set_volume(-1.0);
        assert_eq!(s.route.volume, 0.0);
    }
}
