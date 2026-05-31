use splitter_core::net::signaling::client_ops::{
    build_stream_route, find_conn, notify_remote_control, stream_open_message,
    wait_for_stream_open_ack, ConnectionMap,
};
use splitter_core::net::signaling::StreamAction;
use splitter_core::net::stream_runtime::{open_stream_as_source, SourceKind, StreamControlSignal};
use splitter_core::{PeerIdentity, SessionManager, StreamRegistry};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

type OutgoingConns = ConnectionMap;

pub(crate) async fn handle(
    parts: &[&str],
    identity: &PeerIdentity,
    sessions: &Arc<SessionManager>,
    registry: &Arc<StreamRegistry>,
    server: &splitter_core::net::signaling::server::SignalingServerHandle,
    outgoing: &OutgoingConns,
) -> anyhow::Result<()> {
    let (verb, rest) = parts.split_first().ok_or_else(|| {
        anyhow::anyhow!("usage: stream <open|close|volume|mute|pause|resume|stats>")
    })?;
    match *verb {
        "open" => stream_open(rest, identity, sessions, registry, server, outgoing).await,
        "close" => stream_close(rest, sessions, registry, server, outgoing).await,
        "volume" => stream_volume(rest, sessions, registry, server, outgoing).await,
        "mute" => stream_set_mute(rest, true, sessions, registry).await,
        "unmute" => stream_set_mute(rest, false, sessions, registry).await,
        "pause" => stream_set_paused(rest, true, sessions, registry, server, outgoing).await,
        "resume" => stream_set_paused(rest, false, sessions, registry, server, outgoing).await,
        "stats" => stream_stats(rest, registry).await,
        other => Err(anyhow::anyhow!("unknown stream verb: {other}")),
    }
}

fn parse_session_stream(rest: &[&str]) -> anyhow::Result<(Uuid, u8)> {
    let raw = rest
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing <session_id>:<stream_id>"))?;
    let (sid_str, stream_str) = raw
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected session_id:stream_id"))?;
    Ok((Uuid::parse_str(sid_str)?, stream_str.parse::<u8>()?))
}

async fn stream_open(
    rest: &[&str],
    identity: &PeerIdentity,
    sessions: &Arc<SessionManager>,
    registry: &Arc<StreamRegistry>,
    server: &splitter_core::net::signaling::server::SignalingServerHandle,
    outgoing: &OutgoingConns,
) -> anyhow::Result<()> {
    let mut from_dev: Option<String> = None;
    let mut to_spec: Option<String> = None;
    let mut session_id: Option<Uuid> = None;
    let mut bitrate: u32 = 64_000;

    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        match *flag {
            "--from" => from_dev = iter.next().map(|s| (*s).to_string()),
            "--to" => to_spec = iter.next().map(|s| (*s).to_string()),
            "--session" => session_id = iter.next().and_then(|s| Uuid::parse_str(s).ok()),
            "--bitrate" => {
                bitrate = iter
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--bitrate needs a value"))?
                    .parse()?;
            }
            other => return Err(anyhow::anyhow!("unknown flag: {other}")),
        }
    }
    let from_dev = from_dev.ok_or_else(|| anyhow::anyhow!("--from required"))?;
    let to_spec = to_spec.ok_or_else(|| anyhow::anyhow!("--to required"))?;
    let session_id = session_id.ok_or_else(|| anyhow::anyhow!("--session required"))?;

    let (peer_key, remote_device_id) = to_spec
        .split_once(':')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .ok_or_else(|| anyhow::anyhow!("--to expects <peer>:<remote_device_id>"))?;
    let _ = peer_key;

    let snap = sessions.snapshot().await;
    let session = snap
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
    let remote_peer_id = session.remote_peer_id;

    let conn = find_conn(&server.connections, outgoing, remote_peer_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("no live signaling connection to remote peer"))?;

    let stream_id: u8 = sessions
        .next_stream_id(&session_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut ack_rx = conn.events.subscribe();
    conn.tx
        .send(stream_open_message(
            session_id,
            stream_id,
            identity.peer_id,
            remote_peer_id,
            &from_dev,
            &remote_device_id,
            bitrate,
        ))
        .await
        .ok();

    let ack_port = wait_for_stream_open_ack(&mut ack_rx, stream_id, Duration::from_secs(5)).await?;
    let remote: SocketAddr = SocketAddr::new(conn.remote_addr.ip(), ack_port);

    let route = build_stream_route(
        identity.peer_id,
        remote_peer_id,
        &from_dev,
        &remote_device_id,
        bitrate,
    );
    let source_kind = if from_dev == "system" {
        SourceKind::System
    } else {
        SourceKind::Mic(from_dev.clone())
    };
    open_stream_as_source(
        registry.clone(),
        session_id,
        stream_id,
        route,
        remote,
        source_kind,
    )
    .await?;
    #[allow(clippy::print_stdout)]
    {
        println!(">> stream {stream_id} on session {session_id} now active");
    }
    Ok(())
}

