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
pub use net::stream::{Stream, StreamId, StreamRoute, StreamState, Volume};
pub use net::stream_runtime::{
    StreamControlSignal, StreamRegistry, StreamRuntime, StreamRuntimeSummary, StreamStats,
    StreamStatsSnapshot,
};
pub use net::trust::{TrustStore, TrustedPeer};
pub use observability::logs::{current_log_path, log_dir, LogsGuard};
pub use settings::{settings_path, FecMode, JitterMode, LogLevel, Settings, SettingsHandle};

#[cfg(all(target_os = "macos", feature = "sck"))]
pub use audio::loopback::MacosLoopbackHandle;

/// Samples per channel per 20ms Opus frame at 48 kHz.
pub const FRAME_SAMPLES: usize = 960;
/// Interleaved stereo samples per 20ms Opus frame (L,R pairs).
pub const FRAME_STEREO_SAMPLES: usize = FRAME_SAMPLES * 2;
pub const SAMPLE_RATE: u32 = 48_000;
