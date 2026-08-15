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
use super::remi::{RemiClient, RemiFetchRequest};
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
    conn: &rusqlite::Connection,
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
    keyring_dir: &Path,
    cas: &crate::filesystem::CasStore,
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

        let result = match download_dependency_package(
            conn,
            pkg_with_repo,
            dest_dir,
            keyring_dir,
            cas,
            Some(pb),
        )
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
    conn: &rusqlite::Connection,
    pkg_with_repo: &PackageWithRepo,
    dest_dir: &Path,
    keyring_dir: &Path,
    cas: &crate::filesystem::CasStore,
    progress_bar: Option<&indicatif::ProgressBar>,
) -> Result<PathBuf> {
    match pkg_with_repo.repository.default_strategy.as_deref() {
        Some("static") => {
            download_static_package_verified_with_progress(
                &pkg_with_repo.package,
                dest_dir,
                None,
                progress_bar,
            )
            .await
        }
        Some("remi") => download_remi_dependency_package(conn, pkg_with_repo, dest_dir, cas).await,
        None | Some("binary") => {
            let trust = DownloadOptions::for_repository(&pkg_with_repo.repository, keyring_dir)?;
            download_package_verified_with_progress(
                &pkg_with_repo.package,
                dest_dir,
                &trust,
                progress_bar,
            )
            .await
        }
        Some(strategy) => Err(Error::ConfigError(format!(
            "repository '{}' has unsupported dependency download strategy '{}'",
            pkg_with_repo.repository.name, strategy
        ))),
    }
}

async fn download_remi_dependency_package(
    conn: &rusqlite::Connection,
    pkg_with_repo: &PackageWithRepo,
    dest_dir: &Path,
    cas: &crate::filesystem::CasStore,
) -> Result<PathBuf> {
    let repository = &pkg_with_repo.repository;
    let endpoint = repository
        .default_strategy_endpoint
        .as_deref()
        .filter(|endpoint| !endpoint.trim().is_empty())
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "Remi repository '{}' has no conversion endpoint",
                repository.name
            ))
        })?;
    let source_profile =
        super::selector::candidate_source_profile(&pkg_with_repo.package, repository)?.ok_or_else(
            || {
                Error::ConfigError(format!(
                    "Remi repository package '{}-{}' has no source profile",
                    pkg_with_repo.package.name, pkg_with_repo.package.version
                ))
            },
        )?;
    let profile =
        super::supported_profiles::profile_for_remi_target(source_profile).ok_or_else(|| {
            Error::ConfigError(format!(
                "Remi repository package '{}-{}' has unsupported source profile '{}'",
                pkg_with_repo.package.name, pkg_with_repo.package.version, source_profile
            ))
        })?;

    let repository_id = repository
        .id
        .ok_or_else(|| Error::ConfigError("Remi repository has no persisted identity".into()))?;
    let policy = super::resolution::remi_transport_trust_policy(conn, repository_id)?;
    RemiClient::new(endpoint)?
        .fetch_package(RemiFetchRequest {
            distro: profile.remi_route_slug(),
            name: &pkg_with_repo.package.name,
            version: Some(&pkg_with_repo.package.version),
            architecture: pkg_with_repo.package.architecture.as_deref(),
            output_dir: dest_dir,
            cas,
            trust_policy: &policy,
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Repository, RepositoryPackage};
    use crate::repository::versioning::VersionScheme;

    fn package_with_repository(
        strategy: Option<&str>,
        endpoint: Option<String>,
        source_profile: Option<&str>,
        download_url: String,
        checksum: String,
        size: i64,
    ) -> PackageWithRepo {
        let mut repository = Repository::new(
            "dependency-source".to_string(),
            "https://metadata.example.invalid".to_string(),
        );
        repository.default_strategy = strategy.map(str::to_string);
        repository.default_strategy_endpoint = endpoint;
        repository.source_profile = source_profile.map(str::to_string);

        let mut package = RepositoryPackage::new(
            7,
            "dbus-broker".to_string(),
            "37-8.fc44".to_string(),
            VersionScheme::Rpm,
            checksum,
            size,
            download_url,
        );
        package.architecture = Some("x86_64".to_string());
        package.source_profile = source_profile.map(str::to_string);

        PackageWithRepo {
            package,
            repository,
        }
    }

    #[tokio::test]
    async fn native_dependency_still_requires_ecosystem_trust() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let cas_root = tempfile::tempdir().unwrap();
        let cas = crate::filesystem::CasStore::new(cas_root.path()).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let keyring = tempfile::tempdir().unwrap();
        let package = package_with_repository(
            Some("binary"),
            None,
            Some("fedora-44"),
            "https://native.example.invalid/dbus-broker.rpm".to_string(),
            "sha256:unused".to_string(),
            1,
        );

        let error = download_dependency_package(
            &conn,
            &package,
            destination.path(),
            keyring.path(),
            &cas,
            None,
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("has no ecosystem-native trust policy"),
            "native dependency trust must remain mandatory: {error}"
        );
    }

    #[tokio::test]
    async fn static_dependency_keeps_checksum_verified_local_path() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let cas_root = tempfile::tempdir().unwrap();
        let cas = crate::filesystem::CasStore::new(cas_root.path()).unwrap();
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let keyring = tempfile::tempdir().unwrap();
        let source_path = source.path().join("dependency.ccs");
        let bytes = [0x1f, 0x8b, 0x08, 0x00];
        std::fs::write(&source_path, bytes).unwrap();
        let package = package_with_repository(
            Some("static"),
            None,
            None,
            source_path.to_string_lossy().into_owned(),
            crate::hash::sha256(&bytes),
            bytes.len() as i64,
        );

        let downloaded = download_dependency_package(
            &conn,
            &package,
            destination.path(),
            keyring.path(),
            &cas,
            None,
        )
        .await
        .expect("static dependency path must remain checksum verified");

        assert_eq!(std::fs::read(downloaded).unwrap(), bytes);
    }
}
