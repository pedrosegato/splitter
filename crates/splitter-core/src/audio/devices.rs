use crate::error::AudioError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub default_sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub enum DeviceKind {
    Input,
    Output,
    SystemAudio,
}

use cpal::traits::{DeviceTrait, HostTrait};

pub fn list_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let mut out = Vec::new();

    let inputs = host.input_devices().map_err(|e| AudioError::BuildStream {
        source: Box::new(e),
    })?;
    for (idx, d) in inputs.enumerate() {
        if let Some(info) = device_info(&d, DeviceKind::Input, idx) {
            out.push(info);
        }
    }

    let outputs = host.output_devices().map_err(|e| AudioError::BuildStream {
        source: Box::new(e),
    })?;
    for (idx, d) in outputs.enumerate() {
        if let Some(info) = device_info(&d, DeviceKind::Output, idx) {
            out.push(info);
        }
    }

    #[cfg(all(target_os = "macos", feature = "sck"))]
    out.push(DeviceInfo {
        id: "SystemAudio:0:ScreenCaptureKit".into(),
        name: "Desktop Audio (ScreenCaptureKit)".into(),
        kind: DeviceKind::SystemAudio,
        default_sample_rate: crate::SAMPLE_RATE,
        channels: 1,
    });

    #[cfg(target_os = "windows")]
    if let Ok(wasapi_host) = cpal::host_from_id(cpal::HostId::Wasapi) {
        if let Some(d) = wasapi_host.default_output_device() {
            let name = d.name().unwrap_or_else(|_| "default output".into());
            out.push(DeviceInfo {
                id: format!("SystemAudio:0:{name}"),
                name: format!("Desktop Audio ({name})"),
                kind: DeviceKind::SystemAudio,
                default_sample_rate: crate::SAMPLE_RATE,
                channels: 2,
            });
        }
    }

    #[cfg(target_os = "linux")]
    {
        let host_linux = cpal::default_host();
        if let Ok(inputs) = host_linux.input_devices() {
            for (idx, d) in inputs.enumerate() {
                if let Ok(name) = d.name() {
                    if name.ends_with(".monitor") {
                        out.push(DeviceInfo {
                            id: format!("SystemAudio:{idx}:{name}"),
                            name: format!("Desktop Audio ({name})"),
                            kind: DeviceKind::SystemAudio,
                            default_sample_rate: crate::SAMPLE_RATE,
                            channels: 2,
                        });
                    }
                }
            }
        }
    }

    Ok(out)
}

fn device_info(d: &cpal::Device, kind: DeviceKind, idx: usize) -> Option<DeviceInfo> {
    let name = d.name().ok()?;
    let cfg = match kind {
        DeviceKind::Input => d.default_input_config().ok()?,
        DeviceKind::Output => d.default_output_config().ok()?,
        DeviceKind::SystemAudio => return None,
    };
    let id = format!("{kind:?}:{idx}:{name}");
    Some(DeviceInfo {
        id,
        name,
        kind,
        default_sample_rate: cfg.sample_rate().0,
        channels: cfg.channels(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_returns_none_for_system_audio() {
        let host = cpal::default_host();
        if let Some(d) = host.default_output_device() {
            assert!(device_info(&d, DeviceKind::SystemAudio, 0).is_none());
        }
    }

    #[test]
    fn list_devices_returns_at_least_default() {
        let devs = list_devices();
        assert!(devs.is_ok());
    }

    #[test]
    fn list_returns_inputs_and_outputs_separated() {
        if let Ok(devs) = list_devices() {
            for d in &devs {
                assert!(matches!(
                    d.kind,
                    DeviceKind::Input | DeviceKind::Output | DeviceKind::SystemAudio
                ));
            }
        }
    }

    #[test]
    fn ids_are_unique_within_a_listing() {
        if let Ok(devs) = list_devices() {
            let mut ids: Vec<&str> = devs.iter().map(|d| d.id.as_str()).collect();
            let n = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), n, "device ids must be unique within a listing");
        }
    }

    #[test]
    fn list_includes_system_audio_entry_on_mac_and_windows() {
        let devs = list_devices().expect("list");
        #[cfg(all(target_os = "macos", feature = "sck"))]
        assert!(
            devs.iter()
                .any(|d| matches!(d.kind, DeviceKind::SystemAudio)),
            "expected a SystemAudio entry on macOS with sck feature"
        );
        #[cfg(target_os = "windows")]
        assert!(
            devs.iter()
                .any(|d| matches!(d.kind, DeviceKind::SystemAudio)),
            "expected a SystemAudio entry on Windows"
        );
        #[cfg(target_os = "linux")]
        {
            let has_monitor = devs
                .iter()
                .any(|d| matches!(d.kind, DeviceKind::SystemAudio));
            let _ = has_monitor;
        }
        #[cfg(all(target_os = "macos", not(feature = "sck")))]
        {
            assert!(
                !devs
                    .iter()
                    .any(|d| matches!(d.kind, DeviceKind::SystemAudio)),
                "expected no SystemAudio entry on macOS without sck feature"
            );
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            assert!(!devs
                .iter()
                .any(|d| matches!(d.kind, DeviceKind::SystemAudio)));
        }
    }
}
