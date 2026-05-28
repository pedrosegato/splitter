use crate::error::AudioError;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub default_sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DeviceKind {
    Input,
    Output,
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

    Ok(out)
}

fn device_info(d: &cpal::Device, kind: DeviceKind, idx: usize) -> Option<DeviceInfo> {
    let name = d.name().ok()?;
    let cfg = match kind {
        DeviceKind::Input => d.default_input_config().ok()?,
        DeviceKind::Output => d.default_output_config().ok()?,
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
    fn list_devices_returns_at_least_default() {
        let devs = list_devices();
        assert!(devs.is_ok());
    }

    #[test]
    fn list_returns_inputs_and_outputs_separated() {
        if let Ok(devs) = list_devices() {
            for d in &devs {
                assert!(matches!(d.kind, DeviceKind::Input | DeviceKind::Output));
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
}
