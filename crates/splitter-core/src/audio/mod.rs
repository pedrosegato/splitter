pub mod capture;
pub mod codec;
pub mod devices;
pub mod loopback;
pub mod playback;
pub mod resampler;
pub mod ring;

#[cfg(test)]
pub mod test_fixtures;

#[cfg(target_os = "windows")]
#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *const std::ffi::c_void, coinit: u32) -> i32;
}

/// cpal 0.15 creates one process-wide WASAPI device enumerator in the COM apartment of the
/// first thread that enumerates; once that thread exits, device calls crash with
/// STATUS_ACCESS_VIOLATION. Create it from a thread that stays in the MTA for good.
pub fn anchor_audio_host() {
    #[cfg(target_os = "windows")]
    {
        static ANCHOR: std::sync::Once = std::sync::Once::new();
        ANCHOR.call_once(|| {
            use cpal::traits::HostTrait;
            const COINIT_MULTITHREADED: u32 = 0;
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("audio-com-anchor".into())
                .spawn(move || {
                    // SAFETY: plain FFI call; never paired with CoUninitialize on purpose.
                    unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED) };
                    let _ = cpal::default_host().output_devices().map(|mut d| d.next());
                    let _ = ready_tx.send(());
                    loop {
                        std::thread::park();
                    }
                });
            if spawned.is_ok() {
                let _ = ready_rx.recv();
            }
        });
    }
}

// The realtime audio callback must never allocate, so scratch is pre-sized to the largest
// buffer the driver can hand us. Drivers that report Unknown give no bound; 4096 matches
// the max already assumed for WASAPI shared mode and acts as a floor.
const FALLBACK_MAX_CALLBACK_FRAMES: usize = 4096;

// WHY: WASAPI reports Range { max: u32::MAX } when GetBufferSizeLimits fails (the norm in
// shared mode); taken literally that pre-sizes tens of GiB per stream. Streams use
// BufferSize::Default, so callbacks never get near this.
const MAX_CALLBACK_FRAMES_CEILING: usize = 65_536;

pub(crate) fn max_callback_frames(buffer_size: &cpal::SupportedBufferSize) -> usize {
    match buffer_size {
        cpal::SupportedBufferSize::Range { max, .. } => {
            (*max as usize).clamp(FALLBACK_MAX_CALLBACK_FRAMES, MAX_CALLBACK_FRAMES_CEILING)
        }
        cpal::SupportedBufferSize::Unknown => FALLBACK_MAX_CALLBACK_FRAMES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::SupportedBufferSize;

    #[test]
    fn max_callback_frames_caps_wasapi_unbounded_range() {
        let wasapi_shared = SupportedBufferSize::Range {
            min: 0,
            max: u32::MAX,
        };
        assert_eq!(
            max_callback_frames(&wasapi_shared),
            MAX_CALLBACK_FRAMES_CEILING
        );
    }

    #[test]
    fn max_callback_frames_honors_driver_max_within_bounds() {
        let range = SupportedBufferSize::Range { min: 64, max: 8192 };
        assert_eq!(max_callback_frames(&range), 8192);
    }

    #[test]
    fn max_callback_frames_floors_small_driver_max() {
        let range = SupportedBufferSize::Range { min: 64, max: 512 };
        assert_eq!(max_callback_frames(&range), FALLBACK_MAX_CALLBACK_FRAMES);
    }

    #[test]
    fn max_callback_frames_unknown_uses_fallback() {
        assert_eq!(
            max_callback_frames(&SupportedBufferSize::Unknown),
            FALLBACK_MAX_CALLBACK_FRAMES
        );
    }
}
