use crate::error::NetError;
use crate::net::session::{Session, SessionId, SessionState};
use crate::net::stream::{Stream, StreamId, StreamState};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct StreamSnapshot {
    pub id: StreamId,
    pub state: StreamState,
    pub source_peer: String,
    pub sink_peer: String,
    pub udp_port: u16,
    pub source_device: String,
    pub sink_device: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
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

    async fn with_session_mut<R>(
        &self,
        id: &SessionId,
        f: impl FnOnce(&mut Session) -> Result<R, NetError>,
    ) -> Result<R, NetError> {
        let mut guard = self.sessions.write().await;
        let s = guard
            .get_mut(id)
            .ok_or(NetError::UnknownSession(id.get()))?;
        f(s)
    }

    async fn with_stream_mut<R>(
        &self,
        id: &SessionId,
        stream_id: StreamId,
        f: impl FnOnce(&mut Stream) -> Result<R, NetError>,
    ) -> Result<R, NetError> {
        let mut guard = self.sessions.write().await;
        let s = guard
            .get_mut(id)
            .ok_or(NetError::UnknownSession(id.get()))?;
        let st = s.stream_mut(stream_id).ok_or(NetError::UnknownStream {
            session: id.get(),
            stream: stream_id.get(),
        })?;
        f(st)
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
        self.with_session_mut(id, |s| s.accept()).await
    }

    pub async fn add_stream(&self, id: &SessionId, stream: Stream) -> Result<(), NetError> {
        self.with_session_mut(id, |s| s.add_stream(stream)).await
    }

    pub async fn activate_stream(
        &self,
        id: &SessionId,
        stream_id: StreamId,
    ) -> Result<(), NetError> {
        self.with_stream_mut(id, stream_id, |st| st.activate())
            .await
    }

    pub async fn close(&self, id: &SessionId) -> Result<(), NetError> {
        self.with_session_mut(id, |s| {
            s.close();
            Ok(())
        })
        .await
    }

    pub async fn set_stream_muted(
        &self,
        id: &SessionId,
        stream_id: StreamId,
        muted: bool,
    ) -> Result<(), NetError> {
        self.with_stream_mut(id, stream_id, |st| {
            st.set_muted(muted);
            Ok(())
        })
        .await
    }

    pub async fn next_stream_id(&self, id: &SessionId) -> Result<StreamId, NetError> {
        self.with_session_mut(id, |s| Ok(s.next_stream_id())).await
    }

    pub async fn remove_stream(&self, id: &SessionId, stream_id: StreamId) -> Result<(), NetError> {
        self.with_session_mut(id, |s| {
            s.remove_stream(stream_id);
            Ok(())
        })
        .await
    }

    pub async fn snapshot(&self) -> Vec<SessionSnapshot> {
        let guard = self.sessions.read().await;
        guard
            .values()
            .map(|s| SessionSnapshot {
                id: s.id,
                remote_peer_id: s.remote_peer_id,
                state: s.state(),
                streams: s
                    .streams()
                    .values()
                    .map(|st| StreamSnapshot {
                        id: st.id,
                        state: st.state(),
                        source_peer: st.route().source.peer_id.clone(),
                        sink_peer: st.route().sink.peer_id.clone(),
                        udp_port: st.udp_port,
                        source_device: st.route().source.device_id.clone(),
                        sink_device: st.route().sink.device_id.clone(),
                        volume: st.volume(),
                        muted: st.muted(),
                    })
                    .collect(),
            })
            .collect()
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
        mgr.add_stream(&id, Stream::new_negotiating(StreamId(0), route(), 5004))
            .await
            .unwrap();
        mgr.activate_stream(&id, StreamId(0)).await.unwrap();
        let snap = mgr.snapshot().await;
        assert_eq!(snap[0].streams[0].state, StreamState::Active);
    }

    #[tokio::test]
    async fn add_stream_to_unknown_session_errors() {
        let mgr = SessionManager::new();
        let fake = SessionId::new();
        let err = mgr
            .add_stream(&fake, Stream::new_negotiating(StreamId(0), route(), 5004))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::UnknownSession(_)));
    }

    #[tokio::test]
    async fn close_marks_session_and_streams_closed() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        mgr.accept(&id).await.unwrap();
        mgr.add_stream(&id, Stream::new_negotiating(StreamId(0), route(), 5004))
            .await
            .unwrap();
        mgr.activate_stream(&id, StreamId(0)).await.unwrap();
        mgr.close(&id).await.unwrap();
        let snap = mgr.snapshot().await;
        assert_eq!(snap[0].state, SessionState::Closed);
        assert_eq!(snap[0].streams[0].state, StreamState::Closed);
    }

    #[tokio::test]
    async fn muted_defaults_false_and_reflects_set_stream_muted() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        mgr.accept(&id).await.unwrap();
        mgr.add_stream(&id, Stream::new_negotiating(StreamId(0), route(), 5004))
            .await
            .unwrap();
        let snap = mgr.snapshot().await;
        assert!(!snap[0].streams[0].muted);

        mgr.set_stream_muted(&id, StreamId(0), true).await.unwrap();
        let snap = mgr.snapshot().await;
        assert!(snap[0].streams[0].muted);

        mgr.set_stream_muted(&id, StreamId(0), false).await.unwrap();
        let snap = mgr.snapshot().await;
        assert!(!snap[0].streams[0].muted);
    }

    #[tokio::test]
    async fn set_stream_muted_unknown_session_errors() {
        let mgr = SessionManager::new();
        let fake = SessionId::new();
        let err = mgr
            .set_stream_muted(&fake, StreamId(0), true)
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::UnknownSession(_)));
    }

    #[tokio::test]
    async fn activate_stream_unknown_stream_returns_unknown_stream_error() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        mgr.accept(&id).await.unwrap();
        mgr.add_stream(&id, Stream::new_negotiating(StreamId(0), route(), 5004))
            .await
            .unwrap();
        let err = mgr.activate_stream(&id, StreamId(99)).await.unwrap_err();
        assert!(matches!(err, NetError::UnknownStream { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn set_stream_muted_unknown_stream_returns_unknown_stream_error() {
        let mgr = SessionManager::new();
        let id = mgr.open_outgoing(Uuid::new_v4(), Uuid::new_v4()).await;
        mgr.accept(&id).await.unwrap();
        mgr.add_stream(&id, Stream::new_negotiating(StreamId(0), route(), 5004))
            .await
            .unwrap();
        let err = mgr
            .set_stream_muted(&id, StreamId(99), true)
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::UnknownStream { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn register_incoming_duplicate_id_returns_err() {
        let mgr = SessionManager::new();
        let id = SessionId::new();
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        mgr.register_incoming(id, local, remote).await.unwrap();
        let err = mgr.register_incoming(id, local, remote).await.unwrap_err();
        assert!(
            matches!(err, NetError::SignalingProtocol { .. }),
            "duplicate session_id is a genuine protocol violation: {err}"
        );
    }

    #[tokio::test]
    async fn remove_stream_unknown_session_returns_unknown_session_error() {
        let mgr = SessionManager::new();
        let fake = SessionId::new();
        let err = mgr.remove_stream(&fake, StreamId(0)).await.unwrap_err();
        assert!(
            matches!(err, NetError::UnknownSession(_)),
            "remove_stream on unknown session must error consistently with sibling methods: {err:?}"
        );
    }
}