async fn stream_close(
    rest: &[&str],
    sessions: &Arc<SessionManager>,
    registry: &Arc<StreamRegistry>,
    server: &splitter_core::net::signaling::server::SignalingServerHandle,
    outgoing: &OutgoingConns,
) -> anyhow::Result<()> {
    let (sid, stream_id) = parse_session_stream(rest)?;
    let snap = sessions.snapshot().await;
    let remote = snap.iter().find(|s| s.id == sid).map(|s| s.remote_peer_id);

    registry.close(&sid, stream_id).await?;
    if let Some(remote) = remote {
        if let Some(conn) = find_conn(&server.connections, outgoing, remote).await {
            notify_remote_control(&conn.tx, stream_id, StreamAction::Close).await;
        }
    }
    #[allow(clippy::print_stdout)]
    {
        println!(">> closed stream {stream_id} on session {sid}");
    }
    Ok(())
}

async fn stream_volume(
    rest: &[&str],
    sessions: &Arc<SessionManager>,
    registry: &Arc<StreamRegistry>,
    server: &splitter_core::net::signaling::server::SignalingServerHandle,
    outgoing: &OutgoingConns,
) -> anyhow::Result<()> {
    let (sid, stream_id) = parse_session_stream(rest)?;
    let raw = rest
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("usage: stream volume <session>:<stream> <0-100>"))?;
    let percent: u32 = raw.parse()?;
    let gain = (percent.min(100) as f32) / 100.0;
    registry
        .send_control(&sid, stream_id, StreamControlSignal::SetVolume(gain))
        .await?;
    let snap = sessions.snapshot().await;
    if let Some(s) = snap.iter().find(|s| s.id == sid) {
        if let Some(conn) = find_conn(&server.connections, outgoing, s.remote_peer_id).await {
            notify_remote_control(
                &conn.tx,
                stream_id,
                StreamAction::SetVolume { volume: gain },
            )
            .await;
        }
    }
    Ok(())
}

async fn stream_set_mute(
    rest: &[&str],
    muted: bool,
    _sessions: &Arc<SessionManager>,
    registry: &Arc<StreamRegistry>,
) -> anyhow::Result<()> {
    let (sid, stream_id) = parse_session_stream(rest)?;
    registry
        .send_control(&sid, stream_id, StreamControlSignal::SetMuted(muted))
        .await?;
    Ok(())
}

async fn stream_set_paused(
    rest: &[&str],
    paused: bool,
    sessions: &Arc<SessionManager>,
    registry: &Arc<StreamRegistry>,
    server: &splitter_core::net::signaling::server::SignalingServerHandle,
    outgoing: &OutgoingConns,
) -> anyhow::Result<()> {
    let (sid, stream_id) = parse_session_stream(rest)?;
    let signal = if paused {
        StreamControlSignal::Pause
    } else {
        StreamControlSignal::Resume
    };
    registry.send_control(&sid, stream_id, signal).await?;
    let action = if paused {
        StreamAction::Pause
    } else {
        StreamAction::Resume
    };
    let snap = sessions.snapshot().await;
    if let Some(s) = snap.iter().find(|s| s.id == sid) {
        if let Some(conn) = find_conn(&server.connections, outgoing, s.remote_peer_id).await {
            notify_remote_control(&conn.tx, stream_id, action).await;
        }
    }
    Ok(())
}

