pub mod audio;
pub mod config;
pub mod error;
pub mod net;

pub use error::CoreError;
pub use net::identity::PeerIdentity;
pub use net::manager::{SessionManager, SessionSnapshot, StreamSnapshot};
pub use net::session::{Session, SessionId, SessionState};
pub use net::stream::{Stream, StreamId, StreamRoute, StreamState};
pub use net::trust::{TrustStore, TrustedPeer};

#[cfg(target_os = "macos")]
pub use audio::loopback::MacosLoopbackHandle;

pub const FRAME_SAMPLES: usize = 960;
pub const SAMPLE_RATE: u32 = 48_000;
