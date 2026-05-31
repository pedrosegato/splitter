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

/// Validated volume in the range `[0.0, 2.0]`. NaN is mapped to `1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume(f32);

impl Volume {
    pub fn new(v: f32) -> Self {
        if v.is_nan() {
            Self(1.0)
        } else {
            Self(v.clamp(0.0, 2.0))
        }
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamRoute {
    pub source: Endpoint,
    pub sink: Endpoint,
    pub codec: CodecParams,
    volume: f32,
}

impl StreamRoute {
    pub fn new(source: Endpoint, sink: Endpoint, codec: CodecParams, volume: f32) -> Self {
        Self {
            source,
            sink,
            codec,
            volume: Volume::new(volume).get(),
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub(crate) fn set_volume(&mut self, v: f32) {
        self.volume = Volume::new(v).get();
    }
}

#[derive(Debug, Clone)]
pub struct Stream {
    pub id: StreamId,
    pub udp_port: u16,
    route: StreamRoute,
    state: StreamState,
    muted: bool,
}

impl Stream {
    pub fn new_negotiating(id: StreamId, route: StreamRoute, udp_port: u16) -> Self {
        Self {
            id,
            route,
            udp_port,
            state: StreamState::Negotiating,
            muted: false,
        }
    }

    pub fn state(&self) -> StreamState {
        self.state
    }

    pub fn route(&self) -> &StreamRoute {
        &self.route
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn volume(&self) -> f32 {
        self.route.volume()
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
        self.route.set_volume(volume);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::signaling::Codec;

    fn sample_route() -> StreamRoute {
        StreamRoute::new(
            Endpoint {
                peer_id: "a".into(),
                device_id: "dev-a".into(),
            },
            Endpoint {
                peer_id: "b".into(),
                device_id: "dev-b".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        )
    }

    #[test]
    fn new_starts_negotiating() {
        let s = Stream::new_negotiating(0, sample_route(), 5004);
        assert_eq!(s.state(), StreamState::Negotiating);
    }

    #[test]
    fn activate_from_negotiating_goes_active() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        s.activate().unwrap();
        assert_eq!(s.state(), StreamState::Active);
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
        assert_eq!(s.state(), StreamState::Paused);
        s.activate().unwrap();
        assert_eq!(s.state(), StreamState::Active);
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
        assert_eq!(s.volume(), 2.0);
        s.set_volume(-1.0);
        assert_eq!(s.volume(), 0.0);
    }

    #[test]
    fn volume_nan_maps_to_one() {
        let mut s = Stream::new_negotiating(0, sample_route(), 5004);
        s.set_volume(f32::NAN);
        assert_eq!(s.volume(), 1.0);
    }

    #[test]
    fn volume_newtype_clamps_and_nan() {
        assert_eq!(Volume::new(5.0).get(), 2.0);
        assert_eq!(Volume::new(-1.0).get(), 0.0);
        assert_eq!(Volume::new(f32::NAN).get(), 1.0);
        assert_eq!(Volume::new(1.5).get(), 1.5);
    }
}
