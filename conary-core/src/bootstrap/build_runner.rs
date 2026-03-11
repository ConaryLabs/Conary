// conary-core/src/bootstrap/build_runner.rs

//! Shared package build runner for bootstrap stages
//!
//! Extracts the common source-fetching, checksum-verification, and extraction
//! logic that was duplicated across Stage 1, Stage 2, and Base builders.

use super::build_helpers;
use super::config::BootstrapConfig;
use crate::recipe::Recipe;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Errors from the shared build runner
#[derive(Debug, thiserror::Error)]
pub enum BuildRunnerError {
    #[error("Source fetch failed for {package}: {reason}")]
    SourceFetchFailed { package: String, reason: String },

    #[error("Build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Path contains invalid UTF-8: {0}")]
    InvalidPath(PathBuf),
}

/// Shared source-fetching and verification logic for bootstrap stages.
///
/// Each bootstrap stage (Stage 1, Stage 2, Base) needs to download sources,
/// verify checksums, and extract archives. This struct encapsulates that
/// shared behavior so the stages only implement their stage-specific logic
/// (environment setup, sandboxing, toolchain creation).
pub struct PackageBuildRunner {
    /// Source cache directory (shared across stages)
    sources_dir: PathBuf,
    /// Whether to skip checksum verification
    skip_verify: bool,
}

impl PackageBuildRunner {
    /// Create a new build runner
    pub fn new(sources_dir: &Path, config: &BootstrapConfig) -> Self {
        Self {
            sources_dir: sources_dir.to_path_buf(),
            skip_verify: config.skip_verify,
        }
    }

