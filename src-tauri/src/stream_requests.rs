use splitter_core::net::signaling::SourceKind;
use uuid::Uuid;

/// Held until the local user accepts it: accepting turns on mic or system-audio capture.
#[derive(Debug, Clone)]
pub struct PendingStreamRequest {
    pub id: Uuid,
    pub peer_id: Uuid,
    pub session_id: Uuid,
    pub source: SourceKind,
    pub sink_device: String,
}

impl PendingStreamRequest {
    pub fn source_device_id(&self) -> &str {
        match &self.source {
            SourceKind::Mic { device_id } | SourceKind::System { device_id } => device_id,
        }
    }

    pub fn source_is_system(&self) -> bool {
        matches!(self.source, SourceKind::System { .. })
    }
}
