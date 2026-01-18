// src/recipe/kitchen/cook.rs

//! Cook: the actual build execution for a single recipe

use crate::ccs::builder::{write_ccs_package, CcsBuilder};
use crate::ccs::manifest::{CcsManifest, PackageDep};
use crate::container::{BindMount, ContainerConfig, Sandbox};
use crate::error::{Error, Result};
use crate::recipe::format::Recipe;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::{debug, info};

use super::archive::{apply_patch, extract_archive};
use super::Kitchen;

/// A single cook operation
pub struct Cook<'a> {
    pub(super) kitchen: &'a Kitchen,
    pub(super) recipe: &'a Recipe,
    /// Temporary build directory
    pub(super) build_dir: TempDir,
    /// Source directory within build_dir
    pub(super) source_dir: PathBuf,
    /// Destination directory (where files get installed)
    pub(super) dest_dir: PathBuf,
    /// Build log accumulator
    pub(super) log: String,
    /// Warnings
    pub(super) warnings: Vec<String>,
}

impl<'a> Cook<'a> {
    pub(super) fn new(kitchen: &'a Kitchen, recipe: &'a Recipe) -> Result<Self> {
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
    pub(super) fn prep(&mut self) -> Result<()> {
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
    pub(super) fn unpack(&mut self) -> Result<()> {
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
    pub(super) fn patch(&mut self) -> Result<()> {
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
    pub(super) fn simmer(&mut self) -> Result<()> {
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
        // Configure container based on pristine mode
        let mut container_config = if self.kitchen.config.pristine_mode {
            // Pristine mode: no host system mounts
            // This is critical for bootstrap builds to avoid toolchain contamination
            let config = if let Some(sysroot) = &self.kitchen.config.sysroot {
                ContainerConfig::pristine_for_bootstrap(
                    sysroot,
                    &self.source_dir,
                    self.build_dir.path(),
                    &self.dest_dir,
                )
            } else {
                ContainerConfig::pristine()
            };
            info!(
                "Using pristine container (no host mounts) for {} phase",
                phase
            );
            config
        } else {
            // Normal mode: mount host system directories
            ContainerConfig::default()
        };

        // Set resource limits from kitchen config
        container_config.memory_limit = self.kitchen.config.memory_limit;
        container_config.cpu_time_limit = self.kitchen.config.cpu_time_limit;
        container_config.timeout = self.kitchen.config.timeout;
        container_config.hostname = "conary-build".to_string();
        container_config.workdir = workdir.to_path_buf();

        // Network isolation is on by default - only allow if explicitly configured
        if self.kitchen.config.allow_network {
            container_config.allow_network();
        }

        // For non-pristine mode, set up bind mounts manually
        if !self.kitchen.config.pristine_mode {
            // Clear default mounts and add build-specific ones
            container_config.bind_mounts.clear();

            // Essential system directories (read-only)
            for path in &["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
                if Path::new(path).exists() {
                    container_config
                        .bind_mounts
                        .push(BindMount::readonly(*path, *path));
                }
            }

            // Config files that build tools might need (no resolv.conf - network is isolated)
            for path in &["/etc/passwd", "/etc/group", "/etc/hosts"] {
                if Path::new(path).exists() {
                    container_config
                        .bind_mounts
                        .push(BindMount::readonly(*path, *path));
                }
            }

            // Only mount resolv.conf if network is allowed
            if self.kitchen.config.allow_network && Path::new("/etc/resolv.conf").exists() {
                container_config
                    .bind_mounts
                    .push(BindMount::readonly("/etc/resolv.conf", "/etc/resolv.conf"));
            }

            // Source directory (read-only - we shouldn't modify sources)
            container_config
                .bind_mounts
                .push(BindMount::readonly(&self.source_dir, &self.source_dir));

            // Destination directory (writable - where install goes)
            container_config
                .bind_mounts
                .push(BindMount::writable(&self.dest_dir, &self.dest_dir));

            // Build directory (writable - for build artifacts)
            container_config
                .bind_mounts
                .push(BindMount::writable(self.build_dir.path(), self.build_dir.path()));
        }

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

        self.log_build_output(phase, true, &stdout, &stderr);

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

        self.log_build_output(phase, false, &stdout, &stderr);

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
    pub(super) fn plate(&mut self, output_dir: &Path) -> Result<PathBuf> {
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

    /// Log build step output (stdout/stderr) with a phase header
    fn log_build_output(&mut self, phase: &str, isolated: bool, stdout: &str, stderr: &str) {
        let header = if isolated {
            format!("=== {} (isolated) ===", phase)
        } else {
            format!("=== {} ===", phase)
        };
        self.log_line(&header);
        if !stdout.is_empty() {
            self.log.push_str(stdout);
            self.log.push('\n');
        }
        if !stderr.is_empty() {
            self.log.push_str(stderr);
            self.log.push('\n');
        }
    }
}
