// src/derived/builder.rs

//! Derived package builder implementation
//!
//! Takes a parent package and applies modifications to create a derived version.

use crate::db::models::{
    DerivedOverride, DerivedPackage, DerivedPatch, FileEntry, Trove, VersionPolicy,
};
use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use crate::hash;
use rusqlite::Connection;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Specification for building a derived package
#[derive(Debug, Clone)]
pub struct DerivedSpec {
    /// Name of the derived package
    pub name: String,
    /// Name of the parent package
    pub parent_name: String,
    /// Parent version constraint (None = latest installed)
    pub parent_version: Option<String>,
    /// Version policy
    pub version_policy: VersionPolicy,
    /// Description
    pub description: Option<String>,
    /// Patches to apply (name, content bytes)
    pub patches: Vec<(String, Vec<u8>)>,
    /// File overrides (target path, new content bytes, optional permissions)
    pub overrides: Vec<(String, Vec<u8>, Option<u32>)>,
    /// Files to remove (target paths)
    pub removals: Vec<String>,
}

impl DerivedSpec {
    /// Create a new derived spec with minimal configuration
    pub fn new(name: String, parent_name: String) -> Self {
        Self {
            name,
            parent_name,
            parent_version: None,
            version_policy: VersionPolicy::Inherit,
            description: None,
            patches: Vec::new(),
            overrides: Vec::new(),
            removals: Vec::new(),
        }
    }

    /// Add a patch
    pub fn add_patch(mut self, name: String, content: Vec<u8>) -> Self {
        self.patches.push((name, content));
        self
    }

    /// Add a file override
    pub fn add_override(mut self, target: String, content: Vec<u8>) -> Self {
        self.overrides.push((target, content, None));
        self
    }

    /// Add a file override with permissions
    pub fn add_override_with_perms(
        mut self,
        target: String,
        content: Vec<u8>,
        perms: u32,
    ) -> Self {
        self.overrides.push((target, content, Some(perms)));
        self
    }

    /// Add a file removal
    pub fn add_removal(mut self, target: String) -> Self {
        self.removals.push(target);
        self
    }
}

/// Result of building a derived package
#[derive(Debug)]
pub struct DerivedResult {
    /// Name of the derived package
    pub name: String,
    /// Version of the derived package
    pub version: String,
    /// Parent package name
    pub parent_name: String,
    /// Parent package version
    pub parent_version: String,
    /// Files in the derived package (path -> hash)
    pub files: HashMap<String, DerivedFile>,
    /// Content blobs (hash -> content)
    pub blobs: HashMap<String, Vec<u8>>,
    /// Total size
    pub total_size: u64,
    /// Patches that were applied
    pub patches_applied: Vec<String>,
    /// Files that were overridden
    pub files_overridden: Vec<String>,
    /// Files that were removed
    pub files_removed: Vec<String>,
}

/// A file in a derived package
#[derive(Debug, Clone)]
pub struct DerivedFile {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub permissions: u32,
    /// Whether this file was modified from the parent
    pub modified: bool,
}

/// Derived package builder
pub struct DerivedBuilder<'a> {
    spec: DerivedSpec,
    conn: &'a Connection,
    cas: Option<&'a CasStore>,
}

impl<'a> DerivedBuilder<'a> {
    /// Create a new builder
    pub fn new(spec: DerivedSpec, conn: &'a Connection) -> Self {
        Self {
            spec,
            conn,
            cas: None,
        }
    }

    /// Set the CAS for reading parent file contents
    pub fn with_cas(mut self, cas: &'a CasStore) -> Self {
        self.cas = Some(cas);
        self
    }

