use super::context::DaemonContext;
use splitter_core::net::signaling::connect_to_peer;
use std::net::SocketAddr;
use std::time::Duration;
use uuid::Uuid;

pub(crate) fn spawn_reconnect_loop(ctx: DaemonContext, peer_id: Uuid) {
    tokio::spawn(async move {
        let delays_secs: [u64; 10] = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30];
        for delay in delays_secs {
            tokio::time::sleep(Duration::from_secs(delay)).await;

            // Only reconnect if peer is still being announced.
            let addr_opt = {
                let map = ctx.discovered.read().await;
                map.values()
                    .find(|p| p.peer_id == peer_id.to_string())
                    .map(|p| format!("{}:{}", p.host, p.port))
            };
            let Some(addr_str) = addr_opt else {
                tracing::debug!(%peer_id, "peer no longer in mDNS; aborting reconnect");
                return;
            };
            let Ok(addr) = addr_str.parse::<SocketAddr>() else {
                continue;
            };

            match connect_to_peer(
                addr,
                &ctx.identity,
                ctx.trust.clone(),
                Some(peer_id),
                Duration::from_secs(5),
            )
            .await
            {
                Ok(outcome) if outcome.accepted => {
                    let name = ctx.peer_display_name(&peer_id).await;
                    #[allow(clippy::print_stdout)]
                    {
                        println!(">> reconnected to {name}");
                    }
                    if let Some(pid) = outcome.remote_peer_id {
                        ctx.register_outgoing_connection(pid, outcome.handle).await;
                    }
                    return;
                }
                Ok(outcome) if !outcome.accepted => {
                    tracing::warn!(
                        %peer_id,
                        reason = ?outcome.reason,
                        "reconnect explicitly rejected by peer; giving up"
                    );
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(%peer_id, "reconnect attempt failed: {e}, retrying");
                }
            }

            // If peer dropped from discovery while we were waiting, bail out.
            let still_present = ctx
                .discovered
                .read()
                .await
                .values()
                .any(|p| p.peer_id == peer_id.to_string());
            if !still_present {
                tracing::debug!(%peer_id, "peer no longer in mDNS; aborting reconnect");
                return;
            }
        }

        let name = ctx.peer_display_name(&peer_id).await;
        #[allow(clippy::print_stdout)]
        {
            println!(">> reconnect to {name} failed");
        }
    });
}
