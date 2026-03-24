// conary-core/src/bootstrap/temp_tools.rs

//! Phase 2: Temporary tools (LFS Chapters 6-7)
//!
//! Uses the Phase 1 cross-toolchain to build a set of utilities that will
//! run inside the chroot. Chapter 6 cross-compiles packages using the
//! `$LFS_TGT`-prefixed tools. Chapter 7 sets up the chroot environment
//! and builds a handful of packages natively inside it.
//!
//! After this phase the chroot contains enough tools (bash, coreutils,
//! make, etc.) to build the final system without any host dependencies.

use std::path::{Path, PathBuf};
use tracing::info;

use super::build_runner::PackageBuildRunner;
use super::chroot_env::ChrootEnv;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;
use crate::recipe::parser::parse_recipe_file;
use crate::recipe::{Kitchen, KitchenConfig};

/// Cross-compiled packages (LFS Chapter 6).
///
/// Built on the host using the Phase 1 cross-toolchain, installed into
/// `$LFS/` so they are available once we enter the chroot.
const CH6_PACKAGES: [&str; 17] = [
    "m4",
    "ncurses",
    "bash",
    "coreutils",
    "diffutils",
    "file",
    "findutils",
    "gawk",
    "grep",
    "gzip",
    "make",
    "patch",
    "sed",
    "tar",
    "xz",
    "binutils-pass2",
    "gcc-pass2",
];

/// Chroot packages (LFS Chapter 7).
///
/// Built natively inside the chroot after `setup_chroot()` prepares the
/// virtual kernel filesystems and directory structure.
const CH7_PACKAGES: [&str; 6] = [
    "gettext",
    "bison",
    "perl",
    "python",
    "texinfo",
    "util-linux",
];

/// Errors specific to the temporary tools build phase.
#[derive(Debug, thiserror::Error)]
pub enum TempToolsError {
    /// A package build step failed.
    #[error("Temp-tools build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    /// Phase 1 cross-tools are missing.
    #[error("Phase 1 cross-tools not found at {0}")]
    MissingCrossTools(PathBuf),

    /// Chroot setup failed.
    #[error("Chroot setup failed: {0}")]
    ChrootSetup(String),

    /// Verification failed.
    #[error("Temp-tools verification failed: {0}")]
    Verification(String),

    /// I/O error during the build.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from the shared build runner.
    #[error(transparent)]
    BuildRunner(#[from] super::build_runner::BuildRunnerError),
}

/// Builder for Phase 2 temporary tools.
///
/// First cross-compiles the Chapter 6 packages using the Phase 1
/// cross-toolchain, then sets up the chroot and builds Chapter 7
/// packages natively inside it.
pub struct TempToolsBuilder {
    /// Working directory for build artifacts.
    work_dir: PathBuf,
    /// Root of the LFS filesystem.
    lfs_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// Phase 1 cross-toolchain (from `$LFS/tools/`).
    cross_toolchain: Toolchain,
    /// Shared build runner for source fetching and verification.
    runner: PackageBuildRunner,
}

impl TempToolsBuilder {
    /// Create a new temporary tools builder.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - scratch space for downloads and build trees
    /// * `lfs_root` - root of the LFS partition
    /// * `config` - bootstrap configuration
    /// * `cross_toolchain` - the Phase 1 cross-toolchain
    ///
    /// # Errors
    ///
    /// Returns `TempToolsError::MissingCrossTools` if `$LFS/tools/bin`
    /// does not exist.
    pub fn new(
        work_dir: &Path,
        lfs_root: &Path,
        config: BootstrapConfig,
        cross_toolchain: Toolchain,
    ) -> Result<Self, TempToolsError> {
        let tools_bin = lfs_root.join("tools").join("bin");
        if !tools_bin.exists() {
            return Err(TempToolsError::MissingCrossTools(tools_bin));
        }

        let sources_dir = work_dir.join("sources");
        std::fs::create_dir_all(&sources_dir)?;

        let runner = PackageBuildRunner::new(&sources_dir, &config);

        Ok(Self {
            work_dir: work_dir.to_path_buf(),
            lfs_root: lfs_root.to_path_buf(),
            config,
            cross_toolchain,
            runner,
        })
    }

