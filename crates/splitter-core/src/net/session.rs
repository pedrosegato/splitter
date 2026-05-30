use crate::error::NetError;
use crate::net::stream::{Stream, StreamId, StreamState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub type SessionId = Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    PendingOutgoing,
    PendingIncoming,
    Active,
    Closed,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: SessionId,
    pub local_peer_id: Uuid,
    pub remote_peer_id: Uuid,
    pub state: SessionState,
    pub streams: HashMap<StreamId, Stream>,
}

impl Session {
    pub fn new_outgoing(local: Uuid, remote: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            local_peer_id: local,
            remote_peer_id: remote,
            state: SessionState::PendingOutgoing,
            streams: HashMap::new(),
        }
    }

    pub fn new_incoming(id: SessionId, local: Uuid, remote: Uuid) -> Self {
        Self {
            id,
            local_peer_id: local,
            remote_peer_id: remote,
            state: SessionState::PendingIncoming,
            streams: HashMap::new(),
        }
    }

    pub fn accept(&mut self) -> Result<(), NetError> {
        match self.state {
            SessionState::PendingOutgoing | SessionState::PendingIncoming => {
                self.state = SessionState::Active;
                Ok(())
            }
            other => Err(NetError::SignalingProtocol {
                reason: format!("cannot accept from {other:?}"),
            }),
        }
    }

    pub fn close(&mut self) {
        self.state = SessionState::Closed;
        for s in self.streams.values_mut() {
            s.close();
        }
    }

    pub fn add_stream(&mut self, stream: Stream) -> Result<(), NetError> {
        if self.state != SessionState::Active {
            return Err(NetError::SignalingProtocol {
                reason: format!("session not active (state={:?})", self.state),
            });
        }
        if self.streams.contains_key(&stream.id) {
            return Err(NetError::SignalingProtocol {
                reason: format!("stream_id {} already exists", stream.id),
            });
        }
        self.streams.insert(stream.id, stream);
        Ok(())
    }

    pub fn active_stream_count(&self) -> usize {
        self.streams
            .values()
            .filter(|s| s.state == StreamState::Active)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::signaling::{CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;

    fn route() -> StreamRoute {
        StreamRoute {
            source: Endpoint {
                peer_id: "a".into(),
                device_id: "d-a".into(),
            },
            sink: Endpoint {
                peer_id: "b".into(),
                device_id: "d-b".into(),
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
    fn outgoing_starts_pending() {
        let s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(s.state, SessionState::PendingOutgoing);
    }

    #[test]
    fn accept_from_pending_outgoing_goes_active() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        assert_eq!(s.state, SessionState::Active);
    }

    #[test]
    fn add_stream_requires_active() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        let stream = Stream::new_negotiating(0, route(), 5004);
        let err = s.add_stream(stream).unwrap_err();
        assert!(matches!(err, NetError::SignalingProtocol { .. }));
    }

    #[test]
    fn add_stream_rejects_duplicate_id() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        s.add_stream(Stream::new_negotiating(0, route(), 5004))
            .unwrap();
        let dup = Stream::new_negotiating(0, route(), 5005);
        assert!(s.add_stream(dup).is_err());
    }

    #[test]
    fn close_propagates_to_streams() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        s.add_stream(Stream::new_negotiating(0, route(), 5004))
            .unwrap();
        s.streams.get_mut(&0).unwrap().activate().unwrap();
        s.close();
        assert_eq!(s.state, SessionState::Closed);
        assert_eq!(s.streams[&0].state, StreamState::Closed);
    }

    #[test]
    fn active_stream_count_reflects_state() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        s.add_stream(Stream::new_negotiating(0, route(), 5004))
            .unwrap();
        s.add_stream(Stream::new_negotiating(1, route(), 5005))
            .unwrap();
        s.streams.get_mut(&0).unwrap().activate().unwrap();
        assert_eq!(s.active_stream_count(), 1);
    }
}
