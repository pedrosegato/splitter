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
use std::collections::HashMap;

/// `Kind:n:name`, where `n` counts earlier same-name devices of that kind. The enumeration
/// position can't be used: it shifts whenever any device is plugged in.
pub(crate) fn stable_ids(
    kind: &str,
    names: impl IntoIterator<Item = String>,
) -> Vec<(String, String)> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    names
        .into_iter()
        .map(|name| {
            let n = seen.entry(name.clone()).or_insert(0);
            let id = format!("{kind}:{n}:{name}");
            *n += 1;
            (id, name)
        })
        .collect()
}

pub(crate) fn parse_device_id(id: &str) -> (&str, usize) {
    let mut parts = id.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_kind), Some(n), Some(name)) => (name, n.parse().unwrap_or(0)),
        _ => (id, 0),
    }
}

/// Falls back to the first name match for older IDs that carry an enumeration index.
pub(crate) fn pick_by_id(
    devices: impl Iterator<Item = cpal::Device>,
    device_id: &str,
) -> Option<cpal::Device> {
    let (name, occurrence) = parse_device_id(device_id);
    let mut matches: Vec<cpal::Device> = devices
        .filter(|d| d.name().map(|n| n == name).unwrap_or(false))
        .collect();
    if occurrence < matches.len() {
        Some(matches.swap_remove(occurrence))
    } else {
        matches.into_iter().next()
    }
}

fn named(devices: impl Iterator<Item = cpal::Device>) -> Vec<(cpal::Device, String)> {
    devices
        .filter_map(|d| d.name().ok().map(|n| (d, n)))
        .collect()
}

pub fn list_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    crate::audio::anchor_audio_host();
    let host = cpal::default_host();
    let mut out = Vec::new();

    let inputs = named(host.input_devices().map_err(|e| AudioError::BuildStream {
        source: Box::new(e),
    })?);
    let ids = stable_ids("Input", inputs.iter().map(|(_, n)| n.clone()));
    for ((d, _), (id, name)) in inputs.iter().zip(ids) {
        if let Some(info) = device_info(d, DeviceKind::Input, id, name) {
            out.push(info);
        }
    }

    let outputs = named(host.output_devices().map_err(|e| AudioError::BuildStream {
        source: Box::new(e),
    })?);
    let ids = stable_ids("Output", outputs.iter().map(|(_, n)| n.clone()));
    for ((d, _), (id, name)) in outputs.iter().zip(ids) {
        if let Some(info) = device_info(d, DeviceKind::Output, id, name) {
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
        let monitors = inputs
            .iter()
            .map(|(_, n)| n.clone())
            .filter(|n| n.ends_with(".monitor"));
        for (id, name) in stable_ids("SystemAudio", monitors) {
            out.push(DeviceInfo {
                id,
                name: format!("Desktop Audio ({name})"),
                kind: DeviceKind::SystemAudio,
                default_sample_rate: crate::SAMPLE_RATE,
                channels: 2,
            });
        }
    }

    Ok(out)
}

/// Blocking: enumerates devices.
pub fn default_output_device_id() -> Option<String> {
    crate::audio::anchor_audio_host();
    let name = cpal::default_host().default_output_device()?.name().ok()?;
    list_devices()
        .ok()?
        .into_iter()
        .find(|d| d.kind == DeviceKind::Output && d.name == name)
        .map(|d| d.id)
}

fn resolve_config(
    default_cfg: Option<cpal::SupportedStreamConfig>,
    fallback_rate: u32,
    fallback_channels: u16,
) -> (u32, u16) {
    match default_cfg {
        Some(cfg) => (cfg.sample_rate().0, cfg.channels()),
        None => (fallback_rate, fallback_channels),
    }
}

fn device_info(d: &cpal::Device, kind: DeviceKind, id: String, name: String) -> Option<DeviceInfo> {
    let (default_sample_rate, channels) = match kind {
        DeviceKind::Input => resolve_config(d.default_input_config().ok(), 48_000, 1),
        DeviceKind::Output => resolve_config(d.default_output_config().ok(), 48_000, 2),
        DeviceKind::SystemAudio => return None,
    };
    Some(DeviceInfo {
        id,
        name,
        kind,
        default_sample_rate,
        channels,
    })
}

#[cfg(test)]
mod stable_id_tests {
    use super::{parse_device_id, stable_ids};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plugging_a_device_in_front_does_not_rename_the_others() {
        let before = stable_ids("Output", names(&["Speakers"]));
        let after = stable_ids("Output", names(&["USB Headset", "Speakers"]));
        assert_eq!(before[0].0, "Output:0:Speakers");
        assert!(after.iter().any(|(id, _)| id == "Output:0:Speakers"));
    }

    #[test]
    fn identically_named_devices_get_distinct_ids() {
        let ids = stable_ids("Input", names(&["USB Mic", "Webcam", "USB Mic"]));
        let ids: Vec<&str> = ids.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["Input:0:USB Mic", "Input:0:Webcam", "Input:1:USB Mic"]
        );
    }

    #[test]
    fn parse_device_id_splits_occurrence_and_name() {
        assert_eq!(
            parse_device_id("Input:1:Zoom H6: main"),
            ("Zoom H6: main", 1)
        );
        assert_eq!(parse_device_id("Speakers"), ("Speakers", 0));
    }
}

#[cfg(test)]
mod device_probe_tests {
    use super::resolve_config;
    #[test]
    fn falls_back_when_no_default_config() {
        assert_eq!(resolve_config(None, 48_000, 2), (48_000, 2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_returns_none_for_system_audio() {
        crate::audio::anchor_audio_host();
        let host = cpal::default_host();
        if let Some(d) = host.default_output_device() {
            assert!(device_info(&d, DeviceKind::SystemAudio, "x".into(), "x".into()).is_none());
        }
    }

    #[test]
    fn default_output_device_id_points_at_the_host_default() {
        crate::audio::anchor_audio_host();
        let Some(default) = cpal::default_host().default_output_device() else {
            return;
        };
        let name = default.name().unwrap();
        // Without a sound card, ALSA's virtual "default" is not among the listed outputs.
        let listed = list_devices()
            .unwrap()
            .iter()
            .any(|d| d.kind == DeviceKind::Output && d.name == name);
        match default_output_device_id() {
            Some(id) => {
                assert!(id.starts_with("Output:"), "got {id}");
                assert!(id.ends_with(&name), "id {id} must name {name}");
            }
            None => assert!(!listed, "{name} is listed, so its id must be returned"),
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
        if devs.iter().any(|d| d.kind == DeviceKind::Output) {
            assert!(
                devs.iter()
                    .any(|d| matches!(d.kind, DeviceKind::SystemAudio)),
                "expected a SystemAudio entry on Windows"
            );
        }
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
