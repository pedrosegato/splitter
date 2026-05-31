use crate::error::NetError;
use crate::net::stream::{Stream, StreamId, StreamState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn get(self) -> Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for SessionId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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
    state: SessionState,
    streams: HashMap<StreamId, Stream>,
    stream_id_counter: u8,
}

impl Session {
    pub fn new_outgoing(local: Uuid, remote: Uuid) -> Self {
        Self {
            id: SessionId::new(),
            local_peer_id: local,
            remote_peer_id: remote,
            state: SessionState::PendingOutgoing,
            streams: HashMap::new(),
            stream_id_counter: 0,
        }
    }

    pub fn new_incoming(id: SessionId, local: Uuid, remote: Uuid) -> Self {
        Self {
            id,
            local_peer_id: local,
            remote_peer_id: remote,
            state: SessionState::PendingIncoming,
            streams: HashMap::new(),
            stream_id_counter: 0,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn streams(&self) -> &HashMap<StreamId, Stream> {
        &self.streams
    }

    pub fn stream(&self, id: StreamId) -> Option<&Stream> {
        self.streams.get(&id)
    }

    pub fn stream_mut(&mut self, id: StreamId) -> Option<&mut Stream> {
        self.streams.get_mut(&id)
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

    pub fn remove_stream(&mut self, id: StreamId) {
        self.streams.remove(&id);
    }

    pub fn next_stream_id(&mut self) -> StreamId {
        let id = self.stream_id_counter;
        self.stream_id_counter = self.stream_id_counter.wrapping_add(1);
        StreamId(id)
    }

    pub fn active_stream_count(&self) -> usize {
        self.streams
            .values()
            .filter(|s| s.state() == StreamState::Active)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::signaling::{Codec, CodecParams, Endpoint};
    use crate::net::stream::StreamRoute;

    fn route() -> StreamRoute {
        StreamRoute::new(
            Endpoint {
                peer_id: "a".into(),
                device_id: "d-a".into(),
            },
            Endpoint {
                peer_id: "b".into(),
                device_id: "d-b".into(),
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
    fn outgoing_starts_pending() {
        let s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(s.state(), SessionState::PendingOutgoing);
    }

    #[test]
    fn accept_from_pending_outgoing_goes_active() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        assert_eq!(s.state(), SessionState::Active);
    }

    #[test]
    fn add_stream_requires_active() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        let stream = Stream::new_negotiating(StreamId(0), route(), 5004);
        let err = s.add_stream(stream).unwrap_err();
        assert!(matches!(err, NetError::SignalingProtocol { .. }));
    }

    #[test]
    fn add_stream_rejects_duplicate_id() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        s.add_stream(Stream::new_negotiating(StreamId(0), route(), 5004))
            .unwrap();
        let dup = Stream::new_negotiating(StreamId(0), route(), 5005);
        assert!(s.add_stream(dup).is_err());
    }

    #[test]
    fn close_propagates_to_streams() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        s.add_stream(Stream::new_negotiating(StreamId(0), route(), 5004))
            .unwrap();
        s.stream_mut(StreamId(0)).unwrap().activate().unwrap();
        s.close();
        assert_eq!(s.state(), SessionState::Closed);
        assert_eq!(s.stream(StreamId(0)).unwrap().state(), StreamState::Closed);
    }

    #[test]
    fn active_stream_count_reflects_state() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        s.add_stream(Stream::new_negotiating(StreamId(0), route(), 5004))
            .unwrap();
        s.add_stream(Stream::new_negotiating(StreamId(1), route(), 5005))
            .unwrap();
        s.stream_mut(StreamId(0)).unwrap().activate().unwrap();
        assert_eq!(s.active_stream_count(), 1);
    }

    #[test]
    fn stream_ids_are_monotonic_across_removal() {
        let mut s = Session::new_outgoing(Uuid::new_v4(), Uuid::new_v4());
        s.accept().unwrap();
        let id0 = s.next_stream_id();
        s.add_stream(Stream::new_negotiating(id0, route(), 5004))
            .unwrap();
        s.remove_stream(id0);
        let id1 = s.next_stream_id();
        assert_ne!(id0, id1);
    }
}
