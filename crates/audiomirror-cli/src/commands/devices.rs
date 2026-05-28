use audiomirror_core::audio::devices::{list_devices, DeviceKind};

#[allow(clippy::print_stdout)]
pub(crate) fn run() -> anyhow::Result<()> {
    let devs = list_devices()?;
    let mut inputs: Vec<_> = devs
        .iter()
        .filter(|d| d.kind == DeviceKind::Input)
        .collect();
    let mut outputs: Vec<_> = devs
        .iter()
        .filter(|d| d.kind == DeviceKind::Output)
        .collect();
    inputs.sort_by_key(|d| d.name.clone());
    outputs.sort_by_key(|d| d.name.clone());

    println!("Inputs:");
    for (idx, d) in inputs.iter().enumerate() {
        println!(
            "  [in:{}] {} | {} ch | {} Hz",
            idx, d.name, d.channels, d.default_sample_rate
        );
    }
    println!();
    println!("Outputs:");
    for (idx, d) in outputs.iter().enumerate() {
        println!(
            "  [out:{}] {} | {} ch | {} Hz",
            idx, d.name, d.channels, d.default_sample_rate
        );
    }
    println!();
    println!("Use the shorthand ID (in:N or out:N) in CLI commands, e.g.:");
    println!("  loop --input in:0 --output out:0");
    println!("  stream open --session <UUID> --from in:0 --to bob:out:0");
    Ok(())
}