    /// Build the derived package
    pub fn build(&self) -> Result<DerivedResult> {
        info!(
            "Building derived package '{}' from parent '{}'",
            self.spec.name, self.spec.parent_name
        );

        // Find the parent trove
        let parent = self.find_parent()?;
        let parent_version = parent.version.clone();

        // Compute derived version
        let derived_version = self.spec.version_policy.compute_version(&parent_version);
        info!("Derived version: {}", derived_version);

        // Get parent files
        let parent_files = FileEntry::find_by_trove(self.conn, parent.id.unwrap())?;
        debug!("Parent has {} files", parent_files.len());

        // Create working directory for patch application
        let work_dir = TempDir::new().map_err(|e| Error::InitError(e.to_string()))?;

        // Extract parent files to work directory
        let mut files: HashMap<String, DerivedFile> = HashMap::new();
        let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();

        for file in &parent_files {
            // Get file content from CAS if available
            let content = if let Some(cas) = self.cas {
                cas.retrieve(&file.sha256_hash).ok()
            } else {
                None
            };

            files.insert(
                file.path.clone(),
                DerivedFile {
                    path: file.path.clone(),
                    hash: file.sha256_hash.clone(),
                    size: file.size as u64,
                    permissions: file.permissions as u32,
                    modified: false,
                },
            );

            // If we have CAS content, extract to work dir and store in blobs
            if let Some(content) = content {
                // Write to work directory for patching
                let work_path = work_dir.path().join(file.path.trim_start_matches('/'));
                if let Some(parent_path) = work_path.parent() {
                    std::fs::create_dir_all(parent_path)
                        .map_err(|e| Error::InitError(e.to_string()))?;
                }
                std::fs::write(&work_path, &content)
                    .map_err(|e| Error::InitError(e.to_string()))?;

                blobs.insert(file.sha256_hash.clone(), content);
            }
        }

        // Apply patches
        let mut patches_applied = Vec::new();
        for (idx, (patch_name, patch_content)) in self.spec.patches.iter().enumerate() {
            debug!("Applying patch {}: {}", idx + 1, patch_name);
            self.apply_patch(work_dir.path(), patch_content, 1)?;
            patches_applied.push(patch_name.clone());
        }

        // If patches were applied, rescan the work directory
        if !patches_applied.is_empty() {
            self.rescan_after_patch(work_dir.path(), &mut files, &mut blobs)?;
        }

        // Apply file overrides
        let mut files_overridden = Vec::new();
        for (target_path, content, perms) in &self.spec.overrides {
            debug!("Overriding file: {}", target_path);
            let new_hash = hash::sha256(content);

            // Update file entry
            if let Some(file) = files.get_mut(target_path) {
                file.hash = new_hash.clone();
                file.size = content.len() as u64;
                if let Some(p) = perms {
                    file.permissions = *p;
                }
                file.modified = true;
            } else {
                // New file
                files.insert(
                    target_path.clone(),
                    DerivedFile {
                        path: target_path.clone(),
                        hash: new_hash.clone(),
                        size: content.len() as u64,
                        permissions: perms.unwrap_or(0o644),
                        modified: true,
                    },
                );
            }

            blobs.insert(new_hash, content.clone());
            files_overridden.push(target_path.clone());
        }

        // Apply removals
        let mut files_removed = Vec::new();
        for target_path in &self.spec.removals {
            if files.remove(target_path).is_some() {
                debug!("Removed file: {}", target_path);
                files_removed.push(target_path.clone());
            } else {
                warn!("File to remove not found: {}", target_path);
            }
        }

        // Calculate total size
        let total_size: u64 = files.values().map(|f| f.size).sum();

        info!(
            "Derived package built: {} files, {} patches applied, {} overrides, {} removals",
            files.len(),
            patches_applied.len(),
            files_overridden.len(),
            files_removed.len()
        );

        Ok(DerivedResult {
            name: self.spec.name.clone(),
            version: derived_version,
            parent_name: self.spec.parent_name.clone(),
            parent_version,
            files,
            blobs,
            total_size,
            patches_applied,
            files_overridden,
            files_removed,
        })
    }

    /// Find the parent trove
    fn find_parent(&self) -> Result<Trove> {
        let troves = Trove::find_by_name(self.conn, &self.spec.parent_name)?;

        if troves.is_empty() {
            return Err(Error::InitError(format!(
                "Parent package '{}' not found",
                self.spec.parent_name
            )));
        }

        // If version constraint specified, filter
        if let Some(ref version) = self.spec.parent_version {
            for trove in troves {
                if trove.version == *version {
                    return Ok(trove);
                }
            }
            return Err(Error::InitError(format!(
                "Parent package '{}' version '{}' not found",
                self.spec.parent_name, version
            )));
        }

        // Return first (most recent) version
        Ok(troves.into_iter().next().unwrap())
    }

