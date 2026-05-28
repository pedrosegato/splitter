pub mod audio;
pub mod config;
pub mod error;
pub mod net;

pub use error::CoreError;

pub const FRAME_SAMPLES: usize = 960;
pub const SAMPLE_RATE: u32 = 48_000;
