// crates/conary-core/src/bootstrap/final_system.rs

//! Phase 3: Final system (LFS Chapter 8)
//!
//! Builds the Chapter 8 final-system package set inside the chroot.
//! Each package is compiled from source using the temporary tools from
//! Phase 2. The build order follows LFS 13.0-systemd Chapter 8, except for
//! the documented Conary deviation that uses systemd-boot instead of the
//! standalone GRUB package in the qcow2 path.
//!
//! This phase produces a fully functional Linux system with a complete
//! toolchain (GCC, glibc, binutils), core utilities, and system
//! infrastructure.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

use super::build_runner::PackageBuildRunner;
use super::chroot_env::{ChrootEnv, ensure_bootstrap_identity_files};
use super::config::BootstrapConfig;
use super::stages::{BootstrapStage, StageManager};
use crate::recipe::parser::parse_recipe_file;

/// Complete build order for the final system and boot kernel.
///
/// This mirrors the LFS 13.0-systemd Chapter 8 package order, with Conary's
/// documented `systemd-boot` deviation: the standalone `grub` package is
/// omitted, and `pyelftools` is added before `systemd` because upstream
/// `systemd-259.1` now requires that Python module when `-Dbootloader=true`.
///
/// Conary also builds the `linux` kernel recipe at the end of Phase 3 so the
/// Phase 4/5 sysroot has concrete boot artifacts under `/boot`.
pub const SYSTEM_BUILD_ORDER: [&str; 83] = [
    "man-pages",
    "iana-etc",
    "glibc",
    "zlib",
    "bzip2",
    "xz",
    "lz4",
    "zstd",
    "file",
    "readline",
    "pcre2",
    "m4",
    "bc",
    "flex",
    "tcl",
    "expect",
    "dejagnu",
    "pkgconf",
    "binutils",
    "gmp",
    "mpfr",
    "mpc",
    "attr",
    "acl",
    "libcap",
    "libxcrypt",
    "shadow",
    "gcc",
    "ncurses",
    "sed",
    "psmisc",
    "gettext",
    "bison",
    "grep",
    "bash",
    "libtool",
    "gdbm",
    "gperf",
    "expat",
    "inetutils",
    "less",
    "perl",
    "xml-parser",
    "intltool",
    "autoconf",
    "automake",
    "openssl",
    "elfutils",
    "libffi",
    "sqlite",
    "python",
    "flit-core",
    "packaging",
    "wheel",
    "setuptools",
    "ninja",
    "meson",
    "composefs",
    "kmod",
    "coreutils",
    "diffutils",
    "gawk",
    "findutils",
    "groff",
    "gzip",
    "iproute2",
    "kbd",
    "libpipeline",
    "make",
    "patch",
    "tar",
    "texinfo",
    "vim",
    "markupsafe",
    "jinja2",
    "pyelftools",
    "systemd",
    "dbus",
    "man-db",
    "procps-ng",
    "util-linux",
    "e2fsprogs",
    "linux",
];

/// Errors specific to the final system build phase.
#[derive(Debug, thiserror::Error)]
pub enum FinalSystemError {
    /// A package build step failed.
    #[error("Final system build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    /// The chroot environment is not set up.
    #[error("Chroot not ready: {0}")]
    ChrootNotReady(String),

    /// Resume was requested but the checkpoint package was not found.
    #[error("Cannot resume from '{0}': not found in build order")]
    InvalidResume(String),

    /// Verification of the final system failed.
    #[error("Final system verification failed: {0}")]
    Verification(String),

    /// I/O error during the build.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from the shared build runner.
    #[error(transparent)]
    BuildRunner(#[from] super::build_runner::BuildRunnerError),
}

/// Builder for the Phase 3 final system.
///
/// Builds all Phase 3 final-system packages inside the chroot, tracking
/// progress so builds can be resumed after failure.
pub struct FinalSystemBuilder {
    /// Root of the LFS filesystem (chroot root).
    lfs_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// Shared build runner for source fetching and verification.
    runner: PackageBuildRunner,
    /// Packages that have been successfully built.
    completed: Vec<String>,
}

impl FinalSystemBuilder {
    /// Create a new final system builder.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - scratch space for downloads and build trees
    /// * `lfs_root` - root of the LFS partition (chroot root)
    /// * `config` - bootstrap configuration
    ///
    /// # Errors
    ///
    /// Returns `FinalSystemError::ChrootNotReady` if `lfs_root` does not
    /// look like a prepared chroot (missing `/usr/bin`).
    pub fn new(
        work_dir: &Path,
        lfs_root: &Path,
        config: BootstrapConfig,
    ) -> Result<Self, FinalSystemError> {
        let usr_bin = lfs_root.join("usr").join("bin");
        if !usr_bin.exists() {
            return Err(FinalSystemError::ChrootNotReady(format!(
                "Missing {}, run Phase 2 first",
                usr_bin.display()
            )));
        }

        let sources_dir = work_dir.join("sources");
        std::fs::create_dir_all(&sources_dir)?;

        let runner = PackageBuildRunner::new(&sources_dir);

        Ok(Self {
            lfs_root: lfs_root.to_path_buf(),
            config,
            runner,
            completed: Vec::new(),
        })
    }

