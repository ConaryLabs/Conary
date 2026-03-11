// conary-core/src/model/lockfile.rs

//! Model lockfile for pinning remote include content hashes
//!
//! The lockfile records the exact content hash of each resolved remote
//! collection, preventing silent upstream changes from affecting the system.

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::ModelResult;
use super::remote::CollectionData;

/// The lockfile structure, serialized as TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLock {
    pub metadata: LockMetadata,
    #[serde(rename = "collection")]
    pub collections: Vec<LockedCollection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockMetadata {
    pub generated_at: String,
    /// SHA-256 hash of the model file itself
    pub model_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedCollection {
    pub name: String,
    pub label: String,
    pub version: String,
    pub content_hash: String,
    pub locked_at: String,
    pub member_count: usize,
}

/// A drift detected between the lockfile and current remote state
#[derive(Debug)]
pub struct LockDrift {
    pub name: String,
    pub label: String,
    pub locked_hash: String,
    pub current_hash: String,
}

impl ModelLock {
    /// Load a lockfile from disk
    pub fn load(path: &Path) -> ModelResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let lock: Self = toml::from_str(&content).map_err(super::ModelError::ParseError)?;
        Ok(lock)
    }

    /// Save the lockfile to disk
    pub fn save(&self, path: &Path) -> ModelResult<()> {
        let content = toml::to_string_pretty(self).map_err(|e| {
            super::ModelError::RemoteFetchError(format!("Failed to serialize lockfile: {}", e))
        })?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Build a lockfile from resolved remote collections.
    ///
    /// Takes a vec of (name, label, collection_data) tuples representing
    /// all remote includes that were resolved.
    ///
    /// The `model_hash` field is left empty. Use [`Self::from_resolved_with_model`]
    /// to populate it at construction time, or set `metadata.model_hash` after
    /// construction.
    pub fn from_resolved(collections: &[(String, String, &CollectionData)]) -> Self {
        Self::build_locked(collections, String::new())
    }

    /// Build a lockfile from resolved remote collections, computing the model
    /// hash from the raw model file bytes.
    ///
    /// This ensures `model_hash` is always populated when the model content is
    /// available, avoiding lockfiles with an empty hash.
    pub fn from_resolved_with_model(
        collections: &[(String, String, &CollectionData)],
        model_bytes: &[u8],
    ) -> Self {
        let model_hash = format!("sha256:{}", crate::hash::sha256(model_bytes));
        Self::build_locked(collections, model_hash)
    }

    /// Shared construction logic for lockfile builders.
    fn build_locked(
        collections: &[(String, String, &CollectionData)],
        model_hash: String,
    ) -> Self {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let locked: Vec<LockedCollection> = collections
            .iter()
            .map(|(name, label, data)| LockedCollection {
                name: name.clone(),
                label: label.clone(),
                version: data.version.clone(),
                content_hash: data.content_hash.clone(),
                locked_at: now.clone(),
                member_count: data.members.len(),
            })
            .collect();

        Self {
            metadata: LockMetadata {
                generated_at: now,
                model_hash,
            },
            collections: locked,
        }
    }

    /// Compare locked hashes against current state
    ///
    /// Takes a vec of (name, label, current_content_hash) tuples and returns
    /// a list of drifts where the hash has changed.
    pub fn check_drift(&self, current: &[(String, String, String)]) -> Vec<LockDrift> {
        use std::collections::HashMap;

        // Build lookup map for O(1) per-item checks instead of O(N*M) linear scan
        let locked_map: HashMap<(&str, &str), &str> = self
            .collections
            .iter()
            .map(|c| ((c.name.as_str(), c.label.as_str()), c.content_hash.as_str()))
            .collect();

        let mut drifts = Vec::new();

        for (name, label, current_hash) in current {
            if let Some(&locked_hash) = locked_map.get(&(name.as_str(), label.as_str()))
                && locked_hash != current_hash
            {
                drifts.push(LockDrift {
                    name: name.clone(),
                    label: label.clone(),
                    locked_hash: locked_hash.to_string(),
                    current_hash: current_hash.clone(),
                });
            }
        }

        drifts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::remote::{CollectionData, CollectionMemberData};
    use std::collections::HashMap;
    use tempfile::NamedTempFile;

    fn make_collection_data(name: &str, hash: &str, members: usize) -> CollectionData {
        let member_list: Vec<CollectionMemberData> = (0..members)
            .map(|i| CollectionMemberData {
                name: format!("pkg-{}", i),
                version_constraint: None,
                is_optional: false,
            })
            .collect();

        CollectionData {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            members: member_list,
            includes: vec![],
            pins: HashMap::new(),
            exclude: vec![],
            content_hash: hash.to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_lockfile_roundtrip() {
        let data = make_collection_data("group-base", "sha256:abc123", 3);
        let collections = vec![("group-base".to_string(), "repo:stable".to_string(), &data)];
        let lock = ModelLock::from_resolved(&collections);

        // Save to temp file
        let temp = NamedTempFile::new().unwrap();
        lock.save(temp.path()).unwrap();

        // Load back
        let loaded = ModelLock::load(temp.path()).unwrap();

        assert_eq!(loaded.collections.len(), 1);
        assert_eq!(loaded.collections[0].name, "group-base");
        assert_eq!(loaded.collections[0].label, "repo:stable");
        assert_eq!(loaded.collections[0].content_hash, "sha256:abc123");
        assert_eq!(loaded.collections[0].version, "1.0.0");
        assert_eq!(loaded.collections[0].member_count, 3);
        assert_eq!(loaded.metadata.generated_at, lock.metadata.generated_at);
    }

    #[test]
    fn test_lock_from_collections() {
        let data1 = make_collection_data("group-base", "sha256:aaa", 5);
        let data2 = make_collection_data("group-extra", "sha256:bbb", 2);
        let collections = vec![
            ("group-base".to_string(), "repo:stable".to_string(), &data1),
            ("group-extra".to_string(), "extras:dev".to_string(), &data2),
        ];

        let lock = ModelLock::from_resolved(&collections);

        assert_eq!(lock.collections.len(), 2);
        assert_eq!(lock.collections[0].name, "group-base");
        assert_eq!(lock.collections[0].label, "repo:stable");
        assert_eq!(lock.collections[0].content_hash, "sha256:aaa");
        assert_eq!(lock.collections[0].member_count, 5);
        assert_eq!(lock.collections[1].name, "group-extra");
        assert_eq!(lock.collections[1].label, "extras:dev");
        assert_eq!(lock.collections[1].content_hash, "sha256:bbb");
        assert_eq!(lock.collections[1].member_count, 2);
        assert!(!lock.metadata.generated_at.is_empty());
    }

    #[test]
    fn test_check_drift_detects_change() {
        let data = make_collection_data("group-base", "sha256:original", 3);
        let collections = vec![("group-base".to_string(), "repo:stable".to_string(), &data)];
        let lock = ModelLock::from_resolved(&collections);

        // Check with a different hash
        let current = vec![(
            "group-base".to_string(),
            "repo:stable".to_string(),
            "sha256:changed".to_string(),
        )];
        let drifts = lock.check_drift(&current);

        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].name, "group-base");
        assert_eq!(drifts[0].locked_hash, "sha256:original");
        assert_eq!(drifts[0].current_hash, "sha256:changed");
    }

    #[test]
    fn test_check_drift_no_change() {
        let data = make_collection_data("group-base", "sha256:same", 3);
        let collections = vec![("group-base".to_string(), "repo:stable".to_string(), &data)];
        let lock = ModelLock::from_resolved(&collections);

        // Check with same hash
        let current = vec![(
            "group-base".to_string(),
            "repo:stable".to_string(),
            "sha256:same".to_string(),
        )];
        let drifts = lock.check_drift(&current);

        assert!(drifts.is_empty());
    }
}
