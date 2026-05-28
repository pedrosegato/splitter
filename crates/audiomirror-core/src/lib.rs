pub mod audio;
pub mod config;
pub mod error;
pub mod net;
pub mod observability;
pub mod settings;

pub use error::CoreError;
pub use net::identity::PeerIdentity;
pub use net::manager::{SessionManager, SessionSnapshot, StreamSnapshot};
pub use net::session::{Session, SessionId, SessionState};
pub use net::stream::{Stream, StreamId, StreamRoute, StreamState};
pub use net::stream_runtime::{
    StreamControlSignal, StreamRegistry, StreamRuntime, StreamRuntimeSummary, StreamStats,
    StreamStatsSnapshot,
};
pub use net::trust::{TrustStore, TrustedPeer};
pub use observability::logs::{current_log_path, log_dir, LogsGuard};
pub use settings::{settings_path, FecMode, JitterMode, LogLevel, Settings, SettingsHandle};

#[cfg(target_os = "macos")]
pub use audio::loopback::MacosLoopbackHandle;

pub const FRAME_SAMPLES: usize = 960;
pub const SAMPLE_RATE: u32 = 48_000;
