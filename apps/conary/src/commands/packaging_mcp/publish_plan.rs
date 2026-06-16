// apps/conary/src/commands/packaging_mcp/publish_plan.rs
//! Publish plan material, confirmation registry, and private artifact staging.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::ccs::attestation::canonical_json_hash;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct PublishPlanMaterial {
    pub schema_version: u16,
    pub plan_kind: String,
    pub mode: String,
    pub stored_route_enum: String,
    pub normalized_artifact_or_project_path: String,
    pub artifact_sha256: String,
    pub artifact_size: u64,
    pub artifact_manifest_identity_when_available: Option<serde_json::Value>,
    pub normalized_static_target: String,
    pub key_dir_path_when_supplied: Option<String>,
    pub state_file_path_when_supplied: Option<String>,
    pub selected_options: BTreeMap<String, serde_json::Value>,
    pub command_risk_projection: String,
    pub destination_root_key_fingerprint: Option<String>,
    pub destination_package_key_hash: Option<String>,
    pub accepted_signer_set_hash: Option<String>,
    pub publish_policy_digest: String,
    pub metadata_versions_or_watermark: Option<serde_json::Value>,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishPlanReceipt {
    pub plan_id: String,
    pub fingerprint: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredPublishPlan {
    pub plan_id: String,
    pub fingerprint: String,
    pub material: PublishPlanMaterial,
}

pub(crate) struct StagedArtifact {
    _temp: tempfile::TempDir,
    path: PathBuf,
    digest: String,
    size: u64,
}

impl StagedArtifact {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }
}

pub(crate) struct PublishPlanRegistry {
    capacity: usize,
    order: VecDeque<String>,
    plans: BTreeMap<String, StoredPublishPlan>,
}

impl PublishPlanRegistry {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            plans: BTreeMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, material: PublishPlanMaterial) -> Result<PublishPlanReceipt> {
        if self.capacity == 0 {
            bail!("publish plan registry capacity must be greater than zero");
        }
        let plan_id = format!("publish-{}", uuid::Uuid::new_v4());
        let fingerprint = canonical_json_hash(&material)?;
        let receipt = PublishPlanReceipt {
            plan_id: plan_id.clone(),
            fingerprint: fingerprint.clone(),
            expires_at: material.expires_at.clone(),
        };
        let stored = StoredPublishPlan {
            plan_id: plan_id.clone(),
            fingerprint,
            material,
        };
        self.order.push_back(plan_id.clone());
        self.plans.insert(plan_id, stored);
        while self.order.len() > self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.plans.remove(&evicted);
            }
        }
        Ok(receipt)
    }

    pub(crate) fn get_confirmed(
        &self,
        plan_id: &str,
        fingerprint: &str,
        confirmation: &str,
    ) -> Result<StoredPublishPlan> {
        let stored = self
            .plans
            .get(plan_id)
            .with_context(|| format!("publish plan {plan_id} was not found"))?;
        if stored.fingerprint != fingerprint {
            bail!("publish plan fingerprint does not match");
        }
        if confirmation != plan_id {
            bail!("publish plan confirmation must exactly match the plan id");
        }
        let expires_at = chrono::DateTime::parse_from_rfc3339(&stored.material.expires_at)
            .with_context(|| format!("parse publish plan expiry {}", stored.material.expires_at))?;
        if expires_at <= chrono::Utc::now() {
            bail!("publish plan {plan_id} has expired");
        }
        Ok(stored.clone())
    }
}

