// conary-core/src/derived/builder.rs

//! Derived package builder implementation
//!
//! Takes a parent package and applies modifications to create a derived version.

use crate::db::models::{DerivedOverride, DerivedPackage, DerivedPatch, Trove, VersionPolicy};
use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use crate::hash;
use crate::payload::{PayloadContentAuthority, PayloadNode, PayloadNodeKind};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use tempfile::TempDir;
use tracing::{debug, info, warn};

mod parent;
mod patch;

/// One exact patch input for a derived build.
#[derive(Debug, Clone)]
pub struct DerivedPatchInput {
    pub name: String,
    pub content: Vec<u8>,
    pub strip_level: u32,
}

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
    /// Patches to apply in declaration order.
    pub patches: Vec<DerivedPatchInput>,
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
        self.patches.push(DerivedPatchInput {
            name,
            content,
            strip_level: 1,
        });
        self
    }

    /// Add a patch with an explicit path-component strip level.
    pub fn add_patch_with_strip(
        mut self,
        name: String,
        content: Vec<u8>,
        strip_level: u32,
    ) -> Self {
        self.patches.push(DerivedPatchInput {
            name,
            content,
            strip_level,
        });
        self
    }

    /// Add a file override
    pub fn add_override(mut self, target: String, content: Vec<u8>) -> Self {
        self.overrides.push((target, content, None));
        self
    }

    /// Add a file override with permissions
    pub fn add_override_with_perms(mut self, target: String, content: Vec<u8>, perms: u32) -> Self {
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
    pub node: PayloadNode,
    pub content: Option<PayloadContentAuthority>,
    /// Whether this file was modified from the parent
    pub modified: bool,
}

