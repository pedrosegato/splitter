use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique per writer, so concurrent processes never truncate or rename each other's file.
fn temp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{unique}.tmp", std::process::id()))
}

fn write_private(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path(path);
    write_private(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Like `write_atomic`, but never replaces an existing file: returns false when another
/// writer created `path` first.
pub(crate) fn create_new_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    let tmp = temp_path(path);
    write_private(&tmp, bytes)?;
    let linked = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
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
        write_atomic(&path, b"x = 2\n").expect("overwrite");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries,
            ["data.toml"],
            "tmp file must not remain after atomic write"
        );
    }

    #[test]
    fn concurrent_writers_to_the_same_path_all_succeed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("shared.toml");
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || write_atomic(&path, format!("n = {i}\n").as_bytes()))
            })
            .collect();
        for writer in writers {
            writer
                .join()
                .unwrap()
                .expect("no writer may lose its temp file");
        }
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("n = "));
    }

    #[test]
    fn create_new_keeps_the_first_file_and_reports_the_loser() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.toml");
        assert!(create_new_atomic(&path, b"first").unwrap());
        assert!(!create_new_atomic(&path, b"second").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "temp files must be cleaned up");
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
