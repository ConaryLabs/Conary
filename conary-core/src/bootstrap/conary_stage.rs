// conary-core/src/bootstrap/conary_stage.rs

//! Conary stage: build Rust and Conary for self-hosting
//!
//! This is an optional stage that makes the bootstrapped system capable of
//! managing itself. It downloads a Rust bootstrap binary, builds Rust from
//! source targeting the new sysroot, then builds Conary with cargo.
//!
//! Skip with `--skip-conary` when building minimal or embedded images.

use super::config::{BootstrapConfig, TargetArch};
use std::path::PathBuf;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum ConaryStageError {
    #[error("Base system sysroot not found at {0}")]
    SysrootNotFound(PathBuf),

    #[error("Sysroot missing required component: {0}")]
    SysrootIncomplete(String),

    #[error("Rust bootstrap download failed: {0}")]
    RustBootstrapFailed(String),

    #[error("Rust build failed: {0}")]
    RustBuildFailed(String),

    #[error("Conary build failed: {0}")]
    ConaryBuildFailed(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Rust version to bootstrap with
const RUST_VERSION: &str = "1.94.0";

/// Conary stage packages
const CONARY_PACKAGES: &[&str] = &["rust", "conary"];

/// Builder for the Conary self-hosting stage.
///
/// Downloads a Rust bootstrap compiler, builds Rust from source targeting the
/// new sysroot, then compiles Conary itself with cargo.
pub struct ConaryStageBuilder {
    _work_dir: PathBuf,
    config: BootstrapConfig,
    sysroot: PathBuf,
}

impl ConaryStageBuilder {
    /// Create a new Conary stage builder.
    pub fn new(work_dir: PathBuf, config: BootstrapConfig, sysroot: PathBuf) -> Self {
        Self {
            _work_dir: work_dir,
            config,
            sysroot,
        }
    }

    /// Package names for this stage.
    pub fn package_names() -> &'static [&'static str] {
        CONARY_PACKAGES
    }

    /// Validate that the sysroot has the minimum requirements for building Rust.
    pub fn validate_sysroot(&self) -> Result<(), ConaryStageError> {
        if !self.sysroot.exists() {
            return Err(ConaryStageError::SysrootNotFound(self.sysroot.clone()));
        }

        // Check for essential components needed to build Rust
        let required = ["usr/bin/gcc", "usr/bin/make", "usr/lib/libc.so"];

        for component in &required {
            let path = self.sysroot.join(component);
            if !path.exists() {
                // Also check without usr/ prefix
                let alt = self
                    .sysroot
                    .join(component.strip_prefix("usr/").unwrap_or(component));
                if !alt.exists() {
                    return Err(ConaryStageError::SysrootIncomplete(
                        (*component).to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Get the Rust bootstrap URL for the target architecture.
    pub fn rust_bootstrap_url(&self) -> String {
        let target = match self.config.target_arch {
            TargetArch::X86_64 => "x86_64-unknown-linux-gnu",
            TargetArch::Aarch64 => "aarch64-unknown-linux-gnu",
            TargetArch::Riscv64 => "riscv64gc-unknown-linux-gnu",
        };
        format!("https://static.rust-lang.org/dist/rust-{RUST_VERSION}-{target}.tar.xz")
    }

    /// Download and install the Rust bootstrap binary into the sysroot.
    ///
    /// Downloads the official Rust binary distribution for the target architecture,
    /// extracts it, and runs `install.sh` to install `rustc` and `cargo` into
    /// `{sysroot}/usr/`.
    pub fn build_rust(&self) -> Result<PathBuf, ConaryStageError> {
        info!("Building Rust {} for sysroot", RUST_VERSION);

        let rust_dir = self._work_dir.join("rust");
        std::fs::create_dir_all(&rust_dir)?;

        // Download bootstrap binary
        let url = self.rust_bootstrap_url();
        let archive = rust_dir.join(format!("rust-{RUST_VERSION}.tar.xz"));

        if !archive.exists() {
            info!("Downloading Rust bootstrap from {}", url);
            let status = std::process::Command::new("curl")
                .args(["-fSL", "-o"])
                .arg(&archive)
                .arg(&url)
                .status()
                .map_err(|e| ConaryStageError::RustBootstrapFailed(e.to_string()))?;
            if !status.success() {
                return Err(ConaryStageError::RustBootstrapFailed(
                    "curl download failed".into(),
                ));
            }
        }

        // Extract
        let extract_dir = rust_dir.join("extract");
        if extract_dir.exists() {
            std::fs::remove_dir_all(&extract_dir)?;
        }
        std::fs::create_dir_all(&extract_dir)?;

        let status = std::process::Command::new("tar")
            .args(["xf"])
            .arg(&archive)
            .arg("-C")
            .arg(&extract_dir)
            .arg("--strip-components=1")
            .status()
            .map_err(|e| ConaryStageError::RustBootstrapFailed(e.to_string()))?;
        if !status.success() {
            return Err(ConaryStageError::RustBootstrapFailed(
                "tar extract failed".into(),
            ));
        }

        // Run installer
        let status = std::process::Command::new(extract_dir.join("install.sh"))
            .arg(format!("--prefix={}/usr", self.sysroot.display()))
            .status()
            .map_err(|e| ConaryStageError::RustBuildFailed(e.to_string()))?;
        if !status.success() {
            return Err(ConaryStageError::RustBuildFailed(
                "install.sh failed".into(),
            ));
        }

        // Verify
        let rustc = self.sysroot.join("usr/bin/rustc");
        let cargo = self.sysroot.join("usr/bin/cargo");
        if !rustc.exists() {
            return Err(ConaryStageError::RustBuildFailed(format!(
                "rustc not found at {}",
                rustc.display()
            )));
        }
        if !cargo.exists() {
            return Err(ConaryStageError::RustBuildFailed(format!(
                "cargo not found at {}",
                cargo.display()
            )));
        }

        info!("[COMPLETE] Rust {} installed to sysroot", RUST_VERSION);
        Ok(rustc)
    }

    /// Build Conary from source inside the sysroot.
    ///
    /// Copies the current source tree into the work directory, builds with the
    /// sysroot's cargo (installed by `build_rust()`), and installs the binary
    /// to `{sysroot}/usr/bin/conary`.
    pub fn build_conary(&self) -> Result<PathBuf, ConaryStageError> {
        info!("Building Conary in sysroot");

        let cargo = self.sysroot.join("usr/bin/cargo");
        if !cargo.exists() {
            return Err(ConaryStageError::ConaryBuildFailed(
                "cargo not found -- run build_rust() first".into(),
            ));
        }

        // Copy conary source into work directory
        let src_dir = self._work_dir.join("conary-src");
        if !src_dir.exists() {
            let status = std::process::Command::new("cp")
                .args(["-a", "."])
                .arg(&src_dir)
                .status()
                .map_err(|e| ConaryStageError::ConaryBuildFailed(e.to_string()))?;
            if !status.success() {
                return Err(ConaryStageError::ConaryBuildFailed(
                    "source copy failed".into(),
                ));
            }
        }

        // Build conary using the sysroot's cargo
        let status = std::process::Command::new(&cargo)
            .args(["build", "--release"])
            .current_dir(&src_dir)
            .status()
            .map_err(|e| ConaryStageError::ConaryBuildFailed(e.to_string()))?;
        if !status.success() {
            return Err(ConaryStageError::ConaryBuildFailed(
                "cargo build failed".into(),
            ));
        }

        // Install binary to sysroot
        let binary = src_dir.join("target/release/conary");
        let dest = self.sysroot.join("usr/bin/conary");
        std::fs::copy(&binary, &dest).map_err(|e| {
            ConaryStageError::ConaryBuildFailed(format!("install failed: {e}"))
        })?;

        info!("[COMPLETE] Conary installed to {}", dest.display());
        Ok(dest)
    }

    /// Run the full Conary stage.
    pub fn build(&self) -> Result<(), ConaryStageError> {
        self.validate_sysroot()?;
        self.build_rust()?;
        self.build_conary()?;
        info!("[COMPLETE] Conary stage: system is now self-hosting");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conary_stage_packages() {
        assert_eq!(ConaryStageBuilder::package_names(), &["rust", "conary"]);
    }

    #[test]
    fn test_conary_stage_validate_missing_sysroot() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let builder = ConaryStageBuilder::new(
            dir.path().to_path_buf(),
            config,
            dir.path().join("nonexistent"),
        );
        assert!(builder.validate_sysroot().is_err());
    }

    #[test]
    fn test_conary_stage_validate_empty_sysroot() {
        let dir = tempfile::tempdir().unwrap();
        let sysroot = dir.path().join("sysroot");
        std::fs::create_dir_all(&sysroot).unwrap();
        let config = BootstrapConfig::new();
        let builder = ConaryStageBuilder::new(dir.path().to_path_buf(), config, sysroot);
        // Missing components
        assert!(builder.validate_sysroot().is_err());
    }

    #[test]
    fn test_rust_bootstrap_url_x86_64() {
        let config = BootstrapConfig::new(); // defaults to x86_64
        let builder =
            ConaryStageBuilder::new(PathBuf::from("/tmp"), config, PathBuf::from("/sysroot"));
        let url = builder.rust_bootstrap_url();
        assert!(url.contains("x86_64-unknown-linux-gnu"));
        assert!(url.contains(RUST_VERSION));
    }

    #[test]
    fn test_rust_bootstrap_url_aarch64() {
        let config = BootstrapConfig::new().with_target(TargetArch::Aarch64);
        let builder =
            ConaryStageBuilder::new(PathBuf::from("/tmp"), config, PathBuf::from("/sysroot"));
        let url = builder.rust_bootstrap_url();
        assert!(url.contains("aarch64-unknown-linux-gnu"));
    }

    #[test]
    fn test_rust_bootstrap_url_riscv64() {
        let config = BootstrapConfig::new().with_target(TargetArch::Riscv64);
        let builder =
            ConaryStageBuilder::new(PathBuf::from("/tmp"), config, PathBuf::from("/sysroot"));
        let url = builder.rust_bootstrap_url();
        assert!(url.contains("riscv64gc-unknown-linux-gnu"));
    }

    #[test]
    fn test_conary_stage_error_display() {
        let err = ConaryStageError::SysrootNotFound(PathBuf::from("/missing"));
        assert!(err.to_string().contains("/missing"));

        let err = ConaryStageError::SysrootIncomplete("usr/bin/gcc".to_string());
        assert!(err.to_string().contains("usr/bin/gcc"));

        let err = ConaryStageError::RustBootstrapFailed("404".to_string());
        assert!(err.to_string().contains("404"));

        let err = ConaryStageError::RustBuildFailed("compile error".to_string());
        assert!(err.to_string().contains("compile error"));

        let err = ConaryStageError::ConaryBuildFailed("link error".to_string());
        assert!(err.to_string().contains("link error"));
    }

    #[test]
    fn test_conary_stage_validate_with_components() {
        let dir = tempfile::tempdir().unwrap();
        let sysroot = dir.path().join("sysroot");

        // Create the required components
        std::fs::create_dir_all(sysroot.join("usr/bin")).unwrap();
        std::fs::create_dir_all(sysroot.join("usr/lib")).unwrap();
        std::fs::write(sysroot.join("usr/bin/gcc"), "").unwrap();
        std::fs::write(sysroot.join("usr/bin/make"), "").unwrap();
        std::fs::write(sysroot.join("usr/lib/libc.so"), "").unwrap();

        let config = BootstrapConfig::new();
        let builder = ConaryStageBuilder::new(dir.path().to_path_buf(), config, sysroot);
        assert!(builder.validate_sysroot().is_ok());
    }

    #[test]
    fn test_build_conary_fails_without_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let sysroot = dir.path().join("sysroot");
        std::fs::create_dir_all(sysroot.join("usr/bin")).unwrap();

        let config = BootstrapConfig::new();
        let builder = ConaryStageBuilder::new(dir.path().to_path_buf(), config, sysroot);
        let err = builder.build_conary().unwrap_err();
        assert!(
            matches!(err, ConaryStageError::ConaryBuildFailed(_)),
            "Expected ConaryBuildFailed error, got: {err}"
        );
        assert!(err.to_string().contains("cargo not found"));
    }
}
