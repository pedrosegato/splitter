use splitter_core::observability::autostart::{write_artifact, AutostartArtifact};
use std::path::Path;

#[allow(clippy::print_stdout)]
pub(crate) fn run_enable() -> anyhow::Result<()> {
    let binary = std::env::current_exe()?;
    let artifact = build_artifact(&binary)?;
    write_artifact(&artifact)?;
    activate(&artifact)?;
    println!("autostart enabled: {}", artifact.path.display());
    Ok(())
}

#[allow(clippy::print_stdout)]
pub(crate) fn run_disable() -> anyhow::Result<()> {
    let binary = std::env::current_exe()?;
    let artifact = build_artifact(&binary)?;
    deactivate(&artifact)?;
    if artifact.path.exists() {
        std::fs::remove_file(&artifact.path)?;
    }
    println!("autostart disabled");
    Ok(())
}

#[allow(clippy::print_stdout)]
pub(crate) fn run_status() -> anyhow::Result<()> {
    let binary = std::env::current_exe()?;
    let artifact = build_artifact(&binary)?;
    println!(
        "autostart path: {} ({})",
        artifact.path.display(),
        if artifact.path.exists() {
            "present"
        } else {
            "absent"
        }
    );
    Ok(())
}

fn build_artifact(binary: &Path) -> anyhow::Result<AutostartArtifact> {
    build_artifact_inner(binary)
}

#[cfg(target_os = "macos")]
fn build_artifact_inner(binary: &Path) -> anyhow::Result<AutostartArtifact> {
    use splitter_core::observability::autostart::macos_plist;
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home_dir"))?;
    Ok(macos_plist(binary, &home))
}

#[cfg(target_os = "linux")]
fn build_artifact_inner(binary: &Path) -> anyhow::Result<AutostartArtifact> {
    use splitter_core::observability::autostart::linux_systemd_unit;
    let config = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("no config_dir"))?;
    Ok(linux_systemd_unit(binary, &config))
}

#[cfg(target_os = "windows")]
fn build_artifact_inner(binary: &Path) -> anyhow::Result<AutostartArtifact> {
    use splitter_core::observability::autostart::windows_run_key_entry;
    Ok(windows_run_key_entry(binary))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn build_artifact_inner(binary: &Path) -> anyhow::Result<AutostartArtifact> {
    let _ = binary;
    anyhow::bail!("autostart not implemented on this platform")
}

fn activate(artifact: &AutostartArtifact) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let uid = nix_uid_or_zero();
        let _ = std::process::Command::new("launchctl")
            .args([
                "bootstrap",
                &format!("gui/{uid}"),
                &artifact.path.to_string_lossy(),
            ])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = artifact;
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "splitter.service"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "start", "splitter.service"])
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let value = artifact.contents.clone();
        let _ = std::process::Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Splitter",
                "/t",
                "REG_SZ",
                "/d",
                &value,
                "/f",
            ])
            .status();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let _ = artifact;
    Ok(())
}

fn deactivate(_artifact: &AutostartArtifact) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let uid = nix_uid_or_zero();
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/com.splitter.daemon")])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "splitter.service"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", "splitter.service"])
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Splitter",
                "/f",
            ])
            .status();
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn nix_uid_or_zero() -> u32 {
    std::env::var("UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_artifact_returns_expected_path() {
        // On this host the function must succeed; we just check path is non-empty.
        let binary = std::path::PathBuf::from("/usr/local/bin/splitter-cli");
        let artifact = build_artifact(&binary).expect("build_artifact should succeed");
        assert!(
            !artifact.path.as_os_str().is_empty(),
            "artifact path should not be empty"
        );
    }

    #[test]
    fn run_status_does_not_panic() {
        // Exercises the full status code path with whatever exe path is available.
        // It should never panic regardless of whether the file exists.
        let result = run_status();
        assert!(result.is_ok(), "run_status returned error: {result:?}");
    }
}
