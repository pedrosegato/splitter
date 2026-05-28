use crate::error::NetError;
use crate::net::session::{Session, SessionId, SessionState};
use crate::net::stream::{Stream, StreamId, StreamState};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct StreamSnapshot {
    pub id: StreamId,
    pub state: StreamState,
    pub source_peer: String,
    pub sink_peer: String,
    pub udp_port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub remote_peer_id: Uuid,
    pub state: SessionState,
    pub streams: Vec<StreamSnapshot>,
}

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionId, Session>>,
}

impl SessionManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn open_outgoing(&self, local: Uuid, remote: Uuid) -> SessionId {
        let session = Session::new_outgoing(local, remote);
        let id = session.id;
        self.sessions.write().await.insert(id, session);
        id
    }

    pub async fn register_incoming(
        &self,
        id: SessionId,
        local: Uuid,
        remote: Uuid,
    ) -> Result<(), NetError> {
        let mut guard = self.sessions.write().await;
        if guard.contains_key(&id) {
            return Err(NetError::SignalingProtocol {
                reason: format!("session_id {id} already exists"),
            });
        }
        guard.insert(id, Session::new_incoming(id, local, remote));
        Ok(())
    }

    pub async fn accept(&self, id: &SessionId) -> Result<(), NetError> {
        let mut guard = self.sessions.write().await;
        let s = guard
            .get_mut(id)
            .ok_or_else(|| NetError::SignalingProtocol {
                reason: format!("unknown session {id}"),
            })?;
        s.accept()
    }

    pub async fn add_stream(&self, id: &SessionId, stream: Stream) -> Result<(), NetError> {
        let mut guard = self.sessions.write().await;
        let s = guard
            .get_mut(id)
            .ok_or_else(|| NetError::SignalingProtocol {
                reason: format!("unknown session {id}"),
            })?;
        s.add_stream(stream)
    }

    pub async fn activate_stream(
        &self,
        id: &SessionId,
        stream_id: StreamId,
    ) -> Result<(), NetError> {
        let mut guard = self.sessions.write().await;
        let s = guard
            .get_mut(id)
            .ok_or_else(|| NetError::SignalingProtocol {
                reason: format!("unknown session {id}"),
            })?;
        let st = s
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| NetError::SignalingProtocol {
                reason: format!("unknown stream {stream_id} in session {id}"),
            })?;
        st.activate()
    }

    pub async fn close(&self, id: &SessionId) -> Result<(), NetError> {
        let mut guard = self.sessions.write().await;
        let s = guard
            .get_mut(id)
            .ok_or_else(|| NetError::SignalingProtocol {
                reason: format!("unknown session {id}"),
            })?;
        s.close();
        Ok(())
    }

    pub async fn snapshot(&self) -> Vec<SessionSnapshot> {
        let guard = self.sessions.read().await;
        guard
            .values()
            .map(|s| SessionSnapshot {
                id: s.id,
                remote_peer_id: s.remote_peer_id,
                state: s.state,
                streams: s
                    .streams
                    .values()
                    .map(|st| StreamSnapshot {
                        id: st.id,
                        state: st.state,
                        source_peer: st.route.source.peer_id.clone(),
                        sink_peer: st.route.sink.peer_id.clone(),
                        udp_port: st.udp_port,
                    })
                    .collect(),
            })
            .collect()
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

    #[tokio::test]
    async fn open_and_snapshot_round_trip() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        let snap = mgr.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, id);
        assert_eq!(snap[0].state, SessionState::PendingOutgoing);
    }

    #[tokio::test]
    async fn accept_then_add_stream_then_activate() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        mgr.accept(&id).await.unwrap();
        mgr.add_stream(&id, Stream::new_negotiating(0, route(), 5004))
            .await
            .unwrap();
        mgr.activate_stream(&id, 0).await.unwrap();
        let snap = mgr.snapshot().await;
        assert_eq!(snap[0].streams[0].state, StreamState::Active);
    }

    #[tokio::test]
    async fn add_stream_to_unknown_session_errors() {
        let mgr = SessionManager::new();
        let fake = Uuid::new_v4();
        let err = mgr
            .add_stream(&fake, Stream::new_negotiating(0, route(), 5004))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::SignalingProtocol { .. }));
    }

    #[tokio::test]
    async fn close_marks_session_and_streams_closed() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        mgr.accept(&id).await.unwrap();
        mgr.add_stream(&id, Stream::new_negotiating(0, route(), 5004))
            .await
            .unwrap();
        mgr.activate_stream(&id, 0).await.unwrap();
        mgr.close(&id).await.unwrap();
        let snap = mgr.snapshot().await;
        assert_eq!(snap[0].state, SessionState::Closed);
        assert_eq!(snap[0].streams[0].state, StreamState::Closed);
    }
}
