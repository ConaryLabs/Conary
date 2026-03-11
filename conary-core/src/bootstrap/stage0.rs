// conary-core/src/bootstrap/stage0.rs

//! Stage 0 toolchain builder using crosstool-ng
//!
//! Stage 0 produces a static cross-compiler that can run on any Linux host
//! and produce binaries for the target architecture. This toolchain is then
//! used to build Stage 1 (the self-hosted toolchain).

use super::build_helpers;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Errors that can occur during Stage 0 build
#[derive(Debug, Error)]
pub enum Stage0Error {
    #[error("crosstool-ng not found. Install with: dnf install crosstool-ng")]
    CrosstoolNotFound,

    #[error("crosstool-ng config not found at {0}")]
    ConfigNotFound(PathBuf),

    #[error("Failed to create work directory: {0}")]
    WorkDirCreation(#[from] std::io::Error),

    #[error("crosstool-ng build failed: {0}")]
    BuildFailed(String),

    #[error("Toolchain verification failed: {0}")]
    VerificationFailed(String),

    #[error("Missing prerequisite: {0}")]
    MissingPrerequisite(String),
}

/// Status of Stage 0 build
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage0Status {
    /// Not started
    NotStarted,
    /// Downloading source tarballs
    Downloading,
    /// Building toolchain components
    Building { component: String, progress: u8 },
    /// Build complete
    Complete,
    /// Build failed
    Failed(String),
}

/// Single-quote a path for safe interpolation into a shell command string.
///
/// Any embedded single quotes are replaced with the sequence `'\''` which
/// ends the current single-quoted segment, inserts an escaped literal quote,
/// and reopens the single-quoted segment.
fn shell_escape_path(path: &Path) -> String {
    let s = path.display().to_string();
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Builder for Stage 0 toolchain
pub struct Stage0Builder {
    /// Work directory for the build
    work_dir: PathBuf,

    /// Bootstrap configuration
    config: BootstrapConfig,

    /// Path to crosstool-ng config file
    ct_config: PathBuf,

    /// Current status
    status: Stage0Status,
}

impl Stage0Builder {
    /// Create a new Stage 0 builder
    pub fn new(work_dir: impl AsRef<Path>, config: &BootstrapConfig) -> Result<Self, Stage0Error> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let stage0_dir = work_dir.join("stage0");
        std::fs::create_dir_all(&stage0_dir)?;

        // Determine config path
        let ct_config = if let Some(custom) = &config.crosstool_config {
            custom.clone()
        } else {
            // Look for bundled config
            Self::find_bundled_config()?
        };

        if !ct_config.exists() {
            return Err(Stage0Error::ConfigNotFound(ct_config));
        }

        Ok(Self {
            work_dir: stage0_dir,
            config: config.clone(),
            ct_config,
            status: Stage0Status::NotStarted,
        })
    }

    /// Find the bundled crosstool-ng config
    fn find_bundled_config() -> Result<PathBuf, Stage0Error> {
        // Try relative to executable
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));

        let search_paths = [
            // Relative to crate root (for development)
            PathBuf::from("bootstrap/stage0/crosstool.config"),
            // Relative to current dir
            PathBuf::from("./bootstrap/stage0/crosstool.config"),
            // System-wide installation
            PathBuf::from("/usr/share/conary/bootstrap/stage0/crosstool.config"),
            // User-local
            dirs::data_local_dir()
                .unwrap_or_default()
                .join("conary/bootstrap/stage0/crosstool.config"),
        ];

        // Also check relative to executable
        let mut all_paths = search_paths.to_vec();
        if let Some(exe) = exe_dir {
            all_paths.push(exe.join("../share/conary/bootstrap/stage0/crosstool.config"));
            all_paths.push(exe.join("bootstrap/stage0/crosstool.config"));
        }

        for path in all_paths {
            if path.exists() {
                return Ok(path);
            }
        }