    /// Build all Phase 3 packages from the beginning.
    ///
    /// `stage_manager` is used to persist per-package completions to disk
    /// immediately after each successful build, enabling crash-resumable
    /// Phase 3 runs.
    pub fn build_all(
        &mut self,
        already_completed: &[String],
        stage_manager: &mut StageManager,
    ) -> Result<(), FinalSystemError> {
        info!(
            "Phase 3: Building final system ({} packages)",
            SYSTEM_BUILD_ORDER.len()
        );

        for (i, pkg) in SYSTEM_BUILD_ORDER.iter().enumerate() {
            if already_completed.contains(&pkg.to_string()) {
                info!("Skipping already-completed: {}", pkg);
                continue;
            }
            info!(
                "Building system package [{}/{}]: {}",
                i + 1,
                SYSTEM_BUILD_ORDER.len(),
                pkg
            );
            self.build_package(pkg)?;
            self.completed.push((*pkg).to_string());
            // Persist per-package completion immediately so a crash during the
            // next package does not lose this one's progress.
            if let Err(e) = stage_manager.mark_package_complete(BootstrapStage::FinalSystem, pkg) {
                warn!("Failed to persist checkpoint for {pkg}: {e}");
            }
        }

        info!(
            "Phase 3 complete: all {} packages built",
            SYSTEM_BUILD_ORDER.len()
        );
        Ok(())
    }

    /// Set up the chroot environment for Phase 3 package builds.
    ///
    /// Creates the required virtual filesystem mounts and compatibility
    /// directories under the sysroot. The returned [`ChrootEnv`] must stay
    /// alive for the duration of the Phase 3 build and is cleaned up on drop.
    pub fn setup_chroot(&self) -> Result<ChrootEnv, FinalSystemError> {
        info!(
            "Setting up final-system chroot environment at {}",
            self.lfs_root.display()
        );

        ensure_bootstrap_identity_files(&self.lfs_root)
            .map_err(|e| FinalSystemError::ChrootNotReady(e.to_string()))?;

        let mut env = ChrootEnv::new(&self.lfs_root);
        env.setup()
            .map_err(|e| FinalSystemError::ChrootNotReady(e.to_string()))?;
        Ok(env)
    }

    /// Resume building from a specific package.
    ///
    /// Skips all packages before `from_package` in the build order and
    /// builds from that point onward.
    ///
    /// `stage_manager` is used to persist per-package completions to disk
    /// immediately after each successful build, enabling crash-resumable
    /// Phase 3 runs.
    ///
    /// # Errors
    ///
    /// Returns `FinalSystemError::InvalidResume` if `from_package` is not
    /// in `SYSTEM_BUILD_ORDER`.
    pub fn build_from(
        &mut self,
        from_package: &str,
        stage_manager: &mut StageManager,
    ) -> Result<(), FinalSystemError> {
        let start_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|&p| p == from_package)
            .ok_or_else(|| FinalSystemError::InvalidResume(from_package.to_string()))?;

        let remaining = SYSTEM_BUILD_ORDER.len() - start_idx;
        info!(
            "Resuming Phase 3 from '{}' ({} packages remaining)",
            from_package, remaining
        );

        for (i, pkg) in SYSTEM_BUILD_ORDER[start_idx..].iter().enumerate() {
            info!(
                "Building system package [{}/{}]: {}",
                start_idx + i + 1,
                SYSTEM_BUILD_ORDER.len(),
                pkg
            );
            self.build_package(pkg)?;
            self.completed.push((*pkg).to_string());
            // Persist per-package completion immediately so a crash during the
            // next package does not lose this one's progress.
            if let Err(e) = stage_manager.mark_package_complete(BootstrapStage::FinalSystem, pkg) {
                warn!("Failed to persist checkpoint for {pkg}: {e}");
            }
        }

        info!("Phase 3 resumed build complete");
        Ok(())
    }

