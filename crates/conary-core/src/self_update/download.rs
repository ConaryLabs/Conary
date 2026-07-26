// conary-core/src/self_update/download.rs

use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Download timeout for update packages (5 minutes)
const UPDATE_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Download the CCS package to a temp directory and return the path
///
/// Streams the download through a SHA-256 hasher while writing to disk,
/// avoiding a second full read of the file for verification.
pub async fn download_update(
    download_url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let dest_path = dest_dir.join("conary-update.ccs");
    let response = send_update_request(download_url).await?;
    stream_update_to_disk(response, &dest_path, expected_sha256, None, None).await?;
    Ok(dest_path)
}

/// Download the CCS package with a visual progress bar
///
/// Like [`download_update`] but displays download progress via `indicatif`.
/// If `content_length` is provided, shows a determinate bar; otherwise a spinner.
pub async fn download_update_with_progress(
    download_url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
    content_length: Option<u64>,
) -> Result<PathBuf> {
    use indicatif::{ProgressBar, ProgressStyle};

    let dest_path = dest_dir.join("conary-update.ccs");
    let response = send_update_request(download_url).await?;
    let total = content_length.or_else(|| response.content_length());

    let pb = if let Some(size) = total {
        let bar = ProgressBar::new(size);
        bar.set_style(
            ProgressStyle::default_bar()
                .template("  Downloading [{bar:40.green/dim}] {bytes}/{total_bytes}")
                .expect("Invalid progress bar template")
                .progress_chars("##-"),
        );
        bar
    } else {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} Downloading... {bytes}")
                .expect("Invalid spinner template"),
        );
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        spinner
    };

    stream_update_to_disk(
        response,
        &dest_path,
        expected_sha256,
        content_length,
        Some(&pb),
    )
    .await?;
    Ok(dest_path)
}

/// Send the update download request and check the HTTP status.
///
/// Returns the response on success, or an appropriate error.
async fn send_update_request(download_url: &str) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .timeout(UPDATE_DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| Error::DownloadError(format!("Failed to download update: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::DownloadError(format!(
            "Download failed: HTTP {}",
            response.status()
        )));
    }

    Ok(response)
}

/// Stream an HTTP response to disk while hashing and verifying the checksum.
///
/// Shared implementation for `download_update` and `download_update_with_progress`.
/// If `progress` is `Some`, each chunk advances the progress bar.
async fn stream_update_to_disk(
    mut response: reqwest::Response,
    dest_path: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
    progress: Option<&indicatif::ProgressBar>,
) -> Result<()> {
    use crate::hash::{HashAlgorithm, Hasher};
    use std::io::Write;

    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::ParseError(
            "Invalid expected self-update SHA-256: expected exactly 64 hexadecimal characters"
                .to_string(),
        ));
    }
    if let (Some(expected), Some(advertised)) = (expected_size, response.content_length())
        && advertised != expected
    {
        return Err(Error::DownloadError(format!(
            "Self-update response length mismatch: signed metadata expects {expected} bytes, HTTP response advertises {advertised}"
        )));
    }

    let mut file = fs::File::create(dest_path)
        .map_err(|e| Error::IoError(format!("Failed to create output file: {e}")))?;
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut actual_size = 0u64;

    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                return Err(cleanup_partial_download(
                    dest_path,
                    Error::DownloadError(format!("Failed to read download stream: {error}")),
                ));
            }
        };
        let chunk_size = u64::try_from(chunk.len()).map_err(|_| {
            cleanup_partial_download(
                dest_path,
                Error::DownloadError("Self-update response size overflow".to_string()),
            )
        })?;
        actual_size = actual_size.checked_add(chunk_size).ok_or_else(|| {
            cleanup_partial_download(
                dest_path,
                Error::DownloadError("Self-update response size overflow".to_string()),
            )
        })?;
        if let Some(expected) = expected_size
            && actual_size > expected
        {
            return Err(cleanup_partial_download(
                dest_path,
                Error::DownloadError(format!(
                    "Self-update response exceeds signed metadata size: expected {expected} bytes, received at least {actual_size}"
                )),
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|error| {
            cleanup_partial_download(
                dest_path,
                Error::IoError(format!("Failed to write downloaded data: {error}")),
            )
        })?;
        if let Some(pb) = progress {
            pb.inc(chunk.len() as u64);
        }
    }

    file.flush().map_err(|error| {
        cleanup_partial_download(
            dest_path,
            Error::IoError(format!("Failed to flush download file: {error}")),
        )
    })?;

    if let Some(pb) = progress {
        pb.finish_and_clear();
    }

    if let Some(expected) = expected_size
        && actual_size != expected
    {
        return Err(cleanup_partial_download(
            dest_path,
            Error::DownloadError(format!(
                "Self-update response length mismatch: signed metadata expects {expected} bytes, received {actual_size}"
            )),
        ));
    }

    let actual_hash = hasher.finalize().value;
    if actual_hash != expected_sha256 {
        return Err(cleanup_partial_download(
            dest_path,
            Error::ChecksumMismatch {
                expected: expected_sha256.to_string(),
                actual: actual_hash,
            },
        ));
    }

    Ok(())
}

fn cleanup_partial_download(path: &Path, primary: Error) -> Error {
    match fs::remove_file(path) {
        Ok(()) => primary,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => primary,
        Err(error) => Error::IoError(format!(
            "{primary}; additionally failed to remove partial update {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_download_server(body: Vec<u8>, include_length: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let length = if include_length {
                format!("Content-Length: {}\r\n", body.len())
            } else {
                String::new()
            };
            let headers = format!("HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n");
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        format!("http://{address}/conary.ccs")
    }

    #[tokio::test]
    async fn signed_download_size_is_exact_and_partial_file_is_removed() {
        let body = b"short".to_vec();
        let url = spawn_download_server(body.clone(), false).await;
        let directory = tempfile::tempdir().unwrap();
        let error = download_update_with_progress(
            &url,
            &crate::hash::sha256(&body),
            directory.path(),
            Some(body.len() as u64 + 1),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("signed metadata expects"));
        assert!(!directory.path().join("conary-update.ccs").exists());
    }

    #[tokio::test]
    async fn exact_signed_download_size_and_digest_succeed() {
        let body = b"current signed update".to_vec();
        let url = spawn_download_server(body.clone(), true).await;
        let directory = tempfile::tempdir().unwrap();
        let path = download_update_with_progress(
            &url,
            &crate::hash::sha256(&body),
            directory.path(),
            Some(body.len() as u64),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(path).unwrap(), body);
    }

    #[tokio::test]
    async fn malformed_expected_digest_is_rejected_before_writing() {
        let body = b"content".to_vec();
        let url = spawn_download_server(body, true).await;
        let directory = tempfile::tempdir().unwrap();
        let error = download_update(&url, "not-a-sha256", directory.path())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("expected exactly 64"));
        assert!(!directory.path().join("conary-update.ccs").exists());
    }
}
