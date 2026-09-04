// conary-core/src/bootstrap/build_runner.rs

//! Shared package build runner for bootstrap stages
//!
//! Extracts the common source-fetching, checksum-verification, and extraction
//! logic that was duplicated across Stage 1, Stage 2, and Base builders.

use super::build_helpers;
use crate::hash::{Hash, HashAlgorithm, verify_file};
use crate::recipe::{Recipe, SourceSection, is_remote_url};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Build context determines how configure/make are invoked.
///
/// Each bootstrap phase has different requirements for how packages are built:
/// cross-compilation uses `--host` and `--build` flags with a sysroot,
/// chroot builds run inside an isolated root, and native builds use the
/// host system directly.
#[derive(Debug, Clone)]
pub enum BuildContext {
    /// Cross-compilation: `--host=$LFS_TGT --build=$(config.guess)`
    Cross {
        /// Target triplet (e.g., "x86_64-conary-linux-gnu")
        host: String,
        /// Sysroot path (e.g., /mnt/lfs)
        sysroot: PathBuf,
    },
    /// Native build inside chroot
    Chroot {
        /// Chroot root path
        root: PathBuf,
    },
    /// Native build on host (default, current behavior)
    Native,
}

/// Checksum contract for a bootstrap phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumContract {
    /// Verify every checksum algorithm implemented by Conary.
    Supported,
    /// Accept only SHA-256 for security-sensitive self-hosting inputs.
    Sha256Only,
}

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
    /// Checksum contract for this build phase.
    checksum_contract: ChecksumContract,
    /// Optional build context for cross-compilation or chroot builds
    context: Option<BuildContext>,
}

fn gnu_fetch_candidates(url: &str) -> Vec<String> {
    let mut candidates = vec![url.to_string()];

    for prefix in ["https://ftpmirror.gnu.org/", "http://ftpmirror.gnu.org/"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            candidates.push(format!("https://ftp.gnu.org/gnu/{rest}"));
            break;
        }
    }

    candidates
}

impl PackageBuildRunner {
    /// Create a new build runner
    pub fn new(sources_dir: &Path) -> Self {
        Self {
            sources_dir: sources_dir.to_path_buf(),
            checksum_contract: ChecksumContract::Supported,
            context: None,
        }
    }

    /// Set the build context for cross-compilation or chroot builds.
    ///
    /// Returns `self` for builder-style chaining. When no context is set
    /// (the default), the runner uses native build behavior.
    #[must_use]
    pub fn with_context(mut self, context: BuildContext) -> Self {
        self.context = Some(context);
        self
    }

    /// Override the checksum contract for this runner.
    #[must_use]
    pub fn with_checksum_contract(mut self, checksum_contract: ChecksumContract) -> Self {
        self.checksum_contract = checksum_contract;
        self
    }

    /// Get the current build context, if any.
    pub fn context(&self) -> Option<&BuildContext> {
        self.context.as_ref()
    }

    fn fetch_artifact_to_cache(
        &self,
        pkg_name: &str,
        url: &str,
        checksum: &str,
        filename: &str,
    ) -> Result<PathBuf, BuildRunnerError> {
        let target_path = self.sources_dir.join(filename);

        if target_path.exists() {
            match self.verify_checksum(pkg_name, checksum, &target_path) {
                Ok(()) => {
                    info!("  Using cached source (checksum verified): {}", filename);
                    return Ok(target_path);
                }
                Err(e) => {
                    warn!(
                        "  Cached source {} failed verification: {e} -- re-downloading",
                        filename
                    );
                    if let Err(rm_err) = fs::remove_file(&target_path) {
                        warn!("  Failed to remove corrupted cache file: {rm_err}");
                    }
                }
            }
        }

        let target_str = target_path
            .to_str()
            .ok_or_else(|| BuildRunnerError::InvalidPath(target_path.clone()))?;

        let mut last_reason = String::new();
        for (idx, candidate) in gnu_fetch_candidates(url).iter().enumerate() {
            if idx == 0 {
                info!("  Fetching: {}", candidate);
            } else {
                warn!("  Primary GNU mirror failed, retrying with fallback: {candidate}");
            }

            let output = Command::new("curl")
                .args([
                    "-fsSL",
                    "--connect-timeout",
                    "30",
                    "--max-time",
                    "600",
                    "--retry",
                    "3",
                    "-o",
                    target_str,
                    candidate,
                ])
                .output()
                .map_err(|e| BuildRunnerError::SourceFetchFailed {
                    package: pkg_name.to_string(),
                    reason: e.to_string(),
                })?;

            if output.status.success() {
                last_reason.clear();
                break;
            }

            last_reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }

        if !last_reason.is_empty() {
            return Err(BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: last_reason,
            });
        }

        self.verify_checksum(pkg_name, checksum, &target_path)?;

