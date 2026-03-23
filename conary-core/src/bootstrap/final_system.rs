// conary-core/src/bootstrap/final_system.rs

//! Phase 3: Final system (LFS Chapter 8)
//!
//! Builds all 77 packages of the complete LFS system inside the chroot.
//! Each package is compiled from source using the temporary tools from
//! Phase 2. The build order follows LFS 13 Chapter 8 exactly.
//!
//! This phase produces a fully functional Linux system with a complete
//! toolchain (GCC, glibc, binutils), core utilities, and system
//! infrastructure.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;
use crate::recipe::parser::parse_recipe_file;

/// Complete build order for the final system (LFS Chapter 8).
///
/// All 77 packages in the order specified by LFS 13.
#[allow(dead_code)]
pub const SYSTEM_BUILD_ORDER: [&str; 77] = [
    "man-pages",
    "iana-etc",
    "glibc",
    "zlib",
    "bzip2",
    "xz",
    "zstd",
    "file",
    "readline",
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
    "kmod",
    "libelf",
    "libffi",
    "python",
    "flit-core",
    "wheel",
    "setuptools",
    "ninja",
    "meson",
    "coreutils",
    "check",
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
    "systemd",
    "dbus",
    "man-db",
    "procps-ng",
    "util-linux",
    "e2fsprogs",
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
/// Builds all 77 LFS Chapter 8 packages inside the chroot, tracking
/// progress so builds can be resumed after failure.
pub struct FinalSystemBuilder {
    /// Working directory for build artifacts.
    #[allow(dead_code)]
    work_dir: PathBuf,
    /// Root of the LFS filesystem (chroot root).
    lfs_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// Toolchain available inside the chroot.
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

    /// Build all 77 packages from the beginning.
    pub fn build_all(&mut self, already_completed: &[String]) -> Result<(), FinalSystemError> {
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
        }

        info!(
            "Phase 3 complete: all {} packages built",
            SYSTEM_BUILD_ORDER.len()
        );
        Ok(())
    }

    /// Resume building from a specific package.
    ///
    /// Skips all packages before `from_package` in the build order and
    /// builds from that point onward.
    ///
    /// # Errors
    ///
    /// Returns `FinalSystemError::InvalidResume` if `from_package` is not
    /// in `SYSTEM_BUILD_ORDER`.
    pub fn build_from(&mut self, from_package: &str) -> Result<(), FinalSystemError> {
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
        self.runner
            .fetch_source(name, &recipe)
            .map_err(|e| FinalSystemError::BuildFailed {
                package: name.to_string(),
                reason: format!("Source fetch failed: {e}"),
            })?;

        let script = super::assemble_build_script(&recipe, "/");
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
    use crate::bootstrap::toolchain::ToolchainKind;

    #[test]
    fn test_system_build_order_count() {
        assert_eq!(SYSTEM_BUILD_ORDER.len(), 77);
    }

    #[test]
    fn test_system_build_order_starts_with_man_pages() {
        assert_eq!(SYSTEM_BUILD_ORDER[0], "man-pages");
    }

    #[test]
    fn test_system_build_order_ends_with_e2fsprogs() {
        assert_eq!(SYSTEM_BUILD_ORDER[76], "e2fsprogs");
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

        let mut builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        assert!(builder.build_all(&[]).is_ok());
        assert_eq!(builder.completed().len(), 77);
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

        let mut builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        assert!(builder.build_from("gcc").is_ok());
        // gcc is at index 25, so 77 - 25 = 52 remaining
        assert_eq!(builder.completed().len(), 52);
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

        let mut builder = FinalSystemBuilder::new(work.path(), lfs.path(), config, tc).unwrap();
        let result = builder.build_from("nonexistent-package");
        assert!(result.is_err());
    }
}