    /// Map a package name to its recipe filename stem.
    ///
    /// Handles special cases like `libstdc++` → `libstdcxx`.
    fn recipe_filename(pkg: &str) -> String {
        pkg.replace("++", "xx").replace('+', "p")
    }

    /// Environment variables for chroot builds (hermetic — `env_clear()` first).
    fn chroot_env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("PATH".into(), "/usr/bin:/usr/sbin".into()),
            ("HOME".into(), "/root".into()),
            ("TERM".into(), "xterm".into()),
            ("LC_ALL".into(), "C".into()),
            ("TZ".into(), "UTC".into()),
            ("SOURCE_DATE_EPOCH".into(), "0".into()),
            ("MAKEFLAGS".into(), format!("-j{}", self.config.jobs)),
        ]
    }

    fn chroot_build_root(&self) -> PathBuf {
        self.lfs_root.join("var/tmp/conary-bootstrap/final-system")
    }

    fn prepare_chroot_build_dirs(
        &self,
        package: &str,
    ) -> Result<(PathBuf, PathBuf), FinalSystemError> {
        let package_root = self.chroot_build_root().join(package);
        let src_dir = package_root.join("src");
        let build_dir = package_root.join("build");

        if package_root.exists() {
            std::fs::remove_dir_all(&package_root)?;
        }
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&build_dir)?;

        Ok((src_dir, build_dir))
    }

    fn path_in_chroot(&self, host_path: &Path) -> Result<String, FinalSystemError> {
        let relative =
            host_path
                .strip_prefix(&self.lfs_root)
                .map_err(|_| FinalSystemError::BuildFailed {
                    package: "final-system".to_string(),
                    reason: format!(
                        "path {} is not inside sysroot {}",
                        host_path.display(),
                        self.lfs_root.display()
                    ),
                })?;

        Ok(format!("/{}", relative.display()))
    }

    /// Build a single package inside the chroot using its recipe.
    fn build_package(&self, name: &str) -> Result<(), FinalSystemError> {
        let filename = Self::recipe_filename(name);
        let recipe_path = std::path::Path::new("recipes/system").join(format!("{filename}.toml"));
        let recipe =
            parse_recipe_file(&recipe_path).map_err(|e| FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Failed to parse recipe: {e}"),
            })?;

        info!("  Fetching source for {name}...");
        let source_archive =
            self.runner
                .fetch_source(name, &recipe)
                .map_err(|e| FinalSystemError::BuildFailed {
                    package: name.to_string(),
                    reason: format!("Source fetch failed: {e}"),
                })?;

        let (src_dir, _build_dir) = self.prepare_chroot_build_dirs(name)?;
        let package_root = src_dir
            .parent()
            .expect("package source directory should have a package root");
        self.runner
            .extract_source_strip(&source_archive, &src_dir)
            .map_err(|e| FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Source extract failed: {e}"),
            })?;
        self.runner
            .stage_additional_sources(name, &recipe, package_root, &src_dir)
            .map_err(|e| FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Additional source staging failed: {e}"),
            })?;
        self.runner
            .stage_and_apply_patches(name, &recipe, package_root, &src_dir)
            .map_err(|e| FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Patch staging failed: {e}"),
            })?;

        let src_dir_in_chroot = self.path_in_chroot(&src_dir)?;
        let script = super::assemble_chroot_build_script(&recipe, &src_dir_in_chroot, "/");
        let env = self.chroot_env_vars();

        info!("  Building {name} in chroot...");
        let output = Command::new("chroot")
            .arg(&self.lfs_root)
            .arg("/bin/sh")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .output()
            .map_err(|e| FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Failed to execute chroot: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Build failed in chroot:\n{stderr}"),
            });
        }

        info!("  [OK] {name} built successfully");
        Ok(())
    }

    /// Verify the final system is functional.
    ///
    /// Checks that critical binaries and libraries exist in the chroot.
    pub fn verify(&self) -> Result<(), FinalSystemError> {
        info!("Verifying final system...");

        let critical = [
            "usr/bin/gcc",
            "usr/bin/bash",
            "usr/bin/make",
            "usr/bin/python3",
            "usr/lib/libc.so.6",
        ];

        for path in &critical {
            let full = self.lfs_root.join(path);
            if !full.exists() {
                warn!("Missing critical file: {}", full.display());
                return Err(FinalSystemError::Verification(format!(
                    "Critical file missing: {path}"
                )));
            }
        }

        info!(
            "Final system verification passed ({} packages completed)",
            self.completed.len()
        );
        Ok(())
    }

    /// Get the list of completed packages.
    pub fn completed(&self) -> &[String] {
        &self.completed
    }
}

#[cfg(test)]
mod tests;
