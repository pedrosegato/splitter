use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
pub enum DeviceEvent {
    Appeared(String),
    Disappeared(String),
}

#[derive(Debug)]
pub struct DeviceWatcherHandle {
    tx: broadcast::Sender<DeviceEvent>,
    #[allow(dead_code)]
    snapshot: Arc<Mutex<HashSet<String>>>,
    _join: JoinHandle<()>,
}

impl DeviceWatcherHandle {
    pub fn subscribe(&self) -> broadcast::Receiver<DeviceEvent> {
        self.tx.subscribe()
    }
}

pub fn diff_snapshots(prev: &HashSet<String>, curr: &HashSet<String>) -> Vec<DeviceEvent> {
    let mut out = Vec::new();
    for id in curr.difference(prev) {
        out.push(DeviceEvent::Appeared(id.clone()));
    }
    for id in prev.difference(curr) {
        out.push(DeviceEvent::Disappeared(id.clone()));
    }
    out
}

pub fn enumerate_device_ids() -> HashSet<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut out = HashSet::new();
    if let Ok(inputs) = host.input_devices() {
        for (idx, d) in inputs.enumerate() {
            if let Ok(n) = d.name() {
                out.insert(format!("Input:{idx}:{n}"));
            }
        }
    }
    if let Ok(outputs) = host.output_devices() {
        for (idx, d) in outputs.enumerate() {
            if let Ok(n) = d.name() {
                out.insert(format!("Output:{idx}:{n}"));
            }
        }
    }
    out
}

pub fn start(poll: std::time::Duration) -> DeviceWatcherHandle {
    let (tx, _) = broadcast::channel(32);
    let snapshot = Arc::new(Mutex::new(enumerate_device_ids()));
    let tx_clone = tx.clone();
    let snap_clone = snapshot.clone();
    let join = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(poll);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let curr = match tokio::task::spawn_blocking(enumerate_device_ids).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("device enumeration task failed: {e}");
                    continue;
                }
            };
            let mut prev = snap_clone.lock().await;
            for ev in diff_snapshots(&prev, &curr) {
                let _ = tx_clone.send(ev);
            }
            *prev = curr;
        }
    });
    DeviceWatcherHandle {
        tx,
        snapshot,
        _join: join,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_appeared_for_new_ids() {
        let mut prev = HashSet::new();
        prev.insert("Input:0:a".to_string());
        let mut curr = prev.clone();
        curr.insert("Output:0:b".to_string());
        let evs = diff_snapshots(&prev, &curr);
        assert_eq!(evs, vec![DeviceEvent::Appeared("Output:0:b".into())]);
    }

    #[test]
    fn diff_reports_disappeared_for_removed_ids() {
        let mut prev = HashSet::new();
        prev.insert("Input:0:a".to_string());
        prev.insert("Output:0:b".to_string());
        let mut curr = HashSet::new();
        curr.insert("Input:0:a".to_string());
        let evs = diff_snapshots(&prev, &curr);
        assert_eq!(evs, vec![DeviceEvent::Disappeared("Output:0:b".into())]);
    }

    #[test]
    fn diff_handles_appeared_and_disappeared_simultaneously() {
        let mut prev = HashSet::new();
        prev.insert("Input:0:a".to_string());
        let mut curr = HashSet::new();
        curr.insert("Output:0:b".to_string());
        let evs = diff_snapshots(&prev, &curr);
        assert_eq!(evs.len(), 2);
        assert!(evs.contains(&DeviceEvent::Appeared("Output:0:b".into())));
        assert!(evs.contains(&DeviceEvent::Disappeared("Input:0:a".into())));
    }

    #[tokio::test]
    async fn start_emits_no_events_on_stable_host() {
        let handle = start(std::time::Duration::from_millis(50));
        let mut rx = handle.subscribe();
        let timed_out = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .is_err();
        assert!(
            timed_out,
            "stable host must not emit phantom events within 200ms"
        );
    }
}
