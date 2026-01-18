// src/ccs/convert/capture.rs

//! Scriptlet capture engine
//!
//! Executes legacy scriptlets in a sandboxed, mocked environment and captures
//! their side effects (file creations) and declarative intents (service enablement).

use crate::container::{ContainerConfig, Sandbox, BindMount};
use crate::error::{Error, Result};
use crate::packages::traits::ExtractedFile;
use super::mock::{self, CapturedIntent};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

/// Results of a scriptlet capture
#[derive(Debug)]
pub struct CaptureResult {
    /// Files created or modified by the script
    pub new_files: Vec<ExtractedFile>,
    /// High-level intents detected (e.g. "enable service")
    pub intents: Vec<CapturedIntent>,
}

/// Engine for capturing scriptlet side effects
pub struct ScriptletCapturer {
    /// Temp directory for the capture environment
    root: TempDir,
}

impl ScriptletCapturer {
    /// Create a new capturer
    pub fn new() -> Result<Self> {
        Ok(Self {
            root: TempDir::new()?,
        })
    }

    /// Run a scriptlet and capture its effects
    pub fn capture(
        &mut self, 
        script: &str, 
        interpreter: &str,
        initial_files: &[ExtractedFile]
    ) -> Result<CaptureResult> {
        let root_path = self.root.path();
        debug!("Setting up capture environment in {}", root_path.display());

        // 1. Populate initial filesystem
        self.write_files(initial_files)?;

        // 2. Setup mock tools
        mock::setup_mock_tools(root_path)?;

        // 3. Create necessary system directories if missing
        for dir in &["proc", "sys", "dev", "tmp", "run"] {
            fs::create_dir_all(root_path.join(dir))?;
        }

        // 4. Configure Sandbox
        // We use a "permissive" sandbox in terms of mounts (we want to write to root)
        // but strict on network.
        // NOTE: The `Sandbox` usually mounts the host /bin. We need to OVERRIDE that
        // to use our mock /bin tools, but we still need a real shell (`sh`, `bash`).
        
        let mut config = ContainerConfig::default();
        config.isolate_network = true; // No network access
        config.workdir = PathBuf::from("/");
        config.hostname = "conary-capture".to_string();

        // Important: We need a real shell.
        // We bind mount /bin/sh and /bin/bash from host to /bin in sandbox
        // BUT we also put our mock tools in /bin.
        // This suggests we need to be careful. 
        // Strategy: Mount host /bin to /host/bin. Symlink /bin/sh -> /host/bin/sh.
        // Since `ContainerConfig` is strict about mounts, let's use `pristine` and build up.
        
        let mut config = ContainerConfig::pristine();
        config.isolate_network = true;
        
        // Mount the temp dir as root (RW)
        config.add_bind_mount(BindMount::writable(root_path, "/"));

        // We need a shell. 
        // Ideally we'd have a static busybox. For now, we rely on host's /bin/sh.
        // We mount host /bin to /usr/bin to avoid clashing with our mock /bin?
        // Let's bind mount host /bin/sh to /bin/sh explicitly if possible, 
        // or mount host /bin to /host-bin and symlink.
        
        // Simpler approach: 
        // 1. Copy host /bin/sh and dependencies to root_path/bin/sh? Too complex (libs).
        // 2. Mount host /usr (RO) so we have libs.
        // 3. Mount host /bin (RO) to /usr/bin (or wherever).
        // 4. Overlay our mock tools?
        
        // Let's try mounting host /usr and /lib (for shell deps)
        config.add_bind_mount(BindMount::readonly("/usr", "/usr"));
        config.add_bind_mount(BindMount::readonly("/lib", "/lib"));
        if Path::new("/lib64").exists() {
            config.add_bind_mount(BindMount::readonly("/lib64", "/lib64"));
        }
        
        // The sandbox setup usually mounts /bin. 
        // Our mock tools are in `root_path/bin`.
        // If we bind mount host /bin to /bin, we hide our mocks.
        // We need our mocks to take precedence.
        
        // Correct approach:
        // 1. Mount host /bin to /host-bin
        // 2. Symlink /bin/sh -> /host-bin/sh in our `root_path`
        // 3. `setup_mock_tools` puts fake tools in `root_path/bin`
        // 4. Sandbox treats `root_path` as `/`.
        
        config.add_bind_mount(BindMount::readonly("/bin", "/host-bin"));
        
        // Fixup shell symlinks in root_path
        let bin_dir = root_path.join("bin");
        fs::create_dir_all(&bin_dir)?;
        
        // Force sh/bash to point to host binaries
        if bin_dir.join("sh").exists() { fs::remove_file(bin_dir.join("sh"))?; }
        if bin_dir.join("bash").exists() { fs::remove_file(bin_dir.join("bash"))?; }
        
        std::os::unix::fs::symlink("/host-bin/sh", bin_dir.join("sh"))?;
        std::os::unix::fs::symlink("/host-bin/bash", bin_dir.join("bash"))?;

        // 5. Execute
        let mut sandbox = Sandbox::new(config);
        
        info!("Running scriptlet in capture mode...");
        let (code, stdout, stderr) = sandbox.execute(
            interpreter,
            script,
            &[],
            &[]
        )?;

        if code != 0 {
            warn!("Scriptlet failed with code {}: {}", code, stderr);
            // We might still want to capture what it *did* do, or fail.
            // For now, let's capture partial results but warn.
        }

        // 6. Diff filesystem
        let new_files = self.scan_for_changes(initial_files)?;
        
        // 7. Parse intents
        let intents = mock::parse_capture_log(root_path)?;

        Ok(CaptureResult {
            new_files,
            intents,
        })
    }

    fn write_files(&self, files: &[ExtractedFile]) -> Result<()> {
        let root = self.root.path();
        for file in files {
            let rel_path = file.path.strip_prefix('/').unwrap_or(&file.path);
            let full_path = root.join(rel_path);
            
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            
            fs::write(&full_path, &file.content)?;
            
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&full_path)?.permissions();
                perms.set_mode(file.mode as u32);
                fs::set_permissions(&full_path, perms)?;
            }
        }
        Ok(())
    }

    fn scan_for_changes(&self, initial_files: &[ExtractedFile]) -> Result<Vec<ExtractedFile>> {
        let root = self.root.path();
        let initial_paths: HashMap<String, _> = initial_files.iter()
            .map(|f| (f.path.clone(), f))
            .collect();
            
        let mut new_files = Vec::new();

        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() { continue; }
            
            let path = entry.path();
            let rel_path = path.strip_prefix(root).unwrap();
            let abs_path = format!("/{}", rel_path.to_string_lossy());

            // Skip logs and temp files
            if abs_path.starts_with("/var/log/conary") || abs_path.starts_with("/tmp") {
                continue;
            }
            
            // Skip initial files (unless modified? For now assume immutability of package payload)
            if initial_paths.contains_key(&abs_path) {
                // TODO: Check hash to see if modified
                continue;
            }

            // It's a new file
            let content = fs::read(path)?;
            let metadata = fs::metadata(path)?;
            
            // Calculate SHA256 (simplified for brevity, should use hash util)
            // let sha = ...
            
            new_files.push(ExtractedFile {
                path: abs_path,
                content,
                size: metadata.len() as i64,
                mode: metadata.permissions().mode() as i32,
                sha256: None, // Recalculate later
            });
        }
        
        Ok(new_files)
    }
}
