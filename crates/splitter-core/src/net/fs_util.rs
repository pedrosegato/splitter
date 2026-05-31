use std::path::Path;

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".into(),
    });
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
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
}
