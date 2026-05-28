use audiomirror_core::config::identity_path;
use audiomirror_core::net::discovery::{Discovery, DiscoveryEvent};
use audiomirror_core::PeerIdentity;
use std::time::Duration;

pub(crate) async fn run(duration_secs: u64, signaling_port: u16) -> anyhow::Result<()> {
    let path = identity_path()?;
    let identity = PeerIdentity::load_or_create(&path)?;
    tracing::info!(
        peer_id = %identity.peer_id,
        peer_name = %identity.peer_name,
        "starting mDNS discovery for {duration_secs}s on signaling_port {signaling_port}"
    );
    let mut discovery = Discovery::start(&identity, signaling_port)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(duration_secs);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, discovery.next_event()).await {
            Ok(Some(DiscoveryEvent::Found(p))) => {
                tracing::info!(
                    "found peer: {} ({}) at {}:{} v{}",
                    p.peer_name,
                    p.peer_id,
                    p.host,
                    p.port,
                    p.version
                );
            }
            Ok(Some(DiscoveryEvent::Removed(name))) => {
                tracing::info!("peer removed: {name}");
            }
            Ok(None) | Err(_) => break,
        }
    }
    Ok(())
}