/// Persisted build metadata returned after recording an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedDerivedArtifact {
    pub version: String,
    pub parent_version: String,
    pub artifact_hash: String,
    pub artifact_path: String,
    pub artifact_size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DerivedArtifactManifest {
    format: String,
    name: String,
    version: String,
    parent_name: String,
    parent_version: String,
    total_size: u64,
    files: BTreeMap<String, DerivedArtifactFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DerivedArtifactFile {
    node: PayloadNode,
    content: Option<PayloadContentAuthority>,
    modified: bool,
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
        let parent_id = parent
            .id
            .ok_or_else(|| Error::InitError("Parent trove missing ID".to_string()))?;
        let parent_payload =
            crate::db::models::PackagePayloadOwnership::load(self.conn, parent_id)?;
        let parent_files = parent_payload.entries();
        debug!("Parent has {} files", parent_files.len());

        // Create working directory for patch application
        let work_dir = TempDir::new().map_err(|e| Error::InitError(e.to_string()))?;

        // Extract parent files to work directory
        let mut files: HashMap<String, DerivedFile> = HashMap::new();
        let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();

        for file in parent_files {
            let content = match file.content.as_ref() {
                Some(authority) => {
                    let cas = self.cas.ok_or_else(|| {
                        Error::InitError(format!(
                            "Cannot derive {}: parent payload {} requires CAS object {}, but no CAS authority was supplied",
                            self.spec.name, file.path, authority.sha256
                        ))
                    })?;
                    let content = cas.retrieve(&authority.sha256).map_err(|error| {
                        Error::InitError(format!(
                            "Cannot derive {}: parent payload {} could not be loaded from exact CAS object {}: {}",
                            self.spec.name, file.path, authority.sha256, error
                        ))
                    })?;
                    if content.len() as u64 != authority.size {
                        return Err(Error::InitError(format!(
                            "Cannot derive {}: parent payload {} CAS object {} has size {}, expected {}",
                            self.spec.name,
                            file.path,
                            authority.sha256,
                            content.len(),
                            authority.size
                        )));
                    }
                    Some(content)
                }
                None => None,
            };

            files.insert(
                file.path.clone(),
                DerivedFile {
                    path: file.path.clone(),
                    node: file.node.source.clone(),
                    content: file.content.clone(),
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

                let authority = file
                    .content
                    .as_ref()
                    .expect("CAS content is retrieved only for regular nodes");
                blobs.insert(authority.sha256.clone(), content);
            }
        }

        // Apply patches
        let mut patches_applied = Vec::new();
        for (idx, patch) in self.spec.patches.iter().enumerate() {
            debug!("Applying patch {}: {}", idx + 1, patch.name);
            patch::apply_patch_set(
                work_dir.path(),
                &patch.content,
                patch.strip_level,
                &mut files,
                &mut blobs,
            )?;
            patches_applied.push(patch.name.clone());
        }

        // Apply file overrides
        let mut files_overridden = Vec::new();
        for (target_path, content, perms) in &self.spec.overrides {
            validate_override_target(target_path)?;
            debug!("Overriding file: {}", target_path);
            let new_hash = hash::sha256(content);
            let new_content = PayloadContentAuthority {
                sha256: new_hash.clone(),
                size: content.len() as u64,
            };

            // Update file entry
            if let Some(file) = files.get_mut(target_path) {
                file.node.kind = PayloadNodeKind::Regular {
                    hardlink_identity: None,
                };
                if let Some(p) = perms {
                    file.node.mode = libc::S_IFREG | (*p & 0o7777);
                } else {
                    file.node.mode = libc::S_IFREG | (file.node.mode & 0o7777);
                }
                file.content = Some(new_content);
                file.modified = true;
            } else {
                // New file
                files.insert(
                    target_path.clone(),
                    DerivedFile {
                        path: target_path.clone(),
                        node: PayloadNode::regular(perms.unwrap_or(0o644)),
                        content: Some(new_content),
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
        let total_size: u64 = files
            .values()
            .filter_map(|file| file.content.as_ref().map(|content| content.size))
            .sum();

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
        parent::find_parent(
            self.conn,
            &self.spec.parent_name,
            self.spec.parent_version.as_deref(),
        )
    }

    /// Save the derived package definition to the database
    pub fn save_definition(&self) -> Result<DerivedPackage> {
        let mut derived =
            DerivedPackage::new(self.spec.name.clone(), self.spec.parent_name.clone());
        derived.parent_version = self.spec.parent_version.clone();
        derived.version_policy = self.spec.version_policy.clone();
        derived.description = self.spec.description.clone();
        derived.insert(self.conn)?;

        let derived_id = derived.id.ok_or_else(|| {
            Error::InitError("Database insert did not return an ID for derived package".into())
        })?;

        // Save patches
        for (idx, patch_input) in self.spec.patches.iter().enumerate() {
            let patch_hash = hash::sha256(&patch_input.content);
            let mut patch = DerivedPatch::new(
                derived_id,
                (idx + 1) as i32,
                patch_input.name.clone(),
                patch_hash,
            );
            patch.strip_level = i32::try_from(patch_input.strip_level).map_err(|_| {
                Error::ConfigError(format!(
                    "derived patch {} strip level {} exceeds persisted range",
                    patch_input.name, patch_input.strip_level
                ))
            })?;
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
    info!(
        "Building derived package '{}' from definition",
        derived.name
    );

    // Load patches
    let patches = derived.patches(conn)?;
    let mut patch_data = Vec::new();
    for patch in patches {
        // Get patch content from CAS
        let content = cas.retrieve(&patch.patch_hash).map_err(|_| {
            Error::InitError(format!("Patch content not found: {}", patch.patch_name))
        })?;
        let strip_level = u32::try_from(patch.strip_level).map_err(|_| {
            Error::ConfigError(format!(
                "derived patch {} has negative strip level {}",
                patch.patch_name, patch.strip_level
            ))
        })?;
        patch_data.push(DerivedPatchInput {
            name: patch.patch_name,
            content,
            strip_level,
        });
    }

    // Load overrides
    let overrides = derived.overrides(conn)?;
    let mut override_data = Vec::new();
    let mut removal_data = Vec::new();

    for ov in overrides {
        if ov.is_removal() {
            removal_data.push(ov.target_path);
        } else if let Some(hash) = ov.source_hash {
            let content = cas.retrieve(&hash).map_err(|_| {
                Error::InitError(format!("Override content not found: {}", ov.target_path))
            })?;
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
pub fn store_in_cas(result: &DerivedResult, cas: &CasStore) -> Result<()> {
    for (hash, content) in &result.blobs {
        if !cas.exists(hash) {
            cas.store(content)?;
        }
    }
    Ok(())
}

/// Persist a concrete CAS-backed build artifact for a derived build.
pub fn persist_build_artifact(
    conn: &Connection,
    derived: &mut DerivedPackage,
    result: &DerivedResult,
    cas: &CasStore,
) -> Result<PersistedDerivedArtifact> {
    store_in_cas(result, cas)?;

    let artifact = DerivedArtifactManifest {
        format: "conary-derived-v2".to_string(),
        name: result.name.clone(),
        version: result.version.clone(),
        parent_name: result.parent_name.clone(),
        parent_version: result.parent_version.clone(),
        total_size: result.total_size,
        files: result
            .files
            .iter()
            .map(|(path, file)| {
                (
                    path.clone(),
                    DerivedArtifactFile {
                        node: file.node.clone(),
                        content: file.content.clone(),
                        modified: file.modified,
                    },
                )
            })
            .collect(),
    };

    let artifact_bytes = serde_json::to_vec_pretty(&artifact)
        .map_err(|e| Error::InitError(format!("Failed to serialize derived artifact: {e}")))?;
    let artifact_hash = cas.store(&artifact_bytes)?;
    let artifact_path = format!("cas://{artifact_hash}");
    let artifact_size = artifact_bytes.len() as i64;

    derived.record_build_artifact(
        conn,
        &result.version,
        &result.parent_version,
        &artifact_hash,
        &artifact_path,
        artifact_size,
    )?;

    Ok(PersistedDerivedArtifact {
        version: result.version.clone(),
        parent_version: result.parent_version.clone(),
        artifact_hash,
        artifact_path,
        artifact_size,
    })
}

/// Parse one current derived build artifact and return its exact CAS content
/// references.
pub fn load_build_artifact_contents(
    cas: &CasStore,
    artifact_hash: &str,
) -> Result<Vec<PayloadContentAuthority>> {
    let bytes = cas.retrieve(artifact_hash)?;
    let artifact = serde_json::from_slice::<DerivedArtifactManifest>(&bytes)
        .map_err(|error| Error::InitError(format!("Invalid derived artifact manifest: {error}")))?;
    if artifact.format != "conary-derived-v2" {
        return Err(Error::InitError(format!(
            "Unsupported derived artifact format '{}'",
            artifact.format
        )));
    }
    let mut contents = Vec::new();
    for (path, file) in artifact.files {
        file.node
            .validate()
            .map_err(|error| Error::InitError(format!("Invalid derived node {path}: {error}")))?;
        file.node
            .validate_content(file.content.as_ref())
            .map_err(|error| {
                Error::InitError(format!("Invalid derived content authority {path}: {error}"))
            })?;
        if let Some(content) = file.content {
            contents.push(content);
        }
    }
    Ok(contents)
}

/// Validate that an override target path is safe
///
/// Rejects paths that:
/// - Contain null bytes
/// - Contain `..` components (path traversal)
fn validate_override_target(path: &str) -> Result<()> {
    if path.contains('\0') {
        return Err(Error::InitError(
            "Override target contains null byte".to_string(),
        ));
    }

    if path.split('/').any(|c| c == "..") {
        return Err(Error::InitError(format!(
            "Override target contains path traversal: {}",
            path
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_build_requires_exact_parent_cas_authority() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        crate::db::init(db_path.to_str().unwrap()).unwrap();
        let conn = crate::db::open(db_path.to_str().unwrap()).unwrap();

        let mut parent = Trove::new(
            "parent".to_string(),
            "1.0.0".to_string(),
            crate::db::models::TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let parent_id = parent.insert(&conn).unwrap();
        let bytes = b"exact parent bytes";
        let authority = PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        };
        let mut file = crate::db::models::FileEntry::new(
            "/usr/share/parent.txt".to_string(),
            crate::payload::ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644))
                .unwrap(),
            Some(authority.clone()),
            parent_id,
        );
        file.insert(&conn).unwrap();

        let error = DerivedBuilder::new(
            DerivedSpec::new("child".to_string(), "parent".to_string()),
            &conn,
        )
        .build()
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("no CAS authority was supplied"),
            "{message}"
        );
        assert!(message.contains(&authority.sha256), "{message}");
    }

    #[test]
    fn test_derived_spec_builder() {
        let spec = DerivedSpec::new("nginx-custom".to_string(), "nginx".to_string())
            .add_patch("fix.patch".to_string(), b"patch content".to_vec())
            .add_override(
                "/etc/nginx/nginx.conf".to_string(),
                b"custom config".to_vec(),
            )
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

    #[test]
    fn test_validate_override_target_accepts_absolute() {
        assert!(validate_override_target("/etc/passwd").is_ok());
        assert!(validate_override_target("/usr/bin/foo").is_ok());
    }

    #[test]
    fn test_validate_override_target_rejects_traversal() {
        assert!(validate_override_target("../etc/passwd").is_err());
        assert!(validate_override_target("foo/../../etc/passwd").is_err());
        assert!(validate_override_target("..").is_err());
    }

    #[test]
    fn test_validate_override_target_rejects_null_bytes() {
        assert!(validate_override_target("foo\0bar").is_err());
        assert!(validate_override_target("/usr/bin/\0test").is_err());
    }

    #[test]
    fn test_validate_override_target_accepts_valid_paths() {
        assert!(validate_override_target("etc/nginx/nginx.conf").is_ok());
        assert!(validate_override_target("usr/bin/foo").is_ok());
        assert!(validate_override_target("config.toml").is_ok());
    }
}
