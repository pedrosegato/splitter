use std::io::Write;
use std::path::Path;

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".into(),
    });
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    // A reused stale .tmp from a prior crash keeps its old (possibly looser) mode,
    // since .mode() only applies to freshly created files; enforce 0600 either way.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn ensure_private_dir(parent: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(parent)?;
    // Best-effort: tightening the dir to 0700 is defense-in-depth, but the parent may be
    // a pre-existing shared dir we don't own (e.g. a temp dir). The credential files
    // themselves are always created 0600 by write_atomic, so a failed dir chmod is tolerable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_atomic_round_trips_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.toml");
        let content = b"hello = true\n";
        write_atomic(&path, content).expect("write");
        assert_eq!(std::fs::read(&path).unwrap(), content);
    }

    #[test]
    fn write_atomic_leaves_no_tmp_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.toml");
        write_atomic(&path, b"x = 1\n").expect("write");
        let tmp = path.with_extension("toml.tmp");
        assert!(!tmp.exists(), "tmp file must not remain after atomic write");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.toml");
        write_atomic(&path, b"auth_token = \"x\"\n").expect("write");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config file must be owner-read/write only");
    }
}
