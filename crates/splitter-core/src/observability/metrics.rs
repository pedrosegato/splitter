use crate::error::NetError;
use prometheus::{GaugeVec, IntCounterVec, Opts, Registry, TextEncoder};
use std::sync::Arc;

pub struct MetricsRegistry {
    pub registry: Arc<Registry>,
    pub packets_sent: IntCounterVec,
    pub packets_received: IntCounterVec,
    pub packets_lost: IntCounterVec,
    pub rtt_ms: GaugeVec,
    pub bitrate_kbps: GaugeVec,
    pub peers_connected: prometheus::Gauge,
    pub sessions_active: prometheus::Gauge,
}

impl MetricsRegistry {
    pub fn new() -> Result<Self, NetError> {
        let registry = Arc::new(Registry::new());
        let packets_sent = IntCounterVec::new(
            Opts::new(
                "splitter_stream_packets_sent_total",
                "packets sent per stream",
            ),
            &["stream_id"],
        )
        .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        let packets_received = IntCounterVec::new(
            Opts::new(
                "splitter_stream_packets_received_total",
                "packets received per stream",
            ),
            &["stream_id"],
        )
        .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        let packets_lost = IntCounterVec::new(
            Opts::new(
                "splitter_stream_packets_lost_total",
                "packets lost per stream",
            ),
            &["stream_id"],
        )
        .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        let rtt_ms = GaugeVec::new(
            Opts::new("splitter_stream_rtt_ms", "RTT in ms per stream"),
            &["stream_id"],
        )
        .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        let bitrate_kbps = GaugeVec::new(
            Opts::new(
                "splitter_stream_bitrate_kbps",
                "encoder bitrate in kbps per stream",
            ),
            &["stream_id"],
        )
        .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        let peers_connected =
            prometheus::Gauge::new("splitter_peers_connected", "currently connected peers")
                .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        let sessions_active = prometheus::Gauge::new("splitter_sessions_active", "active sessions")
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(packets_sent.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(packets_received.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(packets_lost.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(rtt_ms.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(bitrate_kbps.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(peers_connected.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        registry
            .register(Box::new(sessions_active.clone()))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")))?;
        Ok(Self {
            registry,
            packets_sent,
            packets_received,
            packets_lost,
            rtt_ms,
            bitrate_kbps,
            peers_connected,
            sessions_active,
        })
    }

    pub fn render(&self) -> Result<String, NetError> {
        let encoder = TextEncoder::new();
        let families = self.registry.gather();
        encoder
            .encode_to_string(&families)
            .map_err(|e| NetError::ConfigIo(format!("prom encode: {e}")))
    }
}

pub async fn serve(metrics: Arc<MetricsRegistry>, port: u16) -> Result<(), NetError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| NetError::ConfigIo(format!("metrics bind {port}: {e}")))?;
    tracing::info!(port, "metrics endpoint listening");

    loop {
        let (sock, _) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("metrics accept: {e}");
                continue;
            }
        };
        let metrics = metrics.clone();
        tokio::spawn(async move {
            let (read, mut write) = sock.into_split();
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            if reader.read_line(&mut line).await.is_err() {
                return;
            }
            let body = metrics.render().unwrap_or_default();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = write.write_all(resp.as_bytes()).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_renders_with_zero_counters() {
        let m = MetricsRegistry::new().unwrap();
        let raw = m.render().unwrap();
        assert!(raw.contains("splitter_peers_connected"));
        assert!(raw.contains("splitter_sessions_active"));
    }

    #[test]
    fn incrementing_counter_appears_in_render() {
        let m = MetricsRegistry::new().unwrap();
        m.packets_sent.with_label_values(&["3"]).inc_by(7);
        let raw = m.render().unwrap();
        assert!(raw.contains("splitter_stream_packets_sent_total{stream_id=\"3\"} 7"));
    }

    #[test]
    fn duplicate_registration_returns_err_not_panic() {
        let registry = Registry::new();
        let g1 = prometheus::Gauge::new("dup_metric", "first").unwrap();
        let g2 = prometheus::Gauge::new("dup_metric", "second").unwrap();
        registry.register(Box::new(g1)).unwrap();
        let result = registry
            .register(Box::new(g2))
            .map_err(|e| NetError::ConfigIo(format!("prom: {e}")));
        assert!(
            matches!(result, Err(NetError::ConfigIo(ref s)) if s.starts_with("prom: ")),
            "expected Err(NetError::ConfigIo(...)) got {:?}",
            result
        );
    }
}
