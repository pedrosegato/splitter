use crate::net::signaling::client_ops::{ConnEndpoints, ConnectionMap};
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait ControlPlaneHost: Send + Sync + 'static {
    fn spawn_loop(&self, peer_id: Uuid, endpoints: ConnEndpoints);
    async fn on_peer_connected(&self, peer_id: Uuid) {
        let _ = peer_id;
    }
}

pub fn spawn_connection_supervisor(
    mut established: broadcast::Receiver<Uuid>,
    connections: ConnectionMap,
    host: Arc<dyn ControlPlaneHost>,
) {
    tokio::spawn(async move {
        loop {
            match established.recv().await {
                Ok(peer_id) => {
                    host.on_peer_connected(peer_id).await;
                    let endpoints = {
                        let guard = connections.read().await;
                        guard.get(&peer_id).map(|c| ConnEndpoints {
                            tx: c.tx.clone(),
                            remote_addr: c.remote_addr,
                            events: c.events.clone(),
                        })
                    };
                    if let Some(endpoints) = endpoints {
                        host.spawn_loop(peer_id, endpoints);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
