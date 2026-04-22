// conary-core/src/bootstrap/final_system.rs

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
use super::toolchain::Toolchain;
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
    /// Working directory for build artifacts.
    // TODO(bootstrap): used when build artifacts are written to a staging area
    // separate from the chroot root (e.g. for incremental/resumable builds).
    #[allow(dead_code)]
    work_dir: PathBuf,
    /// Root of the LFS filesystem (chroot root).
    lfs_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// Toolchain available inside the chroot.
    // TODO(bootstrap): used when chroot builds switch to toolchain-aware
    // environment setup via build_helpers::setup_build_env.
    #[allow(dead_code)]
    toolchain: Toolchain,
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
    /// * `toolchain` - toolchain available inside the chroot
    ///
    /// # Errors
    ///
    /// Returns `FinalSystemError::ChrootNotReady` if `lfs_root` does not
    /// look like a prepared chroot (missing `/usr/bin`).
    pub fn new(
        work_dir: &Path,
        lfs_root: &Path,
        config: BootstrapConfig,
        toolchain: Toolchain,
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

        let runner = PackageBuildRunner::new(&sources_dir, &config);

        Ok(Self {
            work_dir: work_dir.to_path_buf(),
            lfs_root: lfs_root.to_path_buf(),
            config,
            toolchain,
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
mod tests {
    use super::*;
    use crate::bootstrap::stages::StageManager;
    use crate::bootstrap::toolchain::ToolchainKind;

    fn workspace_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|dir| dir.join("recipes/system").is_dir())
            .expect("workspace root not found from crate manifest ancestors")
    }

    #[test]
    fn test_system_build_order_count() {
        assert_eq!(SYSTEM_BUILD_ORDER.len(), 83);
    }

    #[test]
    fn test_system_build_order_starts_with_man_pages() {
        assert_eq!(SYSTEM_BUILD_ORDER[0], "man-pages");
    }

    #[test]
    fn test_system_build_order_ends_with_linux() {
        assert_eq!(SYSTEM_BUILD_ORDER[82], "linux");
    }

    #[test]
    fn test_system_build_order_includes_sqlite_before_python() {
        let sqlite_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "sqlite")
            .expect("sqlite in system build order");
        let python_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "python")
            .expect("python in system build order");

        assert!(sqlite_idx < python_idx);
    }

    #[test]
    fn test_system_build_order_includes_pyelftools_before_systemd() {
        let pyelftools_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "pyelftools")
            .expect("pyelftools in system build order");
        let systemd_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "systemd")
            .expect("systemd in system build order");

        assert!(pyelftools_idx < systemd_idx);
    }

    #[test]
    fn test_system_build_order_includes_composefs_after_meson_before_kmod() {
        let composefs_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "composefs")
            .expect("composefs in system build order");
        let meson_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "meson")
            .expect("meson in system build order");
        let kmod_idx = SYSTEM_BUILD_ORDER
            .iter()
            .position(|pkg| *pkg == "kmod")
            .expect("kmod in system build order");

        assert!(meson_idx < composefs_idx);
        assert!(composefs_idx < kmod_idx);
    }

    #[test]
    fn test_system_build_order_includes_linux_kernel() {
        assert!(
            SYSTEM_BUILD_ORDER.contains(&"linux"),
            "Phase 5 image generation requires the kernel recipe to run during bootstrap"
        );
    }

    #[test]
    fn test_system_build_order_tracks_lfs13_selection_with_conary_bootloader_deviation() {
        assert!(SYSTEM_BUILD_ORDER.contains(&"lz4"));
        assert!(SYSTEM_BUILD_ORDER.contains(&"pcre2"));
        assert!(SYSTEM_BUILD_ORDER.contains(&"packaging"));
        assert!(SYSTEM_BUILD_ORDER.contains(&"elfutils"));
        assert!(SYSTEM_BUILD_ORDER.contains(&"pyelftools"));
        assert!(SYSTEM_BUILD_ORDER.contains(&"linux"));
        assert!(!SYSTEM_BUILD_ORDER.contains(&"check"));
        assert!(!SYSTEM_BUILD_ORDER.contains(&"grub"));
    }

    #[test]
    fn test_system_build_order_has_recipe_files() {
        for pkg in SYSTEM_BUILD_ORDER {
            let filename = FinalSystemBuilder::recipe_filename(pkg);
            let recipe_path = workspace_root()
                .join("recipes/system")
                .join(format!("{filename}.toml"));
            assert!(
                recipe_path.is_file(),
                "missing Phase 3 recipe file for {pkg}: {}",
                recipe_path.display()
            );
        }
    }

    #[test]
    fn test_new_requires_usr_bin() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let result = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_succeeds_with_usr_bin() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_build_all_placeholder() {
        if !std::path::Path::new("recipes/cross-tools").exists() {
            eprintln!("Skipping: recipes/cross-tools not found in cwd");
            return;
        }

        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let mut sm = StageManager::new(work.path()).unwrap();
        let mut builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        assert!(builder.build_all(&[], &mut sm).is_ok());
        assert_eq!(builder.completed().len(), 83);
    }

    #[test]
    fn test_build_from_resume() {
        if !std::path::Path::new("recipes/cross-tools").exists() {
            eprintln!("Skipping: recipes/cross-tools not found in cwd");
            return;
        }

        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let mut sm = StageManager::new(work.path()).unwrap();
        let mut builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        assert!(builder.build_from("gcc", &mut sm).is_ok());
        // gcc is at index 27, so 83 - 27 = 56 remaining
        assert_eq!(builder.completed().len(), 56);
    }

    #[test]
    fn test_build_from_invalid_package() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let mut sm = StageManager::new(work.path()).unwrap();
        let mut builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        let result = builder.build_from("nonexistent-package", &mut sm);
        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_chroot_build_dirs_uses_sysroot_staging_area() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        let (src_dir, build_dir) = builder.prepare_chroot_build_dirs("man-pages").unwrap();

        assert_eq!(
            src_dir,
            lfs.path()
                .join("var/tmp/conary-bootstrap/final-system/man-pages/src")
        );
        assert_eq!(
            build_dir,
            lfs.path()
                .join("var/tmp/conary-bootstrap/final-system/man-pages/build")
        );
    }

    #[test]
    fn test_path_in_chroot_rewrites_sysroot_staging_paths() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        let staged_src = lfs
            .path()
            .join("var/tmp/conary-bootstrap/final-system/man-pages/src");

        assert_eq!(
            builder.path_in_chroot(&staged_src).unwrap(),
            "/var/tmp/conary-bootstrap/final-system/man-pages/src"
        );
    }

    #[test]
    fn test_setup_chroot_creates_virtual_fs_directories() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        let _ = builder.setup_chroot();

        assert!(lfs.path().join("dev").exists());
        assert!(lfs.path().join("proc").exists());
        assert!(lfs.path().join("sys").exists());
        assert!(lfs.path().join("run").exists());
    }

    #[test]
    fn test_setup_chroot_repairs_missing_shadow_prerequisite_groups() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("usr/bin")).unwrap();
        std::fs::create_dir_all(lfs.path().join("etc")).unwrap();
        std::fs::write(
            lfs.path().join("etc/group"),
            "root:x:0:\nwheel:x:10:\ntty:x:5:\nnogroup:x:65534:\n",
        )
        .unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        let _ = builder.setup_chroot();

        let group = std::fs::read_to_string(lfs.path().join("etc/group")).unwrap();
        assert!(group.contains("mail:x:34:"));
        assert!(group.contains("users:x:999:"));
        assert!(group.contains("wheel:x:10:"));
    }
}