    /// Apply a patch using the system `patch` command
    fn apply_patch(&self, work_dir: &Path, patch_content: &[u8], strip_level: i32) -> Result<()> {
        let mut cmd = Command::new("patch")
            .arg("-p")
            .arg(strip_level.to_string())
            .arg("--no-backup-if-mismatch")
            .arg("-d")
            .arg(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::InitError(format!("Failed to run patch command: {}", e)))?;

        // Write patch content to stdin
        if let Some(mut stdin) = cmd.stdin.take() {
            stdin
                .write_all(patch_content)
                .map_err(|e| Error::InitError(format!("Failed to write patch: {}", e)))?;
        }

        let output = cmd
            .wait_with_output()
            .map_err(|e| Error::InitError(format!("Failed to wait for patch: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::InitError(format!("Patch failed: {}", stderr)));
        }

        Ok(())
    }

    /// Rescan work directory after patching to update file hashes
    fn rescan_after_patch(
        &self,
        work_dir: &Path,
        files: &mut HashMap<String, DerivedFile>,
        blobs: &mut HashMap<String, Vec<u8>>,
    ) -> Result<()> {
        for file in files.values_mut() {
            let work_path = work_dir.join(file.path.trim_start_matches('/'));
            if work_path.exists() {
                let content = std::fs::read(&work_path)
                    .map_err(|e| Error::InitError(format!("Failed to read patched file: {}", e)))?;

                let new_hash = hash::sha256(&content);
                if new_hash != file.hash {
                    debug!("File modified by patch: {}", file.path);
                    file.hash = new_hash.clone();
                    file.size = content.len() as u64;
                    file.modified = true;
                    blobs.insert(new_hash, content);
                }
            }
        }

        Ok(())
    }

    /// Save the derived package definition to the database
    pub fn save_definition(&self) -> Result<DerivedPackage> {
        let mut derived = DerivedPackage::new(self.spec.name.clone(), self.spec.parent_name.clone());
        derived.parent_version = self.spec.parent_version.clone();
        derived.version_policy = self.spec.version_policy.clone();
        derived.description = self.spec.description.clone();
        derived.insert(self.conn)?;

        let derived_id = derived.id.unwrap();

        // Save patches
        for (idx, (patch_name, patch_content)) in self.spec.patches.iter().enumerate() {
            let patch_hash = hash::sha256(patch_content);
            let mut patch = DerivedPatch::new(
                derived_id,
                (idx + 1) as i32,
                patch_name.clone(),
                patch_hash,
            );
            patch.insert(self.conn)?;
        }

        // Save overrides
        for (target_path, content, perms) in &self.spec.overrides {
            let source_hash = hash::sha256(content);
            let mut ov = DerivedOverride::new_replace(derived_id, target_path.clone(), source_hash);
            ov.permissions = perms.map(|p| p as i32);
            ov.insert(self.conn)?;
        }

        // Save removals
        for target_path in &self.spec.removals {
            let mut ov = DerivedOverride::new_remove(derived_id, target_path.clone());
            ov.insert(self.conn)?;
        }

        Ok(derived)
    }
}

/// Build a derived package from a database definition
pub fn build_from_definition(
    conn: &Connection,
    derived: &DerivedPackage,
    cas: &CasStore,
) -> Result<DerivedResult> {
    info!("Building derived package '{}' from definition", derived.name);

    // Load patches
    let patches = derived.patches(conn)?;
    let mut patch_data = Vec::new();
    for patch in patches {
        // Get patch content from CAS
        let content = cas
            .retrieve(&patch.patch_hash)
            .map_err(|_| Error::InitError(format!("Patch content not found: {}", patch.patch_name)))?;
        patch_data.push((patch.patch_name, content));
    }

    // Load overrides
    let overrides = derived.overrides(conn)?;
    let mut override_data = Vec::new();
    let mut removal_data = Vec::new();

    for ov in overrides {
        if ov.is_removal() {
            removal_data.push(ov.target_path);
        } else if let Some(hash) = ov.source_hash {
            let content = cas
                .retrieve(&hash)
                .map_err(|_| Error::InitError(format!("Override content not found: {}", ov.target_path)))?;
            override_data.push((ov.target_path, content, ov.permissions.map(|p| p as u32)));
        }
    }

    // Create spec
    let spec = DerivedSpec {
        name: derived.name.clone(),
        parent_name: derived.parent_name.clone(),
        parent_version: derived.parent_version.clone(),
        version_policy: derived.version_policy.clone(),
        description: derived.description.clone(),
        patches: patch_data,
        overrides: override_data,
        removals: removal_data,
    };

    // Build
    let builder = DerivedBuilder::new(spec, conn).with_cas(cas);
    builder.build()
}

/// Store derived package result content in CAS
pub fn store_in_cas(result: &DerivedResult, cas: &mut CasStore) -> Result<()> {
    for (hash, content) in &result.blobs {
        if !cas.exists(hash) {
            cas.store(content)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_spec_builder() {
        let spec = DerivedSpec::new("nginx-custom".to_string(), "nginx".to_string())
            .add_patch("fix.patch".to_string(), b"patch content".to_vec())
            .add_override("/etc/nginx/nginx.conf".to_string(), b"custom config".to_vec())
            .add_removal("/etc/nginx/default.conf".to_string());

        assert_eq!(spec.name, "nginx-custom");
        assert_eq!(spec.parent_name, "nginx");
        assert_eq!(spec.patches.len(), 1);
        assert_eq!(spec.overrides.len(), 1);
        assert_eq!(spec.removals.len(), 1);
    }

    #[test]
    fn test_version_computation() {
        let spec = DerivedSpec {
            name: "test".to_string(),
            parent_name: "parent".to_string(),
            parent_version: None,
            version_policy: VersionPolicy::Suffix("+custom".to_string()),
            description: None,
            patches: vec![],
            overrides: vec![],
            removals: vec![],
        };

        let version = spec.version_policy.compute_version("1.0.0");
        assert_eq!(version, "1.0.0+custom");
    }
}
