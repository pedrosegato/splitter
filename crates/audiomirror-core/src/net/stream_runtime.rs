use crate::net::session::SessionId;
use crate::net::stream::StreamId;
use serde::Serialize;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamControlSignal {
    SetVolume(f32),
    SetMuted(bool),
    Pause,
    Resume,
    Close,
}

#[derive(Debug)]
pub struct StreamRuntime {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub stats: Arc<StreamStats>,
    pub control_tx: mpsc::Sender<StreamControlSignal>,
    pub bound_device_id: Option<String>,
    pub join: JoinHandle<()>,
}

impl StreamRuntime {
    pub async fn abort(self) {
        let _ = self.control_tx.send(StreamControlSignal::Close).await;
        self.join.abort();
    }
}

#[derive(Debug, Default)]
pub struct StreamStats {
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub packets_lost: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub last_rtt_ms: AtomicU32,
    pub last_seq_received: AtomicU32,
    pub last_heartbeat_echo_ms: AtomicU64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StreamStatsSnapshot {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub last_rtt_ms: u32,
    pub bitrate_kbps_sent: u32,
    pub bitrate_kbps_received: u32,
}

impl StreamStats {
    pub fn snapshot(&self, window_ms: u32, prev: &StreamStatsSnapshot) -> StreamStatsSnapshot {
        let packets_sent = self.packets_sent.load(Ordering::Relaxed);
        let packets_received = self.packets_received.load(Ordering::Relaxed);
        let packets_lost = self.packets_lost.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let bytes_received = self.bytes_received.load(Ordering::Relaxed);
        let last_rtt_ms = self.last_rtt_ms.load(Ordering::Relaxed);

        let bytes_sent_delta = bytes_sent.saturating_sub(prev.bytes_sent);
        let bytes_recv_delta = bytes_received.saturating_sub(prev.bytes_received);
        let denom = window_ms.max(1) as u64;
        let bitrate_kbps_sent = ((bytes_sent_delta * 8) / denom) as u32;
        let bitrate_kbps_received = ((bytes_recv_delta * 8) / denom) as u32;

        StreamStatsSnapshot {
            packets_sent,
            packets_received,
            packets_lost,
            bytes_sent,
            bytes_received,
            last_rtt_ms,
            bitrate_kbps_sent,
            bitrate_kbps_received,
        }
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn control_signal_volume_round_trips_through_channel() {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        tx.send(StreamControlSignal::SetVolume(0.5)).await.unwrap();
        match rx.recv().await {
            Some(StreamControlSignal::SetVolume(v)) => assert!((v - 0.5).abs() < 1e-6),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn control_signal_close_round_trips() {
        let (tx, mut rx) = mpsc::channel::<StreamControlSignal>(4);
        tx.send(StreamControlSignal::Close).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StreamControlSignal::Close)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_stats_snapshot_is_all_zero() {
        let stats = StreamStats::default();
        let prev = StreamStatsSnapshot {
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_rtt_ms: 0,
            bitrate_kbps_sent: 0,
            bitrate_kbps_received: 0,
        };
        let snap = stats.snapshot(5_000, &prev);
        assert_eq!(snap, prev);
    }

    #[test]
    fn snapshot_computes_bitrate_from_window() {
        let stats = StreamStats::default();
        stats.bytes_sent.store(8_000, Ordering::Relaxed);
        let prev = StreamStatsSnapshot {
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_rtt_ms: 0,
            bitrate_kbps_sent: 0,
            bitrate_kbps_received: 0,
        };
        let snap = stats.snapshot(1_000, &prev);
        assert_eq!(snap.bitrate_kbps_sent, 64);
    }

    #[test]
    fn snapshot_reads_rtt_atomically() {
        let stats = StreamStats::default();
        stats.last_rtt_ms.store(42, Ordering::Relaxed);
        let prev = StreamStatsSnapshot {
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_rtt_ms: 0,
            bitrate_kbps_sent: 0,
            bitrate_kbps_received: 0,
        };
        let snap = stats.snapshot(1_000, &prev);
        assert_eq!(snap.last_rtt_ms, 42);
    }
}
