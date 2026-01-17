// src/recipe/kitchen.rs

//! Kitchen: the isolated build environment for cooking recipes
//!
//! The Kitchen provides a sandboxed environment for building packages
//! from source recipes. It handles:
//! - Fetching source archives and patches
//! - Extracting and patching sources
//! - Running build commands in isolation
//! - Packaging the result as CCS

use crate::ccs::builder::{write_ccs_package, CcsBuilder};
use crate::ccs::manifest::{CcsManifest, PackageDep};
use crate::container::{BindMount, ContainerConfig, Sandbox};
use crate::error::{Error, Result};
use crate::hash::{hash_bytes, HashAlgorithm};
use crate::recipe::format::Recipe;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Configuration for the Kitchen
#[derive(Debug, Clone)]
pub struct KitchenConfig {
    /// Directory for downloaded sources
    pub source_cache: PathBuf,
    /// Timeout for build operations
    pub timeout: Duration,
    /// Number of parallel jobs
    pub jobs: u32,
    /// Enable network access during build (not recommended)
    pub allow_network: bool,
    /// Keep build directory after completion (for debugging)
    pub keep_builddir: bool,
    /// Enable container isolation for builds (requires root or user namespaces)
    pub use_isolation: bool,
    /// Memory limit for isolated builds (bytes, 0 = no limit)
    pub memory_limit: u64,
    /// CPU time limit for isolated builds (seconds, 0 = no limit)
    pub cpu_time_limit: u64,
}

impl Default for KitchenConfig {
    fn default() -> Self {
        let jobs = std::thread::available_parallelism()
            .map(|p| p.get() as u32)
            .unwrap_or(4);

        Self {
            source_cache: PathBuf::from("/var/cache/conary/sources"),
            timeout: Duration::from_secs(3600), // 1 hour
            jobs,
            allow_network: false,
            keep_builddir: false,
            use_isolation: false, // Off by default, requires root or user namespaces
            memory_limit: 4 * 1024 * 1024 * 1024, // 4 GB for builds
            cpu_time_limit: 0, // No CPU time limit (builds can be long)
        }
    }
}

/// Result of cooking a recipe
#[derive(Debug)]
pub struct CookResult {
    /// Path to the built CCS package
    pub package_path: PathBuf,
    /// Build log
    pub log: String,
    /// Warnings generated during build
    pub warnings: Vec<String>,
}

/// The Kitchen: where recipes are cooked
pub struct Kitchen {
    config: KitchenConfig,
}

impl Kitchen {
    /// Create a new Kitchen with the given configuration
    pub fn new(config: KitchenConfig) -> Self {
        Self { config }
    }

    /// Create a Kitchen with default configuration
    pub fn with_defaults() -> Self {
        Self::new(KitchenConfig::default())
    }

    /// Cook a recipe and produce a CCS package
    ///
    /// This is the main entry point for building from source.
    pub fn cook(&self, recipe: &Recipe, output_dir: &Path) -> Result<CookResult> {
        info!(
            "Cooking {} version {}",
            recipe.package.name, recipe.package.version
        );

        let mut cook = Cook::new(self, recipe)?;

        // Phase 1: Prep - fetch ingredients
        info!("Prep: fetching ingredients...");
        cook.prep()?;

        // Phase 2: Unpack and patch
        info!("Unpacking and patching sources...");
        cook.unpack()?;
        cook.patch()?;

        // Phase 3: Simmer - run the build
        info!("Simmering: running build...");
        cook.simmer()?;

        // Phase 4: Plate - package the result
        info!("Plating: creating CCS package...");
        let package_path = cook.plate(output_dir)?;

        Ok(CookResult {
            package_path,
            log: cook.log,
            warnings: cook.warnings,
        })
    }

