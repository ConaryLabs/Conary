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

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Rust version to bootstrap with
const RUST_VERSION: &str = "1.93.0";

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

    /// Build Rust from source.
    ///
    /// Currently a stub -- returns an error to prevent the bootstrap pipeline
    /// from silently "succeeding" with an empty output directory.
    pub fn build_rust(&self) -> Result<PathBuf, ConaryStageError> {
        Err(ConaryStageError::NotImplemented(
            "build_rust() is a stub -- Rust cross-compilation build logic is not yet implemented"
                .to_string(),
        ))
    }

    /// Build Conary from source.
    ///
    /// Currently a stub -- returns an error to prevent the bootstrap pipeline
    /// from silently "succeeding" with an empty output directory.
    pub fn build_conary(&self) -> Result<PathBuf, ConaryStageError> {
        Err(ConaryStageError::NotImplemented(
            "build_conary() is a stub -- Conary cross-compilation build logic is not yet implemented"
                .to_string(),
        ))
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
    fn test_conary_stage_build_returns_not_implemented() {
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
        let err = builder.build().unwrap_err();
        assert!(
            matches!(err, ConaryStageError::NotImplemented(_)),
            "Expected NotImplemented error, got: {err}"
        );
    }
}
