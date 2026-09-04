// crates/conary-core/src/repository/static_repo/location.rs

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncReadExt;

use super::paths::validate_repo_relative_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoLocation {
    Http { base: String },
    File { root: PathBuf },
}

impl RepoLocation {
    pub fn parse(input: &str) -> Result<Self> {
        if input.starts_with("http://") || input.starts_with("https://") {
            return Ok(Self::Http {
                base: input.trim_end_matches('/').to_string(),
            });
        }

        if let Some(path) = input.strip_prefix("file://") {
            return Ok(Self::File {
                root: PathBuf::from(path),
            });
        }

        if has_url_scheme(input) {
            bail!("static repo location must use http://, https://, file://, or a local path");
        }

        Ok(Self::File {
            root: PathBuf::from(input),
        })
    }

    pub fn join_display(&self, relative: &str) -> Result<String> {
        validate_repo_relative_path(relative)?;

        match self {
            Self::Http { base, .. } => Ok(format!("{base}/{relative}")),
            Self::File { root } => Ok(root.join(relative).display().to_string()),
        }
    }

    pub async fn fetch_bytes(&self, relative: &str, limit: u64) -> Result<Vec<u8>> {
        self.fetch_bytes_with_network_policy(relative, limit, false)
            .await
    }

    pub(crate) async fn fetch_bytes_public_network(
        &self,
        relative: &str,
        limit: u64,
    ) -> Result<Vec<u8>> {
        self.fetch_bytes_with_network_policy(relative, limit, true)
            .await
    }

    async fn fetch_bytes_with_network_policy(
        &self,
        relative: &str,
        limit: u64,
        public_network_only: bool,
    ) -> Result<Vec<u8>> {
        match self
            .try_fetch_bytes_with_network_policy(relative, limit, public_network_only)
            .await?
        {
            Some(bytes) => Ok(bytes),
            None => bail!(
                "static repo path not found: {}",
                self.join_display(relative)?
            ),
        }
    }

    pub async fn try_fetch_bytes(&self, relative: &str, limit: u64) -> Result<Option<Vec<u8>>> {
        self.try_fetch_bytes_with_network_policy(relative, limit, false)
            .await
    }

    pub(crate) async fn try_fetch_bytes_public_network(
        &self,
        relative: &str,
        limit: u64,
    ) -> Result<Option<Vec<u8>>> {
        self.try_fetch_bytes_with_network_policy(relative, limit, true)
            .await
    }

    async fn try_fetch_bytes_with_network_policy(
        &self,
        relative: &str,
        limit: u64,
        public_network_only: bool,
    ) -> Result<Option<Vec<u8>>> {
        validate_repo_relative_path(relative)?;

        match self {
            Self::Http { .. } => {
                self.try_fetch_http_bytes(relative, limit, public_network_only)
                    .await
            }
            Self::File { root } => try_fetch_file_bytes(root.join(relative), limit).await,
        }
    }

    async fn try_fetch_http_bytes(
        &self,
        relative: &str,
        limit: u64,
        public_network_only: bool,
    ) -> Result<Option<Vec<u8>>> {
        let url = self.join_display(relative)?;
        let client = if public_network_only {
            crate::repository::client::RepositoryClient::new_public_network()?
        } else {
            crate::repository::client::RepositoryClient::new()?
        };
        match client.download_to_bytes_with_limit(&url, limit).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(crate::Error::HttpStatus { status: 404, .. }) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

async fn try_fetch_file_bytes(path: PathBuf, limit: u64) -> Result<Option<Vec<u8>>> {
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read static repo metadata {}", path.display()));
        }
    };

    if !metadata.file_type().is_file() {
        bail!(
            "static repo path must be a regular file: {}",
            path.display()
        );
    }

    if metadata.len() > limit {
        bail!(
            "static repo path exceeds byte limit ({} bytes, max {}): {}",
            metadata.len(),
            limit,
            path.display()
        );
    }

    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("open static repo path {}", path.display()));
        }
    };

    let mut reader = file.take(limit.saturating_add(1));
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("read static repo path {}", path.display()))?;

    if bytes.len() as u64 > limit {
        bail!(
            "static repo path exceeds byte limit ({} bytes, max {}): {}",
            bytes.len(),
            limit,
            path.display()
        );
    }

    Ok(Some(bytes))
}

fn has_url_scheme(input: &str) -> bool {
    let Some(colon_index) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

#[cfg(test)]
mod tests {
    use super::RepoLocation;

    #[tokio::test]
    async fn static_location_fetches_from_bare_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("conary-repo.toml"), b"schema = 1\n").unwrap();
        let location = RepoLocation::parse(dir.path().to_str().unwrap()).unwrap();
        let bytes = location
            .fetch_bytes("conary-repo.toml", 1024)
            .await
            .unwrap();
        assert_eq!(bytes, b"schema = 1\n");
    }

    #[tokio::test]
    async fn static_location_fetches_from_file_url() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("metadata")).unwrap();
        std::fs::write(dir.path().join("metadata/root.json"), b"{}").unwrap();
        let url = format!("file://{}", dir.path().display());
        let location = RepoLocation::parse(&url).unwrap();
        let bytes = location
            .fetch_bytes("metadata/root.json", 1024)
            .await
            .unwrap();
        assert_eq!(bytes, b"{}");
    }

    #[tokio::test]
    async fn static_location_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("conary-repo.toml"), b"schema = 1\n").unwrap();
        let location = RepoLocation::parse(dir.path().to_str().unwrap()).unwrap();

        let error = location
            .fetch_bytes("conary-repo.toml", 4)
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("byte limit"),
            "expected byte-limit error, got: {error}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn static_location_rejects_non_regular_file_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        let result = unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) };
        assert_eq!(
            result,
            0,
            "mkfifo failed: {}",
            std::io::Error::last_os_error()
        );

        let writer_fifo = fifo.clone();
        let writer = std::thread::spawn(move || {
            let writer_fifo_c = CString::new(writer_fifo.as_os_str().as_bytes()).unwrap();
            for _ in 0..100 {
                let fd = unsafe {
                    libc::open(writer_fifo_c.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK)
                };
                if fd >= 0 {
                    let data = b"pipe bytes";
                    let _ = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
                    unsafe {
                        libc::close(fd);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        let location = RepoLocation::parse(dir.path().to_str().unwrap()).unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(2), location.fetch_bytes("pipe", 1024)).await;
        writer.join().unwrap();
        let error = result
            .expect("non-regular file handling should not block")
            .expect_err("FIFO should be rejected before reading");

        assert!(
            error.to_string().contains("regular file"),
            "expected regular-file rejection, got: {error}"
        );
    }
}