    /// Fetch a source archive (with caching)
    fn fetch_source(&self, url: &str, checksum: &str) -> Result<PathBuf> {
        // Create cache directory if needed
        fs::create_dir_all(&self.config.source_cache)?;

        // Use checksum as cache key
        let cache_key = checksum.replace(':', "_");
        let cached_path = self.config.source_cache.join(&cache_key);

        // Check if already cached
        if cached_path.exists() {
            debug!("Using cached source: {}", cached_path.display());
            // Verify checksum
            if verify_file_checksum(&cached_path, checksum)? {
                return Ok(cached_path);
            }
            warn!("Cached file checksum mismatch, re-downloading");
            fs::remove_file(&cached_path)?;
        }

        // Download the source
        info!("Downloading: {}", url);
        let temp_path = self.config.source_cache.join(format!("{}.tmp", cache_key));

        download_file(url, &temp_path)?;

        // Verify checksum
        if !verify_file_checksum(&temp_path, checksum)? {
            fs::remove_file(&temp_path)?;
            return Err(Error::ChecksumMismatch {
                expected: checksum.to_string(),
                actual: "mismatch".to_string(),
            });
        }

        // Move to final location
        fs::rename(&temp_path, &cached_path)?;
        Ok(cached_path)
    }
}

/// A single cook operation
pub struct Cook<'a> {
    kitchen: &'a Kitchen,
    recipe: &'a Recipe,
    /// Temporary build directory
    build_dir: TempDir,
    /// Source directory within build_dir
    source_dir: PathBuf,
    /// Destination directory (where files get installed)
    dest_dir: PathBuf,
    /// Build log accumulator
    log: String,
    /// Warnings
    warnings: Vec<String>,
}

impl<'a> Cook<'a> {
    fn new(kitchen: &'a Kitchen, recipe: &'a Recipe) -> Result<Self> {
        let build_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create build directory: {}", e)))?;

        let source_dir = build_dir.path().join("source");
        let dest_dir = build_dir.path().join("destdir");

        fs::create_dir_all(&source_dir)?;
        fs::create_dir_all(&dest_dir)?;

        Ok(Self {
            kitchen,
            recipe,
            build_dir,
            source_dir,
            dest_dir,
            log: String::new(),
            warnings: Vec::new(),
        })
    }

    /// Phase 1: Prep - fetch all sources
    fn prep(&mut self) -> Result<()> {
        // Fetch main source archive
        let archive_url = self.recipe.archive_url();
        let archive_path = self.kitchen.fetch_source(&archive_url, &self.recipe.source.checksum)?;

        // Copy to build directory
        let local_archive = self.build_dir.path().join(self.recipe.archive_filename());
        fs::copy(&archive_path, &local_archive)?;

        self.log_line(&format!("Fetched source: {}", archive_url));

        // Fetch additional sources
        for additional in &self.recipe.source.additional {
            let path = self.kitchen.fetch_source(&additional.url, &additional.checksum)?;
            let filename = additional
                .url
                .split('/')
                .last()
                .unwrap_or("additional.tar.gz");
            let local_path = self.build_dir.path().join(filename);
            fs::copy(&path, &local_path)?;
            self.log_line(&format!("Fetched additional source: {}", additional.url));
        }

        // Fetch patches
        if let Some(patches) = &self.recipe.patches {
            for patch in &patches.files {
                if patch.file.starts_with("http://") || patch.file.starts_with("https://") {
                    let checksum = patch.checksum.as_deref().unwrap_or("sha256:0");
                    let path = self.kitchen.fetch_source(&patch.file, checksum)?;
                    let filename = patch.file.split('/').last().unwrap_or("patch.diff");
                    let local_path = self.build_dir.path().join("patches").join(filename);
                    fs::create_dir_all(local_path.parent().unwrap())?;
                    fs::copy(&path, &local_path)?;
                    self.log_line(&format!("Fetched patch: {}", patch.file));
                }
            }
        }

        Ok(())
    }

