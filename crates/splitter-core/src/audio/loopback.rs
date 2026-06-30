#[cfg(all(target_os = "macos", feature = "sck"))]
pub mod macos;

#[cfg(all(target_os = "macos", feature = "sck"))]
pub use macos::MacosLoopbackHandle;
