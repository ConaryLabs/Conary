// conary-core/src/repository/dependencies.rs

//! Dependency downloads
//!
//! Resolution happens in the SAT provider and returns exact repository row
//! identities. This module only downloads those already-selected rows.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use tracing::info;

use super::download::{
    DownloadOptions, DownloadProgress, download_package_verified_with_progress,
    download_static_package_verified_with_progress,
};
use super::selector::PackageWithRepo;

/// Download all dependencies to a directory in parallel
///
/// Downloads are performed concurrently using rayon's parallel iterators.
/// This significantly speeds up the download of multiple dependencies.
///
/// # Arguments
/// * `dependencies` - List of (name, package info) tuples to download
/// * `dest_dir` - Directory to download packages to
/// * `keyring_dir` - Prepared native repository trust store
///
/// # Returns
/// Vec<(dependency_name, downloaded_path)> on success
pub async fn download_dependencies(
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
    keyring_dir: &Path,
) -> Result<Vec<(String, PathBuf)>> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    // Calculate total size for aggregate progress
    let total_size: u64 = dependencies
        .iter()
        .map(|(_, pkg)| pkg.package.size as u64)
        .sum();
    let total_mb = total_size as f64 / 1_048_576.0;

    info!(
        "Downloading {} dependencies in parallel ({:.2} MB total)...",
        dependencies.len(),
        total_mb
    );

    // Create multi-progress manager with aggregate tracking
    let progress = DownloadProgress::with_aggregate(dependencies.len(), total_size);

    // Pre-create progress bars for all downloads
    let progress_bars: Vec<_> = dependencies
        .iter()
        .map(|(dep_name, pkg_with_repo)| {
            progress.add_download(dep_name, pkg_with_repo.package.size as u64)
        })
        .collect();

    // Use parallel iterator for concurrent downloads with progress
    // Collect as Vec<Result<_>> to track individual successes/failures
    let mut individual_results: Vec<Result<(String, PathBuf, u64)>> = Vec::new();
    for ((dep_name, pkg_with_repo), pb) in dependencies.iter().zip(progress_bars.iter()) {
        info!("Downloading dependency: {}", dep_name);

        let trust = if pkg_with_repo.repository.default_strategy.as_deref() == Some("static") {
            None
        } else {
            Some(DownloadOptions::for_repository(
                &pkg_with_repo.repository,
                keyring_dir,
            )?)
        };

        let result =
            match download_dependency_package(pkg_with_repo, dest_dir, trust.as_ref(), Some(pb))
                .await
            {
                Ok(path) => {
                    DownloadProgress::finish_download(pb, dep_name);
                    Ok((dep_name.clone(), path, pkg_with_repo.package.size as u64))
                }
                Err(e) => {
                    DownloadProgress::fail_download(pb, dep_name, &e.to_string());
                    Err(e)
                }
            };
        individual_results.push(result);
    }

    // Calculate statistics and show summary
    let mut succeeded_results = Vec::new();
    let mut failures = Vec::new();
    let mut bytes_downloaded: u64 = 0;

    for result in individual_results {
        match result {
            Ok((name, path, size)) => {
                bytes_downloaded += size;
                succeeded_results.push((name, path));
            }
            Err(e) => {
                failures.push(e.to_string());
            }
        }
    }

    let failed_count = failures.len();
    progress.finish_all(succeeded_results.len(), failed_count, bytes_downloaded);

    // If any downloads failed, return error
    if failed_count > 0 {
        let details = failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let suffix = if failed_count > 5 {
            format!("; ... and {} more", failed_count - 5)
        } else {
            String::new()
        };
        return Err(Error::DownloadError(format!(
            "{} of {} dependency downloads failed: {}{}",
            failed_count,
            dependencies.len(),
            details,
            suffix
        )));
    }

    Ok(succeeded_results)
}

async fn download_dependency_package(
    pkg_with_repo: &PackageWithRepo,
    dest_dir: &Path,
    native_trust: Option<&DownloadOptions>,
    progress_bar: Option<&indicatif::ProgressBar>,
) -> Result<PathBuf> {
    if pkg_with_repo.repository.default_strategy.as_deref() == Some("static") {
        download_static_package_verified_with_progress(
            &pkg_with_repo.package,
            dest_dir,
            None,
            progress_bar,
        )
        .await
    } else {
        let trust = native_trust.ok_or_else(|| {
            Error::ConfigError(format!(
                "native repository '{}' download has no prepared trust policy",
                pkg_with_repo.repository.name
            ))
        })?;
        download_package_verified_with_progress(
            &pkg_with_repo.package,
            dest_dir,
            trust,
            progress_bar,
        )
        .await
    }
}