    /// Cross-compile all Chapter 6 packages.
    ///
    /// Uses the Phase 1 cross-toolchain to build each package and installs
    /// the results into `$LFS/`. Accepts `completed` for resume support --
    /// packages whose names appear in the slice are skipped.
    pub fn build_cross_packages(&self, completed: &[String]) -> Result<(), TempToolsError> {
        info!(
            "Phase 2a: Cross-compiling temp tools ({} packages)",
            CH6_PACKAGES.len()
        );

        // Build the hermetic environment map that every child process needs.
        // Passed explicitly to each Command via KitchenConfig::extra_env so we
        // never touch the process-wide environment (which would be UB in a
        // multi-threaded context per Rust 1.83+).
        let tools_bin = self.lfs_root.join("tools/bin");
        let host_path = std::env::var("PATH").unwrap_or_default();
        let bootstrap_env: Vec<(String, String)> = vec![
            ("LFS".into(), self.lfs_root.display().to_string()),
            ("LFS_TGT".into(), self.cross_toolchain.target.clone()),
            ("LC_ALL".into(), "C".into()),
            ("TZ".into(), "UTC".into()),
            ("SOURCE_DATE_EPOCH".into(), "0".into()),
            (
                "PATH".into(),
                format!("{}:{host_path}", tools_bin.display()),
            ),
        ];

        for (i, pkg) in CH6_PACKAGES.iter().enumerate() {
            if completed.contains(&(*pkg).to_string()) {
                info!("Skipping already-completed: {}", pkg);
                continue;
            }
            info!(
                "Cross-compiling [{}/{}]: {}",
                i + 1,
                CH6_PACKAGES.len(),
                pkg
            );

            let recipe_path =
                std::path::Path::new("recipes/temp-tools").join(format!("{pkg}.toml"));
            let recipe =
                parse_recipe_file(&recipe_path).map_err(|e| TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Failed to parse recipe: {e}"),
                })?;

            info!("  Fetching source for {pkg}...");
            self.runner
                .fetch_source(pkg, &recipe)
                .map_err(|e| TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Source fetch failed: {e}"),
                })?;

            let config = KitchenConfig {
                source_cache: self.work_dir.join("sources"),
                jobs: self.config.jobs as u32,
                use_isolation: false,
                extra_env: bootstrap_env.clone(),
                ..Default::default()
            };
            let kitchen = Kitchen::new(config);
            let mut cook = kitchen
                .new_cook_with_dest(&recipe, std::path::Path::new("/"))
                .map_err(|e| TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Cook setup failed: {e}"),
                })?;

            info!("  Preparing {pkg}...");
            cook.prep().map_err(|e| TempToolsError::BuildFailed {
                package: pkg.to_string(),
                reason: format!("Prep failed: {e}"),
            })?;
            cook.unpack().map_err(|e| TempToolsError::BuildFailed {
                package: pkg.to_string(),
                reason: format!("Unpack failed: {e}"),
            })?;
            cook.patch().map_err(|e| TempToolsError::BuildFailed {
                package: pkg.to_string(),
                reason: format!("Patch failed: {e}"),
            })?;

            info!("  Building {pkg}...");
            cook.simmer().map_err(|e| TempToolsError::BuildFailed {
                package: pkg.to_string(),
                reason: format!("Build failed: {e}"),
            })?;

