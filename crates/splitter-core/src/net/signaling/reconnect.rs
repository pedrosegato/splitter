use crate::net::identity::PeerIdentity;
use crate::net::signaling::{connect_to_peer, ConnectOutcome};
use crate::net::trust::TrustStore;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

const BACKOFF_SECS: [u64; 10] = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30];

#[async_trait::async_trait]
pub trait ReconnectDriver: Send + Sync + 'static {
    fn identity(&self) -> PeerIdentity;
    fn trust(&self) -> Arc<RwLock<TrustStore>>;
    async fn resolve_addr(&self, peer_id: Uuid) -> Option<SocketAddr>;
    async fn still_discoverable(&self, peer_id: Uuid) -> bool;
    async fn on_reconnected(&self, peer_id: Uuid, outcome: ConnectOutcome);
    async fn on_reconnected_display(&self, peer_id: Uuid);
    async fn on_reconnect_failed_display(&self, peer_id: Uuid);
}

pub fn spawn_reconnect(peer_id: Uuid, driver: Arc<dyn ReconnectDriver>) {
    tokio::spawn(async move {
        for delay in BACKOFF_SECS {
            tokio::time::sleep(Duration::from_secs(delay)).await;

            let Some(addr) = driver.resolve_addr(peer_id).await else {
                tracing::debug!(%peer_id, "no dialable address; aborting reconnect");
                return;
            };

            match connect_to_peer(
                addr,
                &driver.identity(),
                driver.trust(),
                Some(peer_id),
                Duration::from_secs(5),
            )
            .await
            {
                Ok(outcome) if outcome.accepted => {
                    driver.on_reconnected_display(peer_id).await;
                    driver.on_reconnected(peer_id, outcome).await;
                    return;
                }
                Ok(outcome) => {
                    tracing::warn!(
                        %peer_id,
                        reason = ?outcome.reason,
                        "reconnect explicitly rejected by peer; giving up"
                    );
                    return;
                }
                Err(e) => {
                    tracing::debug!(%peer_id, "reconnect attempt failed: {e}, retrying");
                }
            }

            if !driver.still_discoverable(peer_id).await {
                tracing::debug!(%peer_id, "peer no longer discoverable; aborting reconnect");
                return;
            }
        }
        driver.on_reconnect_failed_display(peer_id).await;
    });
}
