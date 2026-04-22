// conary-core/src/bootstrap/build_runner.rs

//! Shared package build runner for bootstrap stages
//!
//! Extracts the common source-fetching, checksum-verification, and extraction
//! logic that was duplicated across Stage 1, Stage 2, and Base builders.

use super::build_helpers;
use super::config::BootstrapConfig;
use crate::recipe::{Recipe, is_remote_url};
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

/// Checksum verification policy for a bootstrap phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumPolicy {
    /// Preserve the current behavior for earlier phases: verify `sha256`,
    /// reject placeholders, and warn on legacy/unknown algorithms.
    Legacy,
    /// Self-hosting / Tier 2 behavior: only `sha256` is accepted.
    StrictSha256,
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
    /// Whether to skip checksum verification
    skip_verify: bool,
    /// Checksum verification policy for this build phase.
    checksum_policy: ChecksumPolicy,
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
    pub fn new(sources_dir: &Path, config: &BootstrapConfig) -> Self {
        Self {
            sources_dir: sources_dir.to_path_buf(),
            skip_verify: config.skip_verify,
            checksum_policy: ChecksumPolicy::Legacy,
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

    /// Override the checksum policy for this runner.
    #[must_use]
    pub fn with_checksum_policy(mut self, checksum_policy: ChecksumPolicy) -> Self {
        self.checksum_policy = checksum_policy;
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
        let url = recipe.archive_url();
        let filename = recipe.archive_filename();
        self.fetch_artifact_to_cache(pkg_name, &url, &recipe.source.checksum, &filename)
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

        let (algo, hash) =
            expected
                .split_once(':')
                .ok_or_else(|| BuildRunnerError::SourceFetchFailed {
                    package: pkg_name.to_string(),
                    reason: "Invalid checksum format".to_string(),
                })?;

        match algo {
            "sha256" => {
                let output = Command::new("sha256sum").arg(path).output().map_err(|e| {
                    BuildRunnerError::SourceFetchFailed {
                        package: pkg_name.to_string(),
                        reason: e.to_string(),
                    }
                })?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                let computed = stdout.split_whitespace().next().unwrap_or("");
                if computed != hash {
                    return Err(BuildRunnerError::SourceFetchFailed {
                        package: pkg_name.to_string(),
                        reason: format!("Checksum mismatch: expected {}, got {}", hash, computed),
                    });
                }
            }
            other if self.checksum_policy == ChecksumPolicy::StrictSha256 => {
                return Err(BuildRunnerError::SourceFetchFailed {
                    package: pkg_name.to_string(),
                    reason: format!("Unsupported checksum algorithm in strict mode: {other}"),
                });
            }
            other => {
                warn!("  Unknown checksum algorithm: {}", other);
            }
        }

        Ok(())
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
        for additional in &recipe.source.additional {
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
mod tests {
    use super::*;
    use crate::recipe::parse_recipe;
    use std::path::Path;
    use std::process::Command;

    fn sha256_of(path: &Path) -> String {
        let output = Command::new("sha256sum").arg(path).output().unwrap();
        assert!(output.status.success(), "sha256sum should succeed in tests");
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }

    fn create_tarball(archive: &Path, top_level: &str, file_name: &str, contents: &str) {
        let tempdir = tempfile::tempdir().unwrap();
        let source_root = tempdir.path().join(top_level);
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join(file_name), contents).unwrap();

        let output = Command::new("tar")
            .arg("-czf")
            .arg(archive)
            .arg("-C")
            .arg(tempdir.path())
            .arg(top_level)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "test tarball creation should succeed"
        );
    }

    fn strict_mode_runner(sources_dir: &Path) -> PackageBuildRunner {
        let config = BootstrapConfig::new();
        PackageBuildRunner::new(sources_dir, &config)
            .with_checksum_policy(ChecksumPolicy::StrictSha256)
    }

    #[test]
    fn test_build_runner_new() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);
        assert_eq!(runner.sources_dir, dir.path());
        assert!(!runner.skip_verify);
        assert_eq!(runner.checksum_policy, ChecksumPolicy::Legacy);
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
    fn test_verify_checksum_unknown_algo_allowed_in_legacy_mode() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);

        let file = dir.path().join("test.tar.gz");
        std::fs::write(&file, b"test").unwrap();

        let result = runner.verify_checksum("test", "md5:d41d8cd98f00b204e9800998ecf8427e", &file);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_checksum_unknown_algo_rejected_in_strict_sha256_mode() {
        let dir = tempfile::tempdir().unwrap();
        let runner = strict_mode_runner(dir.path());

        let file = dir.path().join("test.tar.gz");
        std::fs::write(&file, b"test").unwrap();

        let result = runner.verify_checksum("test", "md5:d41d8cd98f00b204e9800998ecf8427e", &file);
        assert!(result.is_err());
    }

    #[test]
    fn test_gnu_fetch_candidates_adds_canonical_fallback_for_ftpmirror() {
        let candidates = gnu_fetch_candidates("https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz");
        assert_eq!(
            candidates,
            vec![
                "https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz".to_string(),
                "https://ftp.gnu.org/gnu/bash/bash-5.3.tar.gz".to_string()
            ]
        );
    }

    #[test]
    fn test_gnu_fetch_candidates_leaves_non_ftpmirror_urls_unchanged() {
        let candidates = gnu_fetch_candidates("https://example.invalid/src.tar.gz");
        assert_eq!(
            candidates,
            vec!["https://example.invalid/src.tar.gz".to_string()]
        );
    }

    #[test]
    fn test_stage_additional_sources_preserves_raw_archive_in_package_root() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);
        let package_root = dir.path().join("pkg");
        let src_dir = package_root.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let cached_archive = dir.path().join("dep-1.0.tar.gz");
        create_tarball(&cached_archive, "dep-1.0", "README", "hello\n");
        let digest = sha256_of(&cached_archive);
        let recipe = parse_recipe(&format!(
            r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.invalid/test-1.0.tar.gz"
checksum = "sha256:{digest}"

[[source.additional]]
url = "https://example.invalid/dep-1.0.tar.gz"
checksum = "sha256:{digest}"

[build]
install = "true"
"#
        ))
        .unwrap();

        runner
            .stage_additional_sources("test", &recipe, &package_root, &src_dir)
            .unwrap();

        assert!(
            package_root.join("dep-1.0.tar.gz").exists(),
            "raw additional archive should remain staged next to the package root"
        );
        assert!(
            src_dir.join("README").exists(),
            "extracting an additional source should populate the source tree"
        );
    }

    #[test]
    fn test_stage_additional_sources_skips_extraction_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);
        let package_root = dir.path().join("pkg");
        let src_dir = package_root.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let cached_archive = dir.path().join("tzdata.tar.gz");
        create_tarball(&cached_archive, "tzdata", "zone.tab", "UTC\n");
        let digest = sha256_of(&cached_archive);
        let recipe = parse_recipe(&format!(
            r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.invalid/test-1.0.tar.gz"
checksum = "sha256:{digest}"

[[source.additional]]
url = "https://example.invalid/tzdata.tar.gz"
checksum = "sha256:{digest}"
extract = false

[build]
install = "true"
"#
        ))
        .unwrap();

        runner
            .stage_additional_sources("test", &recipe, &package_root, &src_dir)
            .unwrap();

        assert!(package_root.join("tzdata.tar.gz").exists());
        assert!(
            !src_dir.join("zone.tab").exists(),
            "extract = false should keep the raw archive only"
        );
    }

    #[test]
    fn test_stage_and_apply_patches_copies_remote_patch_then_applies_it() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let runner = PackageBuildRunner::new(dir.path(), &config);
        let package_root = dir.path().join("pkg");
        let src_dir = package_root.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("hello.txt"), "before\n").unwrap();

        let patch_content = "\
--- a/hello.txt\n\
+++ b/hello.txt\n\
@@ -1 +1 @@\n\
-before\n\
+after\n";
        let cached_patch = dir.path().join("fix.patch");
        fs::write(&cached_patch, patch_content).unwrap();
        let digest = sha256_of(&cached_patch);
        let recipe = parse_recipe(&format!(
            r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.invalid/test-1.0.tar.gz"
checksum = "sha256:{digest}"

[patches]
files = [
  {{ file = "https://example.invalid/fix.patch", checksum = "sha256:{digest}", strip = 1 }},
]

[build]
install = "true"
"#
        ))
        .unwrap();

        runner
            .stage_and_apply_patches("test", &recipe, &package_root, &src_dir)
            .unwrap();

        assert!(
            package_root.join("patches/fix.patch").exists(),
            "remote patch should be staged into the package-local patches directory"
        );
        assert_eq!(
            fs::read_to_string(src_dir.join("hello.txt")).unwrap(),
            "after\n"
        );
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