#[allow(clippy::print_stdout)]
async fn stream_stats(rest: &[&str], registry: &Arc<StreamRegistry>) -> anyhow::Result<()> {
    use sysinfo::System;

    const BAR: &str = "═══════════════════════════════════════════════════════════════════════════";
    let target: Option<(Uuid, u8)> = if rest.is_empty() {
        None
    } else {
        Some(parse_session_stream(rest)?)
    };
    let snaps = registry.snapshot_stats(1_000).await;
    let filtered: Vec<_> = match target {
        Some(t) => snaps
            .into_iter()
            .filter(|(sid, st, _)| (*sid, *st) == t)
            .collect(),
        None => snaps,
    };

    let mut sys = System::new();
    sys.refresh_cpu_usage();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu_usage();
    let cpu_pct = {
        let cpus = sys.cpus();
        if cpus.is_empty() {
            0.0f32
        } else {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
        }
    };

    println!("{BAR}");
    println!("  STREAM STATS  [process CPU: {cpu_pct:.1}%]");
    println!("{BAR}");
    if filtered.is_empty() {
        println!("  (no active streams)");
    } else {
        println!(
            "  {:<38}  {:<5}  {:<8}  {:<8}  {:<6}  {:<7}  {:<7}  {:<6}  BW(B/s)",
            "SESSION", "SID", "SENT", "RECV", "LOST", "↑KBPS", "↓KBPS", "RTT"
        );
        println!(
            "  {:<38}  {:<5}  {:<8}  {:<8}  {:<6}  {:<7}  {:<7}  {:<6}  ───────",
            "───────", "───", "────", "────", "────", "─────", "─────", "───"
        );
        for (sid, stream_id, snap) in filtered {
            // Total bandwidth bytes/sec = sent + received kbps converted to bytes/sec.
            // bitrate_kbps values are already per-window so divide by 8 to get kB/s * 1000.
            let bw_bytes_sec =
                (snap.bitrate_kbps_sent as u64 + snap.bitrate_kbps_received as u64) * 1000 / 8;
            println!(
                "  {:<38}  {:<5}  {:<8}  {:<8}  {:<6}  {:<7}  {:<7}  {:<6}  {}",
                sid,
                stream_id,
                snap.packets_sent,
                snap.packets_received,
                snap.packets_lost,
                snap.bitrate_kbps_sent,
                snap.bitrate_kbps_received,
                snap.last_rtt_ms,
                bw_bytes_sec,
            );
        }
    }
    println!("{BAR}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::RwLock;

    #[test]
    fn parse_session_stream_valid() {
        let sid = Uuid::new_v4();
        let raw = format!("{sid}:3");
        let parts: Vec<&str> = vec![raw.as_str()];
        let (parsed_sid, parsed_stream) = parse_session_stream(&parts).unwrap();
        assert_eq!(parsed_sid, sid);
        assert_eq!(parsed_stream, 3u8);
    }

    #[test]
    fn parse_session_stream_missing_arg() {
        let err = parse_session_stream(&[]).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn parse_session_stream_bad_format() {
        let err = parse_session_stream(&["no-colon-here"]).unwrap_err();
        assert!(err.to_string().contains("expected"));
    }

    async fn make_test_server(
        identity: &PeerIdentity,
        sessions: Arc<SessionManager>,
    ) -> splitter_core::net::signaling::server::SignalingServerHandle {
        use splitter_core::net::signaling::server::SignalingServer;
        use splitter_core::net::trust::TrustStore;
        use splitter_core::settings::Settings;

        let path = std::env::temp_dir().join(format!("trust-{}.toml", Uuid::new_v4()));
        let trust = Arc::new(RwLock::new(TrustStore::load_or_create(&path).unwrap()));
        let settings = Arc::new(RwLock::new(Settings::default()));
        SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            identity.clone(),
            trust,
            sessions,
            settings,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn handle_unknown_verb_errors() {
        let identity = PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: "test".into(),
        };
        let sessions = SessionManager::new();
        let registry = StreamRegistry::new();
        let server = make_test_server(&identity, sessions.clone()).await;

        let outgoing = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let parts: Vec<&str> = vec!["bogus"];
        let err = handle(&parts, &identity, &sessions, &registry, &server, &outgoing)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown stream verb: bogus"));
    }

    #[tokio::test]
    async fn handle_missing_verb_errors() {
        let identity = PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: "test".into(),
        };
        let sessions = SessionManager::new();
        let registry = StreamRegistry::new();
        let server = make_test_server(&identity, sessions.clone()).await;

        let outgoing = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let err = handle(&[], &identity, &sessions, &registry, &server, &outgoing)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("usage: stream"));
    }
}
