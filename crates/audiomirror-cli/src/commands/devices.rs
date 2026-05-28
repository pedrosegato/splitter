use audiomirror_core::audio::devices::{list_devices, DeviceKind};

#[allow(clippy::print_stdout)]
pub(crate) fn run() -> anyhow::Result<()> {
    let devs = list_devices()?;
    println!("Inputs:");
    for d in devs.iter().filter(|d| d.kind == DeviceKind::Input) {
        println!(
            "  {} | {} ch | {} Hz",
            d.name, d.channels, d.default_sample_rate
        );
    }
    println!("Outputs:");
    for d in devs.iter().filter(|d| d.kind == DeviceKind::Output) {
        println!(
            "  {} | {} ch | {} Hz",
            d.name, d.channels, d.default_sample_rate
        );
    }
    Ok(())
}
