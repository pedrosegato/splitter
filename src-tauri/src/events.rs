use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct PeersChanged(pub Vec<splitter_core::net::discovery::DiscoveredPeer>);

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct IncomingSession {
    pub peer_id: String,
    pub peer_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct StreamStat {
    pub session_id: String,
    pub stream_id: u8,
    pub rtt_ms: u32,
    pub loss_pct: f32,
    pub kbps_sent: u32,
    pub kbps_received: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct StatsTick(pub Vec<StreamStat>);

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct PeerDisconnected {
    pub peer_id: String,
    pub reason: String,
}