            info!("  [OK] {pkg} built successfully");
        }
        info!("Phase 2a complete: all Chapter 6 packages cross-compiled");
        Ok(())
    }

    /// Set up the chroot environment.
    ///
    /// Creates essential directories, device nodes, and virtual kernel
    /// filesystems (`/dev`, `/proc`, `/sys`, `/run`) inside `$LFS/`.
    /// Returns a [`ChrootEnv`] that the caller manages (teardown on drop).
    pub fn setup_chroot(&self) -> Result<ChrootEnv, TempToolsError> {
        info!(
            "Setting up chroot environment at {}",
            self.lfs_root.display()
        );

        let mut env = ChrootEnv::new(&self.lfs_root);
        env.setup()
            .map_err(|e| TempToolsError::ChrootSetup(e.to_string()))?;
        Ok(env)
    }

    /// Build Chapter 7 packages inside the chroot.
    ///
    /// These are built natively (not cross-compiled) using the tools
    /// that are now available inside the chroot. Accepts `completed` for
    /// resume support -- packages whose names appear in the slice are skipped.
    pub fn build_chroot_packages(&self, completed: &[String]) -> Result<(), TempToolsError> {
        info!(
            "Phase 2b: Building chroot packages ({} packages)",
            CH7_PACKAGES.len()
        );

        for (i, pkg) in CH7_PACKAGES.iter().enumerate() {
            if completed.contains(&(*pkg).to_string()) {
                info!("Skipping already-completed: {}", pkg);
                continue;
            }
            info!(
                "Building in chroot [{}/{}]: {}",
                i + 1,
                CH7_PACKAGES.len(),
                pkg
            );

            let recipe_path =
                std::path::Path::new("recipes/temp-tools").join(format!("{pkg}.toml"));
            let recipe =
                parse_recipe_file(&recipe_path).map_err(|e| TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Failed to parse recipe: {e}"),
                })?;

            // Fetch source to $LFS/sources/ (accessible inside chroot)
            info!("  Fetching source for {pkg}...");
            self.runner
                .fetch_source(pkg, &recipe)
                .map_err(|e| TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Source fetch failed: {e}"),
                })?;

            // Assemble build script and run in chroot
            let script = super::assemble_build_script(&recipe, "/");
            let env = self.chroot_env_vars();

            info!("  Building {pkg} in chroot...");
            let output = std::process::Command::new("chroot")
                .arg(&self.lfs_root)
                .arg("/bin/sh")
                .arg("-c")
                .arg(&script)
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .output()
                .map_err(|e| TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Failed to execute chroot: {e}"),
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(TempToolsError::BuildFailed {
                    package: pkg.to_string(),
                    reason: format!("Build failed in chroot:\n{stderr}"),
                });
            }

            info!("  [OK] {pkg} built successfully in chroot");
        }
        info!("Phase 2b complete: all Chapter 7 packages built");
        Ok(())
    }

    /// Environment variables for chroot builds (hermetic -- `env_clear()` first).
    fn chroot_env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("PATH".into(), "/usr/bin:/usr/sbin".into()),
            ("HOME".into(), "/root".into()),
            ("TERM".into(), "xterm".into()),
            ("LC_ALL".into(), "C".into()),
            ("TZ".into(), "UTC".into()),
            ("SOURCE_DATE_EPOCH".into(), "0".into()),
            ("MAKEFLAGS".into(), format!("-j{}", self.config.jobs)),
            ("LFS_TGT".into(), self.cross_toolchain.target.clone()),
        ]
    }

    /// Verify that the temporary tools environment is functional.
    ///
    /// Checks that key binaries exist and are executable inside the
    /// chroot root.
    pub fn verify(&self) -> Result<(), TempToolsError> {
        info!("Verifying temporary tools...");

        let essential_binaries = ["bash", "cat", "ls", "make", "gcc"];

        for bin in &essential_binaries {
            let path = self.lfs_root.join("usr").join("bin").join(bin);
            if !path.exists() {
                // Also check tools/bin as a fallback
                let tools_path = self.lfs_root.join("tools").join("bin").join(bin);
                if !tools_path.exists() {
                    return Err(TempToolsError::Verification(format!(
                        "Essential binary not found: {bin}"
                    )));
                }
            }
        }

        let _ = &self.config;
        info!("Temporary tools verification passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::toolchain::ToolchainKind;

    #[test]
    fn test_ch6_package_count() {
        assert_eq!(CH6_PACKAGES.len(), 17);
    }

    #[test]
    fn test_ch7_package_count() {
        assert_eq!(CH7_PACKAGES.len(), 6);
    }

    #[test]
    fn test_new_requires_tools_bin() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        // No tools/bin directory created
        let config = BootstrapConfig::new();
        let cross_tc = Toolchain {
            kind: ToolchainKind::CrossTools,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let result = TempToolsBuilder::new(work.path(), lfs.path(), config, cross_tc);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_succeeds_with_tools_bin() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("tools/bin")).unwrap();

        let config = BootstrapConfig::new();
        let cross_tc = Toolchain {
            kind: ToolchainKind::CrossTools,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = TempToolsBuilder::new(work.path(), lfs.path(), config, cross_tc);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_cross_packages_placeholder() {
        if !std::path::Path::new("recipes/cross-tools").exists() {
            eprintln!("Skipping: recipes/cross-tools not found in cwd");
            return;
        }

        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("tools/bin")).unwrap();

        let config = BootstrapConfig::new();
        let cross_tc = Toolchain {
            kind: ToolchainKind::CrossTools,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = TempToolsBuilder::new(work.path(), lfs.path(), config, cross_tc).unwrap();
        assert!(builder.build_cross_packages(&[]).is_ok());
    }
}
