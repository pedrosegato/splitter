use audiomirror_core::current_log_path;
use tokio::io::AsyncReadExt;

#[allow(clippy::print_stdout)]
pub(crate) async fn run_path() -> anyhow::Result<()> {
    println!("{}", current_log_path()?.display());
    Ok(())
}

pub(crate) async fn run_tail() -> anyhow::Result<()> {
    let path = current_log_path()?;
    let mut file = tokio::fs::File::open(&path).await?;
    let mut pos = file.metadata().await?.len();
    let mut buf = vec![0u8; 4096];
    loop {
        let len = tokio::fs::metadata(&path).await?.len();
        if len > pos {
            use tokio::io::AsyncSeekExt;
            file.seek(std::io::SeekFrom::Start(pos)).await?;
            let n = file.read(&mut buf).await?;
            if n > 0 {
                tokio::io::AsyncWriteExt::write_all(&mut tokio::io::stdout(), &buf[..n]).await?;
                pos += n as u64;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn current_log_path_is_callable() {
        // Smoke-test: current_log_path is callable without panicking.
        // On headless CI data_dir may be absent, so both Ok and Err are valid.
        let _ = audiomirror_core::current_log_path();
    }
}
