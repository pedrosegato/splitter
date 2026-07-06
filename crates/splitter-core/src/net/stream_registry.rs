use crate::error::NetError;
use crate::net::session::SessionId;
use crate::net::stream::StreamId;
use crate::net::stream_runtime::{StreamControlSignal, StreamRuntime};
use crate::net::stream_stats::{StreamStats, StreamStatsSnapshot};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
pub struct StreamRuntimeSummary {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub bound_device_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct StreamRegistry {
    pub(crate) inner: RwLock<HashMap<(SessionId, StreamId), StreamRuntime>>,
    prev_snapshots: RwLock<HashMap<(SessionId, StreamId), StreamStatsSnapshot>>,
}

impl StreamRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn register(&self, rt: StreamRuntime) -> Result<(), NetError> {
        let key = (rt.session_id, rt.stream_id);
        let mut guard = self.inner.write().await;
        if guard.contains_key(&key) {
            return Err(NetError::SignalingProtocol {
                reason: format!("stream {} on session {} already registered", key.1, key.0),
            });
        }
        guard.insert(key, rt);
        Ok(())
    }

    pub async fn list(&self) -> Vec<StreamRuntimeSummary> {
        self.inner
            .read()
            .await
            .values()
            .map(|rt| StreamRuntimeSummary {
                session_id: rt.session_id,
                stream_id: rt.stream_id,
                bound_device_id: rt.bound_device_id.clone(),
            })
            .collect()
    }

    pub async fn send_control(
        &self,
        session_id: &SessionId,
        stream_id: StreamId,
        signal: StreamControlSignal,
    ) -> Result<(), NetError> {
        let guard = self.inner.read().await;
        let rt = guard
            .get(&(*session_id, stream_id))
            .ok_or(NetError::UnknownStream {
                session: session_id.get(),
                stream: stream_id.get(),
            })?;
        rt.control_tx
            .send(signal)
            .await
            .map_err(|_| NetError::ChannelClosed)
    }

    pub async fn close(&self, session_id: &SessionId, stream_id: StreamId) -> Result<(), NetError> {
        let mut guard = self.inner.write().await;
        if let Some(rt) = guard.remove(&(*session_id, stream_id)) {
            let _ = rt.control_tx.send(StreamControlSignal::Close).await;
            rt.join.abort();
            Ok(())
        } else {
            Err(NetError::UnknownStream {
                session: session_id.get(),
                stream: stream_id.get(),
            })
        }
    }

    pub async fn get_stats(
        &self,
        session_id: &SessionId,
        stream_id: StreamId,
    ) -> Option<Arc<StreamStats>> {
        self.inner
            .read()
            .await
            .get(&(*session_id, stream_id))
            .map(|rt| rt.stats.clone())
    }

    pub async fn current_stats(&self) -> Vec<(SessionId, StreamId, Arc<StreamStats>)> {
        self.inner
            .read()
            .await
            .iter()
            .map(|(&(sid, stream_id), rt)| (sid, stream_id, rt.stats.clone()))
            .collect()
    }

    pub async fn snapshot_stats(
        &self,
        window_ms: u32,
    ) -> Vec<(SessionId, StreamId, StreamStatsSnapshot)> {
        let guard = self.inner.read().await;
        let mut prev = self.prev_snapshots.write().await;
        let mut out = Vec::with_capacity(guard.len());
        for (&key, rt) in guard.iter() {
            let last = prev.get(&key).cloned().unwrap_or_default();
            let snap = rt.stats.snapshot(window_ms, &last);
            prev.insert(key, snap.clone());
            out.push((key.0, key.1, snap));
        }
        out
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::net::stream_runtime::DeviceGuard;
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc;

    fn fake_runtime(session_id: SessionId, stream_id: StreamId) -> StreamRuntime {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async move {
            while let Some(sig) = rx.recv().await {
                if matches!(sig, StreamControlSignal::Close) {
                    break;
                }
            }
        });
        StreamRuntime {
            session_id,
            stream_id,
            stats: Arc::new(StreamStats::default()),
            control_tx: tx,
            bound_device_id: Some("dev-a".into()),
            join,
            device_guard: DeviceGuard::None,
        }
    }

    #[tokio::test]
    async fn register_and_list_round_trip() {
        let reg = StreamRegistry::new();
        let sid = SessionId::new();
        reg.register(fake_runtime(sid, StreamId(0))).await.unwrap();
        let listed = reg.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, sid);
        assert_eq!(listed[0].stream_id, StreamId(0));
    }

    #[tokio::test]
    async fn register_rejects_duplicate_key() {
        let reg = StreamRegistry::new();
        let sid = SessionId::new();
        reg.register(fake_runtime(sid, StreamId(0))).await.unwrap();
        let err = reg
            .register(fake_runtime(sid, StreamId(0)))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::error::NetError::SignalingProtocol { .. }
        ));
    }

    #[tokio::test]
    async fn close_sends_close_signal_and_removes_entry() {
        let reg = StreamRegistry::new();
        let sid = SessionId::new();
        reg.register(fake_runtime(sid, StreamId(0))).await.unwrap();
        reg.close(&sid, StreamId(0)).await.unwrap();
        assert!(reg.list().await.is_empty());
    }

    #[tokio::test]
    async fn snapshot_stats_collects_per_stream() {
        let reg = StreamRegistry::new();
        let sid = SessionId::new();
        let rt = fake_runtime(sid, StreamId(7));
        rt.stats.packets_sent.store(123, Ordering::Relaxed);
        reg.register(rt).await.unwrap();
        let snap = reg.snapshot_stats(1_000).await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].2.packets_sent, 123);
    }

    #[tokio::test]
    async fn current_stats_does_not_mutate_prev_snapshots() {
        let reg = StreamRegistry::new();
        let sid = SessionId::new();
        let rt = fake_runtime(sid, StreamId(3));
        rt.stats.bytes_sent.store(8_000, Ordering::Relaxed);
        reg.register(rt).await.unwrap();

        let before = reg.snapshot_stats(1_000).await;
        assert_eq!(before.len(), 1);
        let bitrate_after_first_snapshot = before[0].2.bitrate_kbps_sent;

        reg.inner
            .read()
            .await
            .values()
            .next()
            .unwrap()
            .stats
            .bytes_sent
            .store(16_000, Ordering::Relaxed);

        let _ = reg.current_stats().await;
        let _ = reg.current_stats().await;

        let after = reg.snapshot_stats(1_000).await;
        assert_eq!(after.len(), 1);
        let bitrate_after_current_stats = after[0].2.bitrate_kbps_sent;

        assert_eq!(
            bitrate_after_current_stats,
            ((16_000u64 - 8_000) * 8 / 1_000) as u32,
            "current_stats must not have advanced prev_snapshots: expected delta from first snapshot baseline, not from current_stats call"
        );
        let _ = bitrate_after_first_snapshot;
    }
}