        Ok(target_path)
    }

    /// Fetch the primary source archive for a package, returning the local path.
    ///
    /// Downloads the archive if not already cached in `sources_dir`.
    pub fn fetch_source(
        &self,
        pkg_name: &str,
        recipe: &Recipe,
    ) -> Result<PathBuf, BuildRunnerError> {
        let SourceSection::Remote(source) = &recipe.source else {
            return Err(BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: "bootstrap source fetch requires an archive source".to_string(),
            });
        };
        let url = recipe.archive_url();
        let filename = recipe.archive_filename();
        self.fetch_artifact_to_cache(pkg_name, &url, &source.checksum, &filename)
    }

    /// Verify a typed checksum, rejecting placeholder identities.
    pub fn verify_checksum(
        &self,
        pkg_name: &str,
        expected: &str,
        path: &Path,
    ) -> Result<(), BuildRunnerError> {
        if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
            return Err(BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: format!("Recipe has placeholder checksum '{expected}'"),
            });
        }

        let expected_hash = Hash::parse_prefixed(expected).map_err(|error| {
            BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: format!("Invalid checksum: {error}"),
            }
        })?;

        if self.checksum_contract == ChecksumContract::Sha256Only
            && expected_hash.algorithm != HashAlgorithm::Sha256
        {
            return Err(BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: format!(
                    "Checksum contract requires sha256, got {}",
                    expected_hash.algorithm
                ),
            });
        }

        verify_file(path, &expected_hash.value, expected_hash.algorithm).map_err(|error| {
            BuildRunnerError::SourceFetchFailed {
                package: pkg_name.to_string(),
                reason: error.to_string(),
            }
        })
    }

    /// Stage additional sources into the package root and optionally extract them.
    ///
    /// Raw archives are copied into `package_root` so recipes can unpack them later
    /// via relative paths like `../foo.tar.xz` or `../../foo.tar.xz`.
    pub fn stage_additional_sources(
        &self,
        pkg_name: &str,
        recipe: &Recipe,
        package_root: &Path,
        src_dir: &Path,
    ) -> Result<(), BuildRunnerError> {
        let SourceSection::Remote(source) = &recipe.source else {
            return Ok(());
        };

        for additional in &source.additional {
            let url = recipe.substitute(&additional.url, "");
            let filename = url.split('/').next_back().unwrap_or("additional.tar.gz");
            let cached_path =
                self.fetch_artifact_to_cache(pkg_name, &url, &additional.checksum, filename)?;
            let staged_path = package_root.join(filename);
            fs::copy(&cached_path, &staged_path)?;

            if !additional.extract {
                continue;
            }

            let extract_dest = if let Some(dest) = &additional.extract_to {
                src_dir.join(dest)
            } else {
                src_dir.to_path_buf()
            };

            self.extract_source_strip(&staged_path, &extract_dest)?;
        }

        Ok(())
    }

    /// Stage recipe patches into the package root and apply them to the source tree.
    pub fn stage_and_apply_patches(
        &self,
        pkg_name: &str,
        recipe: &Recipe,
        package_root: &Path,
        src_dir: &Path,
    ) -> Result<(), BuildRunnerError> {
        let Some(patches) = &recipe.patches else {
            return Ok(());
        };

        let patch_root = package_root.join("patches");
        fs::create_dir_all(&patch_root)?;

        for patch_info in &patches.files {
            let staged_patch = if is_remote_url(&patch_info.file) {
                let filename = patch_info
                    .file
                    .split('/')
                    .next_back()
                    .unwrap_or("patch.diff");
                let checksum = patch_info.checksum.as_ref().ok_or_else(|| {
                    BuildRunnerError::SourceFetchFailed {
                        package: pkg_name.to_string(),
                        reason: format!("Remote patch '{}' has no checksum", patch_info.file),
                    }
                })?;
                let cached =
                    self.fetch_artifact_to_cache(pkg_name, &patch_info.file, checksum, filename)?;
                let staged = patch_root.join(filename);
                fs::copy(cached, &staged)?;
                staged
            } else {
                let source_patch = PathBuf::from(&patch_info.file);
                let filename = source_patch
                    .file_name()
                    .ok_or_else(|| BuildRunnerError::InvalidPath(source_patch.clone()))?;
                let staged = patch_root.join(filename);
                fs::copy(&source_patch, &staged)?;
                staged
            };

            let output = Command::new("patch")
                .arg(format!("-Np{}", patch_info.strip))
                .arg("-i")
                .arg(&staged_patch)
                .current_dir(src_dir)
                .output()
                .map_err(|e| BuildRunnerError::BuildFailed {
                    package: pkg_name.to_string(),
                    reason: format!("failed to execute patch: {e}"),
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(BuildRunnerError::BuildFailed {
                    package: pkg_name.to_string(),
                    reason: format!("patch apply failed:\n{stderr}"),
                });
            }
        }

        Ok(())
    }

    /// Extract a tar archive
    pub fn extract_source(&self, archive: &Path, dest: &Path) -> Result<(), BuildRunnerError> {
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
        build_helpers::extract_tar(archive, dest, true).map_err(|e| BuildRunnerError::BuildFailed {
            package: "extract".to_string(),
            reason: e,
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
mod tests;