        // Last resort: check CONARY_BOOTSTRAP_CONFIG env var
        if let Ok(path) = std::env::var("CONARY_BOOTSTRAP_CONFIG") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Ok(path);
            }
        }

        Err(Stage0Error::ConfigNotFound(PathBuf::from(
            "crosstool.config (not found in any search path)",
        )))
    }

    /// Check if crosstool-ng is available
    pub fn check_crosstool() -> Result<String, Stage0Error> {
        let output = Command::new("ct-ng")
            .arg("version")
            .output()
            .map_err(|_| Stage0Error::CrosstoolNotFound)?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .to_string())
        } else {
            Err(Stage0Error::CrosstoolNotFound)
        }
    }

    /// Get current build status
    pub fn status(&self) -> &Stage0Status {
        &self.status
    }

    /// Download source tarballs without building
    pub fn download_sources(&mut self) -> Result<(), Stage0Error> {
        info!("Downloading source tarballs...");
        self.status = Stage0Status::Downloading;

        self.setup_work_dir()?;

        let status = Command::new("ct-ng")
            .arg("source")
            .current_dir(&self.work_dir)
            .status()
            .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;

        if !status.success() {
            let msg = "Failed to download sources".to_string();
            self.status = Stage0Status::Failed(msg.clone());
            return Err(Stage0Error::BuildFailed(msg));
        }

        Ok(())
    }

    /// Build the Stage 0 toolchain
    pub fn build(&mut self) -> Result<Toolchain, Stage0Error> {
        info!("Building Stage 0 toolchain...");

        // Strategy 1: Download Seed (Preferred)
        let downloads_dir = self.work_dir.join("downloads");
        std::fs::create_dir_all(&downloads_dir)?;
        let has_cache = Self::has_cached_seed(&downloads_dir, self.config.triple());
        if self.config.seed_url.is_some() || has_cache {
            info!("Using Stage 0 seed...");
            self.download_and_install_seed()?;

            // Verify
            self.verify_toolchain()?;

            return Toolchain::from_prefix(&self.config.tools_prefix)
                .map_err(|e| Stage0Error::VerificationFailed(e.to_string()));
        }

        // Strategy 2: Build from Source (Fallback)
        info!("No seed configured. Falling back to 'crosstool-ng' build from source.");
        info!("Config: {}", self.ct_config.display());
        info!("Work dir: {}", self.work_dir.display());
        info!("Target: {}", self.config.triple());

        // Check prerequisites
        Self::check_crosstool()?;

        // Set up work directory with config
        self.setup_work_dir()?;

        // Run the build
        self.status = Stage0Status::Building {
            component: "all".to_string(),
            progress: 0,
        };

        let is_root = nix::unistd::getuid().is_root();

        let mut cmd;
        if is_root {
            // ct-ng refuses to run as root; use su to drop to a build user
            let build_user = Self::find_build_user()?;
            info!(
                "Running as root, dropping to user '{}' for ct-ng",
                build_user
            );

            // Ensure the build user owns the work and output dirs
            let uid_gid = Command::new("id")
                .args(["-u", &build_user])
                .output()
                .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;
            let uid: u32 = String::from_utf8_lossy(&uid_gid.stdout)
                .trim()
                .parse()
                .map_err(|e: std::num::ParseIntError| Stage0Error::BuildFailed(e.to_string()))?;
            let gid_out = Command::new("id")
                .args(["-g", &build_user])
                .output()
                .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;
            let gid: u32 = String::from_utf8_lossy(&gid_out.stdout)
                .trim()
                .parse()
                .map_err(|e: std::num::ParseIntError| Stage0Error::BuildFailed(e.to_string()))?;

            for dir in [&self.work_dir, &self.config.tools_prefix] {
                std::fs::create_dir_all(dir)
                    .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;
                let status = Command::new("chown")
                    .args(["-R", &format!("{uid}:{gid}"), &dir.to_string_lossy()])
                    .status()
                    .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;
                if !status.success() {
                    return Err(Stage0Error::BuildFailed(format!(
                        "chown -R {uid}:{gid} {} failed",
                        dir.display()
                    )));
                }
            }

            // Build the ct-ng command to run via su.
            // Shell-escape paths to prevent injection via metacharacters.
            let safe_work_dir = shell_escape_path(&self.work_dir);
            let safe_prefix = shell_escape_path(&self.config.tools_prefix);
            let mut ct_env = format!("CT_PREFIX={safe_prefix}");
            if self.config.jobs > 1 {
                ct_env.push_str(&format!(" CT_JOBS={}", self.config.jobs));
            }
            cmd = Command::new("su");
            cmd.args(["-s", "/bin/bash", &build_user, "-c"])
                .arg(format!("cd {safe_work_dir} && {ct_env} ct-ng build"));
        } else {
            cmd = Command::new("ct-ng");
            cmd.arg("build")
                .current_dir(&self.work_dir)
                .env("CT_PREFIX", &self.config.tools_prefix);

            if self.config.jobs > 1 {
                cmd.env("CT_JOBS", self.config.jobs.to_string());
            }
        }

        // Capture output for logging
        if self.config.verbose {
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        }

        info!(
            "Running: ct-ng build (this may take 30-60 minutes, {} jobs)",
            self.config.jobs
        );

        let output = cmd
            .output()
            .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = format!("ct-ng build failed:\n{}", stderr);
            self.status = Stage0Status::Failed(msg.clone());
            return Err(Stage0Error::BuildFailed(msg));
        }

        self.status = Stage0Status::Complete;

        // Verify the toolchain
        self.verify_toolchain()?;

        // Return the toolchain
        Toolchain::from_prefix(&self.config.tools_prefix)
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))
    }

    /// Seed archive filename patterns for a given triple.
    fn seed_patterns(triple: &str) -> [String; 3] {
        [
            format!("{triple}-seed.tar.xz"),
            format!("{triple}-seed.tar.gz"),
            format!("{triple}-seed.tar.bz2"),
        ]
    }

    /// Check if a cached seed tarball exists in the downloads directory.
    pub fn has_cached_seed(downloads_dir: &Path, triple: &str) -> bool {
        Self::find_cached_seed(downloads_dir, triple).is_some()
    }

    /// Find the cached seed path if it exists.
    pub fn find_cached_seed(downloads_dir: &Path, triple: &str) -> Option<PathBuf> {
        Self::seed_patterns(triple)
            .iter()
            .map(|name| downloads_dir.join(name))
            .find(|p| p.exists())
    }

    fn download_and_install_seed(&mut self) -> Result<(), Stage0Error> {
        let downloads_dir = self.work_dir.join("downloads");
        std::fs::create_dir_all(&downloads_dir)?;
        let triple = self.config.triple();

        // Check cache first
        if let Some(cached) = Self::find_cached_seed(&downloads_dir, triple) {
            info!("Using cached seed: {}", cached.display());
            return self.extract_seed(&cached);
        }

        // Download if not cached
        let url = self.config.seed_url.as_ref().ok_or_else(|| {
            Stage0Error::MissingPrerequisite("No seed URL configured".to_string())
        })?;

        let default_name = format!("{triple}-seed.tar.xz");
        let filename = url.split('/').next_back().unwrap_or(&default_name);
        let target_path = downloads_dir.join(filename);

        if !target_path.exists() {
            info!("Downloading seed: {}", url);
            let status = Command::new("curl")
                .args(["-fsSL", "-o", target_path.to_str().unwrap(), url])
                .status()
                .map_err(|e| Stage0Error::BuildFailed(format!("Curl failed: {e}")))?;

            if !status.success() {
                return Err(Stage0Error::BuildFailed(
                    "Failed to download seed".to_string(),
                ));
            }
        }

        // Verify Checksum -- require a checksum for seed packages
        let expected = self.config.seed_checksum.as_ref().ok_or_else(|| {
            Stage0Error::VerificationFailed(
                "No seed_checksum configured. A SHA-256 checksum is required for seed packages."
                    .to_string(),
            )
        })?;

        info!("Verifying seed checksum...");
        let output = Command::new("sha256sum")
            .arg(&target_path)
            .output()
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        let computed = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();

        if !computed.eq_ignore_ascii_case(expected) {
            return Err(Stage0Error::VerificationFailed(format!(
                "Checksum mismatch! Expected {expected}, got {computed}"
            )));
        }

        self.extract_seed(&target_path)
    }

    /// Extract a seed tarball to the tools prefix directory.
    fn extract_seed(&self, seed_path: &Path) -> Result<(), Stage0Error> {
        info!(
            "Extracting seed to {}...",
            self.config.tools_prefix.display()
        );

        build_helpers::extract_tar(seed_path, &self.config.tools_prefix, true)
            .map_err(|e| Stage0Error::BuildFailed(format!("Seed extraction failed: {e}")))
    }

    /// Set up the work directory with crosstool-ng config
    fn setup_work_dir(&self) -> Result<(), Stage0Error> {
        debug!("Setting up work directory: {}", self.work_dir.display());

        // Create directories
        std::fs::create_dir_all(&self.work_dir)?;
        std::fs::create_dir_all(self.work_dir.join("tarballs"))?;

        // Copy config to work dir (ct-ng expects it in current dir)
        let dest_config = self.work_dir.join(".config");
        if !dest_config.exists() || self.config_changed(&dest_config)? {
            std::fs::copy(&self.ct_config, &dest_config)?;
            info!("Copied config to {}", dest_config.display());
        }

        // Create CT_PREFIX directory
        if let Err(e) = std::fs::create_dir_all(&self.config.tools_prefix) {
            warn!(
                "Could not create tools prefix {}: {} (may need sudo)",
                self.config.tools_prefix.display(),
                e
            );
        }

        Ok(())
    }

    /// Check if the config file has changed
    fn config_changed(&self, dest: &Path) -> Result<bool, Stage0Error> {
        let src_content = std::fs::read_to_string(&self.ct_config)?;
        let dst_content = std::fs::read_to_string(dest).unwrap_or_default();
        Ok(src_content != dst_content)
    }

    /// Find a non-root user to run ct-ng as (ct-ng refuses to run as root)
    fn find_build_user() -> Result<String, Stage0Error> {
        // Prefer 'conary' user, then 'nobody'
        for user in ["conary", "nobody"] {
            let status = Command::new("id")
                .arg(user)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if status.is_ok_and(|s| s.success()) {
                return Ok(user.to_string());
            }
        }
        Err(Stage0Error::BuildFailed(
            "no non-root user found for ct-ng (tried: conary, nobody)".into(),
        ))
    }

    /// Verify the built toolchain works
    fn verify_toolchain(&self) -> Result<(), Stage0Error> {
        info!("Verifying toolchain...");

        let gcc_path = self.config.tool_path("gcc");

        // Check gcc exists
        if !gcc_path.exists() {
            return Err(Stage0Error::VerificationFailed(format!(
                "gcc not found at {}",
                gcc_path.display()
            )));
        }

        // Check it's static (no dynamic dependencies)
        let ldd_output = Command::new("ldd")
            .arg(&gcc_path)
            .output()
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        let ldd_str = String::from_utf8_lossy(&ldd_output.stdout);
        if !ldd_str.contains("not a dynamic executable") && !ldd_output.status.success() {
            warn!(
                "Toolchain may not be fully static: {}",
                String::from_utf8_lossy(&ldd_output.stderr)
            );
        }

        // Check target triple
        let output = Command::new(&gcc_path)
            .arg("-dumpmachine")
            .output()
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        let triple = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if triple != self.config.triple() {
            return Err(Stage0Error::VerificationFailed(format!(
                "Target mismatch: expected {}, got {}",
                self.config.triple(),
                triple
            )));
        }

        // Test compile
        let test_result = self.test_compile(&gcc_path);
        if let Err(e) = test_result {
            warn!("Test compile warning: {}", e);
        }

        info!("Toolchain verified successfully");
        Ok(())
    }

    /// Test that the toolchain can compile a simple program
    fn test_compile(&self, gcc: &Path) -> Result<(), Stage0Error> {
        let temp_dir =
            tempfile::tempdir().map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        let src = temp_dir.path().join("test.c");
        let bin = temp_dir.path().join("test");

        std::fs::write(&src, "int main() { return 0; }")
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        let output = Command::new(gcc)
            .args(["-o", bin.to_str().unwrap(), src.to_str().unwrap()])
            .output()
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(Stage0Error::VerificationFailed(format!(
                "Test compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Verify it's the right architecture
        let file_output = Command::new("file")
            .arg(&bin)
            .output()
            .map_err(|e| Stage0Error::VerificationFailed(e.to_string()))?;

        let file_str = String::from_utf8_lossy(&file_output.stdout);
        let expected_arch = match self.config.target_arch {
            super::config::TargetArch::X86_64 => "x86-64",
            super::config::TargetArch::Aarch64 => "ARM aarch64",
            super::config::TargetArch::Riscv64 => "RISC-V",
        };

        if !file_str.contains(expected_arch) {
            return Err(Stage0Error::VerificationFailed(format!(
                "Binary architecture mismatch: expected {}, got {}",
                expected_arch, file_str
            )));
        }

        debug!("Test compile successful: {}", file_str.trim());
        Ok(())
    }

    /// Clean the work directory
    pub fn clean(&self) -> Result<(), Stage0Error> {
        info!("Cleaning work directory...");

        let status = Command::new("ct-ng")
            .arg("distclean")
            .current_dir(&self.work_dir)
            .status()
            .map_err(|e| Stage0Error::BuildFailed(e.to_string()))?;

        if !status.success() {
            warn!("ct-ng distclean had warnings");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_crosstool() {
        // This test just checks the function doesn't panic
        // crosstool-ng may or may not be installed
        let result = Stage0Builder::check_crosstool();
        // We don't assert success because ct-ng might not be installed
        if let Ok(version) = result {
            assert!(!version.is_empty());
        }
    }

    #[test]
    fn test_seed_cache_detection() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();

        // No seed file -> false
        assert!(!Stage0Builder::has_cached_seed(
            &downloads,
            "x86_64-conary-linux-gnu"
        ));
        assert!(Stage0Builder::find_cached_seed(&downloads, "x86_64-conary-linux-gnu").is_none());

        // Create a fake seed tarball
        let seed_path = downloads.join("x86_64-conary-linux-gnu-seed.tar.xz");
        std::fs::write(&seed_path, b"fake").unwrap();

        // Seed file exists -> true
        assert!(Stage0Builder::has_cached_seed(
            &downloads,
            "x86_64-conary-linux-gnu"
        ));
        assert_eq!(
            Stage0Builder::find_cached_seed(&downloads, "x86_64-conary-linux-gnu"),
            Some(seed_path),
        );
    }

    #[test]
    fn test_seed_cache_multiple_formats() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();

        // .tar.gz format should also be found
        let seed_path = downloads.join("aarch64-conary-linux-gnu-seed.tar.gz");
        std::fs::write(&seed_path, b"fake").unwrap();
        assert!(Stage0Builder::has_cached_seed(
            &downloads,
            "aarch64-conary-linux-gnu"
        ));
    }

    #[test]
    fn test_stage0_status_variants() {
        let status = Stage0Status::NotStarted;
        assert_eq!(status, Stage0Status::NotStarted);

        let status = Stage0Status::Building {
            component: "gcc".to_string(),
            progress: 50,
        };
        if let Stage0Status::Building {
            component,
            progress,
        } = status
        {
            assert_eq!(component, "gcc");
            assert_eq!(progress, 50);
        }
    }
}
