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
    stream_update_to_disk(response, &dest_path, expected_sha256, None).await?;
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

    stream_update_to_disk(response, &dest_path, expected_sha256, Some(&pb)).await?;
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
    progress: Option<&indicatif::ProgressBar>,
) -> Result<()> {
    use crate::hash::{HashAlgorithm, Hasher};
    use std::io::Write;

    let mut file = fs::File::create(dest_path)
        .map_err(|e| Error::IoError(format!("Failed to create output file: {e}")))?;
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::DownloadError(format!("Failed to read download stream: {e}")))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| Error::IoError(format!("Failed to write downloaded data: {e}")))?;
        if let Some(pb) = progress {
            pb.inc(chunk.len() as u64);
        }
    }

    file.flush()
        .map_err(|e| Error::IoError(format!("Failed to flush download file: {e}")))?;

    if let Some(pb) = progress {
        pb.finish_and_clear();
    }

    let actual_hash = hasher.finalize().value;
    if actual_hash != expected_sha256 {
        fs::remove_file(dest_path).ok();
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual: actual_hash,
        });
    }

    Ok(())
}