    /// Fetch the primary source archive for a package, returning the local path.
    ///
    /// Downloads the archive if not already cached in `sources_dir`.
    pub fn fetch_source(
        &self,
        pkg_name: &str,
        recipe: &Recipe,
    ) -> Result<PathBuf, BuildRunnerError> {
        let url = recipe.archive_url();
        let filename = recipe.archive_filename();
        let target_path = self.sources_dir.join(&filename);

        if target_path.exists() {
            info!("  Using cached source: {}", filename);
            return Ok(target_path);
        }

        info!("  Fetching: {}", url);

        let target_str = target_path.to_str().ok_or_else(|| {
            BuildRunnerError::InvalidPath(target_path.clone())
        })?;

        let output = Command::new("curl")
            .args(["-fsSL", "-o", target_str, &url])
            .output()
            .map_err(|e| BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: stderr.to_string(),
            });
        }

        self.verify_checksum(pkg_name, &recipe.source.checksum, &target_path)?;

        Ok(target_path)
    }

    /// Verify a SHA-256 checksum, rejecting placeholders unless `skip_verify` is set.
    pub fn verify_checksum(
        &self,
        pkg_name: &str,
        expected: &str,
        path: &Path,
    ) -> Result<(), BuildRunnerError> {
        // Reject placeholder checksums unless skip_verify is enabled
        if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
            if self.skip_verify {
                warn!(
                    "  Skipping placeholder checksum (--skip-verify enabled): {}",
                    expected
                );
                return Ok(());
            }
            return Err(BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: format!(
                    "Recipe has placeholder checksum '{}' -- provide a real SHA-256 or use --skip-verify",
                    expected
                ),
            });
        }

        let (algo, hash) = expected
            .split_once(':')
            .ok_or_else(|| BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: "Invalid checksum format".to_string(),
            })?;

        if algo == "sha256" {
            let output = Command::new("sha256sum")
                .arg(path)
                .output()
                .map_err(|e| BuildRunnerError::SourceFetchFailed {
                    package: pkg_name.to_string(),
                    reason: e.to_string(),
                })?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let computed = stdout.split_whitespace().next().unwrap_or("");
            if computed != hash {
                return Err(BuildRunnerError::SourceFetchFailed {
                    package: pkg_name.to_string(),
                    reason: format!("Checksum mismatch: expected {}, got {}", hash, computed),
                });
            }
        } else {
            warn!("  Unknown checksum algorithm: {}", algo);
        }

        Ok(())
    }

    /// Fetch and extract additional sources (e.g., GMP, MPFR, MPC for GCC).
    pub fn fetch_additional_sources(
        &self,
        pkg_name: &str,
        recipe: &Recipe,
        src_dir: &Path,
    ) -> Result<(), BuildRunnerError> {
        let additional_sources: Vec<_> = recipe
            .source
            .additional
            .iter()
            .map(|a| (a.url.clone(), a.checksum.clone(), a.extract_to.clone()))
            .collect();

        for (url, checksum, extract_to) in additional_sources {
            let filename = url.split('/').next_back().unwrap_or("additional.tar.gz");
            let target_path = self.sources_dir.join(filename);

            // Download if not cached
            if !target_path.exists() {
                info!("  Fetching additional: {}", filename);
                let target_str = target_path.to_str().ok_or_else(|| {
                    BuildRunnerError::InvalidPath(target_path.clone())
                })?;

                let output = Command::new("curl")
                    .args(["-fsSL", "-o", target_str, &url])
                    .output()
                    .map_err(|e| BuildRunnerError::SourceFetchFailed {
                        package: pkg_name.to_string(),
                        reason: e.to_string(),
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(BuildRunnerError::SourceFetchFailed {
                        package: pkg_name.to_string(),
                        reason: format!("Additional source fetch failed: {}", stderr),
                    });
                }
            }

            // Verify checksum
            self.verify_checksum(pkg_name, &checksum, &target_path)?;

            // Extract to the specified location, stripping top-level directory
            let extract_dest = if let Some(dest) = &extract_to {
                src_dir.join(dest)
            } else {
                src_dir.to_path_buf()
            };

            self.extract_source_strip(&target_path, &extract_dest)?;
        }

        Ok(())
    }

    /// Extract a tar archive
    pub fn extract_source(
        &self,
        archive: &Path,
        dest: &Path,
    ) -> Result<(), BuildRunnerError> {
        build_helpers::extract_tar(archive, dest, false).map_err(|e| {
            BuildRunnerError::BuildFailed {
                package: "extract".to_string(),
                reason: e,
            }
        })
    }

    /// Extract a tar archive, stripping the top-level directory
    pub fn extract_source_strip(
        &self,
        archive: &Path,
        dest: &Path,
    ) -> Result<(), BuildRunnerError> {
        build_helpers::extract_tar(archive, dest, true).map_err(|e| {
            BuildRunnerError::BuildFailed {
                package: "extract".to_string(),
                reason: e,
            }
        })
    }

    /// Find the actual source directory after extraction
    pub fn find_source_dir(&self, dir: &Path) -> Result<PathBuf, BuildRunnerError> {
        build_helpers::find_source_dir(dir).map_err(BuildRunnerError::Io)
    }

    /// Prepare the build directory structure for a package.
    ///
    /// Creates `build/<pkg_name>/src` and `build/<pkg_name>/build` directories,
    /// cleaning any previous build artifacts. Returns `(src_dir, build_dir)`.
    pub fn prepare_build_dirs(
        &self,
        work_dir: &Path,
        pkg_name: &str,
    ) -> Result<(PathBuf, PathBuf), BuildRunnerError> {
        let build_base = work_dir.join("build").join(pkg_name);
        let src_dir = build_base.join("src");
        let build_dir = build_base.join("build");

        if build_base.exists() {
            std::fs::remove_dir_all(&build_base)?;
        }
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&build_dir)?;

        Ok((src_dir, build_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_runner_new() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);
        assert_eq!(runner.sources_dir, dir.path());
        assert!(!runner.skip_verify);
    }

    #[test]
    fn test_build_runner_skip_verify() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = BootstrapConfig::new();
        config.skip_verify = true;
        let runner = PackageBuildRunner::new(dir.path(), &config);
        assert!(runner.skip_verify);
    }

    #[test]
    fn test_verify_checksum_placeholder_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);

        let file = dir.path().join("test.tar.gz");
        std::fs::write(&file, b"test").unwrap();

        let result = runner.verify_checksum("test", "VERIFY_BEFORE_BUILD", &file);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_checksum_placeholder_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = BootstrapConfig::new();
        config.skip_verify = true;
        let runner = PackageBuildRunner::new(dir.path(), &config);

        let file = dir.path().join("test.tar.gz");
        std::fs::write(&file, b"test").unwrap();

        let result = runner.verify_checksum("test", "VERIFY_BEFORE_BUILD", &file);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_checksum_invalid_format() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);

        let file = dir.path().join("test.tar.gz");
        std::fs::write(&file, b"test").unwrap();

        let result = runner.verify_checksum("test", "nocolon", &file);
        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_build_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);

        let (src_dir, build_dir) = runner.prepare_build_dirs(dir.path(), "test-pkg").unwrap();
        assert!(src_dir.exists());
        assert!(build_dir.exists());
        assert!(src_dir.ends_with("src"));
        assert!(build_dir.ends_with("build"));
    }

    #[test]
    fn test_prepare_build_dirs_cleans_previous() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);

        // Create a file in the build dir
        let build_base = dir.path().join("build").join("test-pkg");
        std::fs::create_dir_all(&build_base).unwrap();
        std::fs::write(build_base.join("old-file"), b"old").unwrap();

        let (src_dir, _) = runner.prepare_build_dirs(dir.path(), "test-pkg").unwrap();
        assert!(src_dir.exists());
        assert!(!build_base.join("old-file").exists());
    }
}
