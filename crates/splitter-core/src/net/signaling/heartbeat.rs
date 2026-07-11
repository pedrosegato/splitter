use crate::net::signaling::message::{HeartbeatStreamStats, SignalingMessage};
use crate::net::stream_runtime::{StatsBaseline, StreamRegistry};
use std::sync::Arc;

pub async fn build_heartbeat(
    registry: &Arc<StreamRegistry>,
    window_ms: u32,
    timestamp_ms: u64,
    baseline: &mut StatsBaseline,
) -> SignalingMessage {
    let snaps = registry.snapshot_stats(window_ms, baseline).await;
    let streams_stats = snaps
        .into_iter()
        .map(|(_sid, stream_id, snap)| HeartbeatStreamStats {
            stream_id: stream_id.get(),
            packets_sent: snap.packets_sent,
            packets_received: snap.packets_received,
            packets_lost: snap.packets_lost,
            rtt_ms: if snap.last_rtt_ms > 0 {
                Some(snap.last_rtt_ms)
            } else {
                None
            },
        })
        .collect();
    SignalingMessage::Heartbeat {
        timestamp_ms,
        streams_stats,
    }
}

#[cfg(test)]
mod phase3_tests {
    use super::*;
    use crate::net::session::SessionId;
    use crate::net::stream::StreamId;
    use crate::net::stream_runtime::{
        DeviceGuard, StatsBaseline, StreamControlSignal, StreamRegistry, StreamRuntime, StreamStats,
    };
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn fake_runtime(session_id: SessionId) -> StreamRuntime {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        StreamRuntime {
            session_id,
            stream_id: StreamId(0),
            stats: Arc::new(StreamStats::default()),
            control_tx: tx,
            bound_device_id: None,
            join,
            device_guard: DeviceGuard::None,
        }
    }

    #[tokio::test]
    async fn build_heartbeat_includes_per_stream_stats() {
        let reg = StreamRegistry::new();
        let sid = SessionId::new();
        let rt = fake_runtime(sid);
        rt.stats.packets_sent.store(7, Ordering::Relaxed);
        rt.stats.packets_received.store(6, Ordering::Relaxed);
        rt.stats.packets_lost.store(1, Ordering::Relaxed);
        reg.register(rt).await.unwrap();

        let mut baseline = StatsBaseline::default();
        let msg = build_heartbeat(&reg, 1_000, 12_345, &mut baseline).await;
        match msg {
            crate::net::signaling::SignalingMessage::Heartbeat { streams_stats, .. } => {
                assert_eq!(streams_stats.len(), 1);
                assert_eq!(streams_stats[0].packets_sent, 7);
                assert_eq!(streams_stats[0].packets_received, 6);
                assert_eq!(streams_stats[0].packets_lost, 1);
            }
            other => panic!("expected Heartbeat, got {other:?}"),
        }
    }
}