pub(crate) fn stage_artifact_private(source: &Path) -> Result<StagedArtifact> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspect artifact source {}", source.display()))?;
    if !metadata.file_type().is_file() {
        bail!("artifact source {} is not a regular file", source.display());
    }

    let temp = tempfile::Builder::new()
        .prefix("conary-publish-artifact-")
        .tempdir()
        .context("create private artifact staging directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    }

    let file_name = source
        .file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| std::ffi::OsStr::new("artifact.ccs"));
    let path = temp.path().join(file_name);
    let mut reader =
        File::open(source).with_context(|| format!("open artifact source {}", source.display()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut writer = options
        .open(&path)
        .with_context(|| format!("create staged artifact {}", path.display()))?;
    let size = io::copy(&mut reader, &mut writer)
        .with_context(|| format!("copy artifact into private staging {}", path.display()))?;
    writer.sync_all()?;
    drop(writer);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    if let Ok(dir) = File::open(temp.path()) {
        let _ = dir.sync_all();
    }
    let mut staged = File::open(&path)?;
    let digest = format!(
        "sha256:{}",
        conary_core::hash::sha256_reader_hex(&mut staged)?
    );

    Ok(StagedArtifact {
        _temp: temp,
        path,
        digest,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn material(expires_at: String) -> PublishPlanMaterial {
        PublishPlanMaterial {
            schema_version: 1,
            plan_kind: "publish".to_string(),
            mode: "artifact_static".to_string(),
            stored_route_enum: "StaticLocal".to_string(),
            normalized_artifact_or_project_path: "/tmp/pkg.ccs".to_string(),
            artifact_sha256: "sha256:artifact".to_string(),
            artifact_size: 12,
            artifact_manifest_identity_when_available: None,
            normalized_static_target: "/tmp/repo".to_string(),
            key_dir_path_when_supplied: Some("/tmp/keys".to_string()),
            state_file_path_when_supplied: None,
            selected_options: BTreeMap::from([
                ("refresh".to_string(), serde_json::json!(false)),
                ("force_reinit".to_string(), serde_json::json!(false)),
            ]),
            command_risk_projection: "high".to_string(),
            destination_root_key_fingerprint: Some("sha256:root".to_string()),
            destination_package_key_hash: Some("sha256:packages".to_string()),
            accepted_signer_set_hash: Some("sha256:signers".to_string()),
            publish_policy_digest: "static-publish-policy-v1".to_string(),
            metadata_versions_or_watermark: Some(serde_json::json!({
                "root_version": 1,
                "targets_version": 1,
            })),
            expires_at,
        }
    }

    fn future_expiry() -> String {
        (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()
    }

    #[test]
    fn registry_requires_matching_fingerprint_and_confirmation() {
        let mut registry = PublishPlanRegistry::new(16);
        let receipt = registry.insert(material(future_expiry())).unwrap();

        assert!(
            registry
                .get_confirmed(&receipt.plan_id, "sha256:wrong", &receipt.plan_id)
                .is_err()
        );
        assert!(
            registry
                .get_confirmed(&receipt.plan_id, &receipt.fingerprint, "wrong")
                .is_err()
        );

        let confirmed = registry
            .get_confirmed(&receipt.plan_id, &receipt.fingerprint, &receipt.plan_id)
            .unwrap();
        assert_eq!(confirmed.fingerprint, receipt.fingerprint);
        assert_eq!(confirmed.material.plan_kind, "publish");
    }

    #[test]
    fn registry_expires_and_evicts_oldest_plans() {
        let mut registry = PublishPlanRegistry::new(1);
        let expired = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        let expired_receipt = registry.insert(material(expired)).unwrap();
        assert!(
            registry
                .get_confirmed(
                    &expired_receipt.plan_id,
                    &expired_receipt.fingerprint,
                    &expired_receipt.plan_id,
                )
                .is_err()
        );

        let first = registry.insert(material(future_expiry())).unwrap();
        let second = registry.insert(material(future_expiry())).unwrap();
        assert!(
            registry
                .get_confirmed(&first.plan_id, &first.fingerprint, &first.plan_id)
                .is_err()
        );
        assert!(
            registry
                .get_confirmed(&second.plan_id, &second.fingerprint, &second.plan_id)
                .is_ok()
        );
    }

    #[test]
    fn stage_artifact_private_copies_regular_file_with_private_modes_and_cleanup() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("pkg.ccs");
        std::fs::write(&source, b"package bytes").unwrap();

        let staged = stage_artifact_private(&source).unwrap();
        let staged_path = staged.path().to_path_buf();
        let staged_dir = staged_path.parent().unwrap().to_path_buf();

        assert_eq!(
            staged.digest(),
            "sha256:2e547448dcd0f2fcd9dbc386d33f1553369883451898177559bcf3e3b1083d16"
        );
        assert_eq!(staged.size(), 13);
        assert_eq!(
            std::fs::metadata(&staged_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&staged_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&staged_path).unwrap(), b"package bytes");
        drop(staged);
        assert!(!staged_path.exists());
    }

    #[test]
    fn stage_artifact_private_rejects_non_regular_sources() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path().join("dir");
        std::fs::create_dir(&dir).unwrap();
        assert!(stage_artifact_private(&dir).is_err());

        #[cfg(unix)]
        {
            let source = temp.path().join("pkg.ccs");
            let link = temp.path().join("pkg-link.ccs");
            std::fs::write(&source, b"package bytes").unwrap();
            std::os::unix::fs::symlink(&source, &link).unwrap();
            assert!(stage_artifact_private(&link).is_err());
        }
    }
}
