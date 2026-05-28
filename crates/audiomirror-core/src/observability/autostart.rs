use crate::error::NetError;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AutostartArtifact {
    pub path: PathBuf,
    pub contents: String,
}

pub fn macos_plist(binary_path: &Path, home: &Path) -> AutostartArtifact {
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.audiomirror.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#,
        binary_path.display()
    );
    AutostartArtifact {
        path: home.join("Library/LaunchAgents/com.audiomirror.daemon.plist"),
        contents: plist,
    }
}

pub fn linux_systemd_unit(binary_path: &Path, config_home: &Path) -> AutostartArtifact {
    let unit = format!(
        r#"[Unit]
Description=AudioMirror Daemon

[Service]
ExecStart={}
Restart=on-failure

[Install]
WantedBy=default.target
"#,
        binary_path.display()
    );
    AutostartArtifact {
        path: config_home.join("systemd/user/audiomirror.service"),
        contents: unit,
    }
}

pub fn windows_run_key_entry(binary_path: &Path) -> AutostartArtifact {
    AutostartArtifact {
        path: PathBuf::from(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\AudioMirror"),
        contents: format!("{} daemon", binary_path.display()),
    }
}

pub fn write_artifact(artifact: &AutostartArtifact) -> Result<(), NetError> {
    if let Some(parent) = artifact.path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(&artifact.path, &artifact.contents)
        .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", artifact.path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn macos_plist_contains_label_and_binary() {
        let a = macos_plist(
            &PathBuf::from("/usr/local/bin/audiomirror-cli"),
            &PathBuf::from("/Users/p"),
        );
        assert!(a.contents.contains("com.audiomirror.daemon"));
        assert!(a.contents.contains("/usr/local/bin/audiomirror-cli"));
        assert!(a
            .path
            .ends_with("Library/LaunchAgents/com.audiomirror.daemon.plist"));
    }

    #[test]
    fn linux_unit_contains_execstart() {
        let a = linux_systemd_unit(
            &PathBuf::from("/usr/bin/audiomirror-cli"),
            &PathBuf::from("/home/p/.config"),
        );
        assert!(a.contents.contains("ExecStart=/usr/bin/audiomirror-cli"));
        assert!(a.path.ends_with("systemd/user/audiomirror.service"));
    }

    #[test]
    fn windows_run_key_contains_daemon_arg() {
        let a = windows_run_key_entry(&PathBuf::from(
            r"C:\Program Files\AudioMirror\audiomirror-cli.exe",
        ));
        assert!(a.contents.contains("daemon"));
        assert!(a
            .path
            .to_string_lossy()
            .contains(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"));
    }

    #[test]
    fn write_artifact_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = AutostartArtifact {
            path: dir.path().join("nested/deep/file.txt"),
            contents: "hello".into(),
        };
        write_artifact(&artifact).unwrap();
        assert!(artifact.path.exists());
    }
}
