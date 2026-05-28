use audiomirror_core::audio::devices::{list_devices, DeviceKind};

const BAR: &str = "═══════════════════════════════════════════════════════════════════";

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

    println!("{BAR}");
    println!("  AUDIO DEVICES");
    println!("{BAR}");

    println!("  INPUTS");
    println!("  ──────");
    for (idx, d) in inputs.iter().enumerate() {
        println!(
            "  [in:{idx}]   {:<30} | {} ch | {} Hz",
            d.name, d.channels, d.default_sample_rate
        );
    }
    println!();
    println!("  OUTPUTS");
    println!("  ───────");
    for (idx, d) in outputs.iter().enumerate() {
        println!(
            "  [out:{idx}]  {:<30} | {} ch | {} Hz",
            d.name, d.channels, d.default_sample_rate
        );
    }
    println!();
    println!("  Shorthand: use in:N / out:N in commands.");
    println!("  Example:  loop --input in:0 --output out:0");
    println!("            stream open --session <UUID> --from in:0 --to bob:out:0");
    println!("{BAR}");
    Ok(())
}
