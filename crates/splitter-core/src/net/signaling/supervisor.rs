use crate::net::signaling::client_ops::{ConnEndpoints, ConnectionMap};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait ControlPlaneHost: Send + Sync + 'static {
    fn spawn_loop(&self, peer_id: Uuid, endpoints: ConnEndpoints) -> AbortHandle;
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
        let mut loops: HashMap<Uuid, AbortHandle> = HashMap::new();
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
                            connection_id: c.connection_id,
                        })
                    };
                    if let Some(endpoints) = endpoints {
                        if let Some(prev) = loops.remove(&peer_id) {
                            prev.abort();
                        }
                        let handle = host.spawn_loop(peer_id, endpoints);
                        loops.insert(peer_id, handle);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::signaling::connection::PeerConnectionHandle;
    use crate::net::signaling::{PeerEvent, SignalingMessage};
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use tokio::sync::{mpsc, RwLock};

    struct RecordingHost {
        created: Arc<Mutex<Vec<AbortHandle>>>,
    }

    #[async_trait::async_trait]
    impl ControlPlaneHost for RecordingHost {
        fn spawn_loop(&self, _peer_id: Uuid, _endpoints: ConnEndpoints) -> AbortHandle {
            let handle = tokio::spawn(async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
            })
            .abort_handle();
            self.created.lock().unwrap().push(handle.clone());
            handle
        }
    }

    fn fake_handle() -> PeerConnectionHandle {
        let (tx, _rx) = mpsc::channel::<SignalingMessage>(8);
        let (events, _) = broadcast::channel::<PeerEvent>(8);
        PeerConnectionHandle {
            tx,
            events,
            remote_addr: "127.0.0.1:9000".parse().unwrap(),
            connection_id: Uuid::new_v4(),
            abort: tokio::spawn(async {}).abort_handle(),
            abort_on_drop: AtomicBool::new(false),
        }
    }

    #[tokio::test]
    async fn re_establishing_a_peer_aborts_the_prior_loop() {
        let peer = Uuid::new_v4();
        let connections: ConnectionMap = Arc::new(RwLock::new(HashMap::new()));
        connections.write().await.insert(peer, fake_handle());
        let created = Arc::new(Mutex::new(Vec::new()));
        let host = Arc::new(RecordingHost {
            created: created.clone(),
        });
        let (established_tx, established_rx) = broadcast::channel::<Uuid>(8);

        spawn_connection_supervisor(established_rx, connections, host);

        established_tx.send(peer).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while created.lock().unwrap().len() < 1 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first loop not spawned");

        established_tx.send(peer).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while created.lock().unwrap().len() < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second loop not spawned");

        let aborted = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if created.lock().unwrap()[0].is_finished() {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false);
        assert!(
            aborted,
            "re-establishing the same peer must abort the prior control-plane loop"
        );
        assert!(
            !created.lock().unwrap()[1].is_finished(),
            "the newest loop must remain live"
        );
    }
}