    /// Phase 2a: Unpack sources
    fn unpack(&mut self) -> Result<()> {
        let archive_path = self.build_dir.path().join(self.recipe.archive_filename());

        // Detect archive type and extract
        extract_archive(&archive_path, &self.source_dir)?;
        self.log_line(&format!(
            "Extracted source to {}",
            self.source_dir.display()
        ));

        // Find the actual source directory (often archives have a top-level dir)
        let entries: Vec<_> = fs::read_dir(&self.source_dir)?
            .filter_map(|e| e.ok())
            .collect();

        if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
            // Single directory - this is the actual source
            self.source_dir = entries[0].path();
            debug!("Source directory: {}", self.source_dir.display());
        }

        // Override with explicit extract_dir if specified
        if let Some(extract_dir) = &self.recipe.source.extract_dir {
            self.source_dir = self.build_dir.path().join("source").join(extract_dir);
        }

        Ok(())
    }

    /// Phase 2b: Apply patches
    fn patch(&mut self) -> Result<()> {
        let patches = match &self.recipe.patches {
            Some(p) => &p.files,
            None => return Ok(()),
        };

        for patch_info in patches {
            let patch_path = if patch_info.file.starts_with("http://")
                || patch_info.file.starts_with("https://")
            {
                let filename = patch_info.file.split('/').last().unwrap_or("patch.diff");
                self.build_dir.path().join("patches").join(filename)
            } else {
                PathBuf::from(&patch_info.file)
            };

            if !patch_path.exists() {
                return Err(Error::NotFound(format!(
                    "Patch file not found: {}",
                    patch_path.display()
                )));
            }

            info!("Applying patch: {}", patch_info.file);
            apply_patch(&self.source_dir, &patch_path, patch_info.strip)?;
            self.log_line(&format!("Applied patch: {}", patch_info.file));
        }

        Ok(())
    }

    /// Phase 3: Simmer - run the build
    fn simmer(&mut self) -> Result<()> {
        let build = &self.recipe.build;

        // Determine working directory
        let workdir = if let Some(wd) = &build.workdir {
            self.source_dir.join(wd)
        } else {
            self.source_dir.clone()
        };

        // Set up environment
        let mut env: Vec<(&str, String)> = vec![
            ("DESTDIR", self.dest_dir.to_string_lossy().to_string()),
            (
                "MAKEFLAGS",
                format!("-j{}", build.jobs.unwrap_or(self.kitchen.config.jobs)),
            ),
        ];

        for (key, value) in &build.environment {
            env.push((key, value.clone()));
        }

        // Run setup if specified
        if let Some(setup) = &build.setup {
            self.run_build_step("setup", setup, &workdir, &env)?;
        }

        // Run configure
        if let Some(configure) = &build.configure {
            let cmd = self.recipe.substitute(configure, &self.dest_dir.to_string_lossy());
            self.run_build_step("configure", &cmd, &workdir, &env)?;
        }

        // Run make
        if let Some(make) = &build.make {
            let cmd = self.recipe.substitute(make, &self.dest_dir.to_string_lossy());
            self.run_build_step("make", &cmd, &workdir, &env)?;
        }

        // Run check if specified
        if let Some(check) = &build.check {
            match self.run_build_step("check", check, &workdir, &env) {
                Ok(_) => {}
                Err(e) => {
                    self.warnings.push(format!("Tests failed: {}", e));
                }
            }
        }

        // Run install
        if let Some(install) = &build.install {
            let cmd = self.recipe.substitute(install, &self.dest_dir.to_string_lossy());
            self.run_build_step("install", &cmd, &workdir, &env)?;
        }

        // Run post_install if specified
        if let Some(post_install) = &build.post_install {
            self.run_build_step("post_install", post_install, &workdir, &env)?;
        }

        Ok(())
    }

    /// Run a build step
    fn run_build_step(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(&str, String)],
    ) -> Result<()> {
        info!("Running {} phase", phase);
        debug!("Command: {}", command);

        if self.kitchen.config.use_isolation {
            self.run_build_step_isolated(phase, command, workdir, env)
        } else {
            self.run_build_step_direct(phase, command, workdir, env)
        }
    }

    /// Run a build step with container isolation
    fn run_build_step_isolated(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(&str, String)],
    ) -> Result<()> {
        // Configure container with build-appropriate settings
        let mut container_config = ContainerConfig::default();

        // Set resource limits from kitchen config
        container_config.memory_limit = self.kitchen.config.memory_limit;
        container_config.cpu_time_limit = self.kitchen.config.cpu_time_limit;
        container_config.timeout = self.kitchen.config.timeout;
        container_config.hostname = "conary-build".to_string();
        container_config.workdir = workdir.to_path_buf();

        // Add build-specific bind mounts
        container_config.bind_mounts.clear();

        // Essential system directories (read-only)
        for path in &["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
            if Path::new(path).exists() {
                container_config.bind_mounts.push(BindMount::readonly(*path, *path));
            }
        }

        // Config files that build tools might need
        for path in &["/etc/passwd", "/etc/group", "/etc/hosts", "/etc/resolv.conf"] {
            if Path::new(path).exists() {
                container_config.bind_mounts.push(BindMount::readonly(*path, *path));
            }
        }

        // Source directory (read-only - we shouldn't modify sources)
        container_config.bind_mounts.push(
            BindMount::readonly(&self.source_dir, &self.source_dir)
        );

        // Destination directory (writable - where install goes)
        container_config.bind_mounts.push(
            BindMount::writable(&self.dest_dir, &self.dest_dir)
        );

        // Build directory (writable - for build artifacts)
        container_config.bind_mounts.push(
            BindMount::writable(self.build_dir.path(), self.build_dir.path())
        );

        let mut sandbox = Sandbox::new(container_config);

        // Convert env to the format expected by Sandbox
        let env_refs: Vec<(&str, &str)> = env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        let (exit_code, stdout, stderr) = sandbox.execute(
            "/bin/sh",
            &format!("cd {} && {}", workdir.display(), command),
            &[],
            &env_refs,
        )?;

        self.log_line(&format!("=== {} (isolated) ===", phase));
        if !stdout.is_empty() {
            self.log.push_str(&stdout);
            self.log.push('\n');
        }
        if !stderr.is_empty() {
            self.log.push_str(&stderr);
            self.log.push('\n');
        }

        if exit_code != 0 {
            return Err(Error::IoError(format!(
                "{} phase failed with exit code {}\nstderr: {}",
                phase, exit_code, stderr
            )));
        }

        Ok(())
    }

    /// Run a build step directly (no isolation)
    fn run_build_step_direct(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(&str, String)],
    ) -> Result<()> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(workdir)
            .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
            .output()
            .map_err(|e| Error::IoError(format!("Failed to run {} phase: {}", phase, e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        self.log_line(&format!("=== {} ===", phase));
        if !stdout.is_empty() {
            self.log.push_str(&stdout);
            self.log.push('\n');
        }
        if !stderr.is_empty() {
            self.log.push_str(&stderr);
            self.log.push('\n');
        }

        if !output.status.success() {
            return Err(Error::IoError(format!(
                "{} phase failed with exit code {:?}\nstderr: {}",
                phase,
                output.status.code(),
                stderr
            )));
        }

        Ok(())
    }

    /// Phase 4: Plate - package the result as CCS
    fn plate(&mut self, output_dir: &Path) -> Result<PathBuf> {
        // Check that destdir has files
        if fs::read_dir(&self.dest_dir)?.count() == 0 {
            return Err(Error::IoError(
                "No files installed to destdir - install phase may have failed".to_string(),
            ));
        }

        // Create CCS manifest from recipe metadata
        let mut manifest = CcsManifest::new_minimal(
            &self.recipe.package.name,
            &self.recipe.package.version,
        );

        // Copy over additional metadata from recipe
        if let Some(desc) = &self.recipe.package.description {
            manifest.package.description = desc.clone();
        } else if let Some(summary) = &self.recipe.package.summary {
            manifest.package.description = summary.clone();
        }
        manifest.package.license = self.recipe.package.license.clone();
        manifest.package.homepage = self.recipe.package.homepage.clone();

        // Add build dependencies as requires (for reference)
        for dep in &self.recipe.build.requires {
            manifest.requires.packages.push(PackageDep {
                name: dep.clone(),
                version: None,
            });
        }

        // Build CCS package from destdir
        let builder = CcsBuilder::new(manifest, &self.dest_dir);
        let build_result = builder
            .build()
            .map_err(|e| Error::IoError(format!("CCS build failed: {e}")))?;

        // Write CCS package
        let package_name = format!(
            "{}-{}-{}.ccs",
            self.recipe.package.name, self.recipe.package.version, self.recipe.package.release
        );
        let package_path = output_dir.join(&package_name);

        write_ccs_package(&build_result, &package_path)
            .map_err(|e| Error::IoError(format!("Failed to write CCS package: {e}")))?;

        self.log_line(&format!(
            "Created CCS package: {} ({} files, {} blobs)",
            package_path.display(),
            build_result.files.len(),
            build_result.blobs.len()
        ));
        info!(
            "Cooked: {} ({} files)",
            package_path.display(),
            build_result.files.len()
        );

        Ok(package_path)
    }

    fn log_line(&mut self, line: &str) {
        self.log.push_str(line);
        self.log.push('\n');
    }
}

/// Download a file from a URL
fn download_file(url: &str, dest: &Path) -> Result<()> {
    // Use curl for now (could use reqwest later)
    let output = Command::new("curl")
        .args(["-fsSL", "-o", dest.to_str().unwrap(), url])
        .output()
        .map_err(|e| Error::DownloadError(format!("curl failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::DownloadError(format!(
            "Failed to download {}: {}",
            url,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Verify file checksum
fn verify_file_checksum(path: &Path, expected: &str) -> Result<bool> {
    let content = fs::read(path)?;

    let (algorithm, expected_hash) = expected
        .split_once(':')
        .ok_or_else(|| Error::ParseError("Invalid checksum format".to_string()))?;

    let algo = match algorithm {
        "sha256" => HashAlgorithm::Sha256,
        "xxh128" => HashAlgorithm::Xxh128,
        _ => {
            return Err(Error::ParseError(format!(
                "Unsupported checksum algorithm: {} (supported: sha256, xxh128)",
                algorithm
            )))
        }
    };

    let actual = hash_bytes(algo, &content);
    Ok(actual.as_str() == expected_hash)
}

/// Extract an archive
fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let filename = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let args: Vec<&str> = if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        vec!["-xzf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
        vec!["-xJf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
        vec!["-xjf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar.zst") {
        vec!["--zstd", "-xf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar") {
        vec!["-xf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else {
        return Err(Error::ParseError(format!(
            "Unknown archive format: {}",
            filename
        )));
    };

    let output = Command::new("tar")
        .args(&args)
        .output()
        .map_err(|e| Error::IoError(format!("tar failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "Failed to extract archive: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Apply a patch to the source
fn apply_patch(source_dir: &Path, patch_path: &Path, strip: u32) -> Result<()> {
    let output = Command::new("patch")
        .args(["-p", &strip.to_string(), "-i", patch_path.to_str().unwrap()])
        .current_dir(source_dir)
        .output()
        .map_err(|e| Error::IoError(format!("patch failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "Failed to apply patch: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kitchen_config_default() {
        let config = KitchenConfig::default();
        assert!(config.jobs > 0);
        assert!(!config.allow_network);
        assert!(!config.keep_builddir);
    }

    #[test]
    fn test_verify_checksum_format() {
        // This test would need actual file content
        // Just testing the format parsing
        let result = verify_file_checksum(Path::new("/nonexistent"), "invalid");
        assert!(result.is_err());
    }
}
