// crates/conary-core/src/repository/static_repo/sync/tests.rs

use super::fetch_static_sync_snapshot;
use crate::ccs::signing::SigningKeyPair;
use crate::db::models::{Repository, RepositoryPackageKeyStatus};
use crate::hash::sha256;
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryProvide, RepositoryRequirementClause,
    RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::static_repo::SCHEMA_VERSION;
use crate::repository::sync::types::RepositorySyncSnapshot;
use crate::trust::metadata::{TargetDescription, VerifiedTufState};
use std::collections::BTreeMap;
use std::path::Path;

const PACKAGE_PATH: &str = "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs";

struct StaticSyncFixture {
    _tempdir: tempfile::TempDir,
    repo: Repository,
    package_bytes: Vec<u8>,
    package_key_active: String,
    package_key_retired: String,
}

impl StaticSyncFixture {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tempdir.path().join("packages/acme-widget")).unwrap();
        std::fs::create_dir_all(tempdir.path().join("keys")).unwrap();

        let package_bytes = b"static ccs payload".to_vec();
        std::fs::write(tempdir.path().join(PACKAGE_PATH), &package_bytes).unwrap();

        let active_key = SigningKeyPair::generate().with_key_id("active");
        let retired_key = SigningKeyPair::generate().with_key_id("retired");
        let package_key_active = active_key.public_key_base64();
        let package_key_retired = retired_key.public_key_base64();

        let mut fixture = Self {
            repo: Self::repo_for(tempdir.path()),
            _tempdir: tempdir,
            package_bytes,
            package_key_active,
            package_key_retired,
        };
        fixture.write_valid_index(7);
        fixture.write_valid_package_keys();
        fixture
    }

    fn repo_for(root: &Path) -> Repository {
        let mut repo = Repository::new("static-test".to_string(), root.display().to_string());
        repo.id = Some(42);
        repo.default_strategy = Some("static".to_string());
        repo.tuf_enabled = true;
        repo
    }

    fn root(&self) -> &Path {
        self._tempdir.path()
    }

    fn write_valid_index(&mut self, index_version: u64) {
        let package_sha = sha256(&self.package_bytes);
        let provides = Self::typed_provides();
        let requirements = Self::typed_requirements();
        let index = serde_json::json!({
            "schema": SCHEMA_VERSION,
            "name": "acme-tools",
            "index_version": index_version,
            "generated": "2026-06-10T18:00:00Z",
            "packages": [{
                "name": "acme-widget",
                "version": "1.4.2",
                "version_scheme": "conary",
                "release": "1",
                "arch": "x86_64",
                "path": PACKAGE_PATH,
                "sha256": package_sha,
                "size": self.package_bytes.len() as u64,
                "description": "Widget frobnicator",
                "provides": provides,
                "requirements": requirements,
                "relations": []
            }]
        });
        self.write_bytes(
            "index.json",
            serde_json::to_string(&index).unwrap().as_bytes(),
        );
    }

    fn write_index_with_path(&mut self, path: &str) {
        let package_sha = sha256(&self.package_bytes);
        let provides = Self::typed_provides();
        let requirements = Self::typed_requirements();
        let index = serde_json::json!({
            "schema": SCHEMA_VERSION,
            "name": "acme-tools",
            "index_version": 7,
            "generated": "2026-06-10T18:00:00Z",
            "packages": [{
                "name": "acme-widget",
                "version": "1.4.2",
                "version_scheme": "conary",
                "release": "1",
                "arch": "x86_64",
                "path": path,
                "sha256": package_sha,
                "size": self.package_bytes.len() as u64,
                "provides": provides,
                "requirements": requirements,
                "relations": []
            }]
        });
        self.write_bytes(
            "index.json",
            serde_json::to_string(&index).unwrap().as_bytes(),
        );
    }

    fn typed_provides() -> Vec<RepositoryProvide> {
        vec![
            RepositoryProvide::package_name("acme-widget".to_string(), Some("1.4.2".to_string())),
            RepositoryProvide {
                name: "widget-api".to_string(),
                kind: RepositoryCapabilityKind::Generic,
                version: Some("3.0.0".to_string()),
                version_relation: Some(
                    crate::repository::dependency_model::ProvideVersionRelation::Equal,
                ),
                architecture_qualifier:
                    crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                native_text: Some("widget-api = 3.0.0".to_string()),
                provenance:
                    crate::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
            },
        ]
    }

    fn typed_requirements() -> Vec<RepositoryRequirementGroup> {
        vec![
            RepositoryRequirementGroup::simple(
                RepositoryRequirementKind::Depends,
                RepositoryRequirementClause::versioned(
                    "libfoo".to_string(),
                    ">= 2.0.0".to_string(),
                ),
            )
            .with_native_text("libfoo >= 2.0.0".to_string()),
            RepositoryRequirementGroup::simple(
                RepositoryRequirementKind::Depends,
                RepositoryRequirementClause::name_only("libbar".to_string()),
            )
            .with_native_text("libbar".to_string()),
        ]
    }

    fn write_valid_package_keys(&mut self) {
        let keys = serde_json::json!({
            "schema": SCHEMA_VERSION,
            "keys": [
                {
                    "algorithm": "ed25519",
                    "public_key": self.package_key_active,
                    "key_id": "active-key",
                    "status": "active"
                },
                {
                    "algorithm": "ed25519",
                    "public_key": self.package_key_retired,
                    "key_id": "retired-key",
                    "status": "retired"
                }
            ]
        });
        self.write_bytes(
            "keys/package-keys.json",
            serde_json::to_string(&keys).unwrap().as_bytes(),
        );
    }

    fn write_package_keys_with_status(&mut self, status: &str) {
        let keys = serde_json::json!({
            "schema": SCHEMA_VERSION,
            "keys": [{
                "algorithm": "ed25519",
                "public_key": self.package_key_active,
                "key_id": "publish",
                "status": status
            }]
        });
        self.write_bytes(
            "keys/package-keys.json",
            serde_json::to_string(&keys).unwrap().as_bytes(),
        );
    }

    fn write_bytes(&self, relative: &str, bytes: &[u8]) {
        std::fs::write(self.root().join(relative), bytes).unwrap();
    }

    fn verified(&self) -> VerifiedTufState {
        let mut targets = BTreeMap::new();
        targets.insert(
            "index.json".to_string(),
            target_for_bytes(&std::fs::read(self.root().join("index.json")).unwrap()),
        );
        targets.insert(
            "keys/package-keys.json".to_string(),
            target_for_bytes(&std::fs::read(self.root().join("keys/package-keys.json")).unwrap()),
        );
        targets.insert(
            PACKAGE_PATH.to_string(),
            target_for_bytes(&self.package_bytes),
        );

        VerifiedTufState {
            root_version: 1,
            targets_version: 7,
            snapshot_version: 7,
            timestamp_version: 7,
            targets,
        }
    }
}

fn target_for_bytes(bytes: &[u8]) -> TargetDescription {
    let mut hashes = BTreeMap::new();
    hashes.insert("sha256".to_string(), sha256(bytes));
    TargetDescription {
        length: bytes.len() as u64,
        hashes,
    }
}

fn set_target_hash(verified: &mut VerifiedTufState, path: &str, hash: &str) {
    verified
        .targets
        .get_mut(path)
        .unwrap()
        .hashes
        .insert("sha256".to_string(), hash.to_string());
}

fn assert_target_mismatch_before_parse(error: impl std::fmt::Display, path: &str) {
    let message = error.to_string();
    assert!(
        message.contains(path),
        "expected target path {path}, got: {message}"
    );
    assert!(
        message.contains("hash mismatch") || message.contains("length mismatch"),
        "expected target hash/length mismatch, got: {message}"
    );
    assert!(
        !message.contains("JSON"),
        "expected target verification before parse, got: {message}"
    );
}

#[tokio::test]
async fn verified_static_index_maps_typed_requirements_and_provides() {
    let fixture = StaticSyncFixture::new();

    let snapshot = fetch_static_sync_snapshot(&fixture.repo, &fixture.verified())
        .await
        .unwrap();

    let RepositorySyncSnapshot::StaticRows {
        packages,
        package_keys,
    } = snapshot
    else {
        panic!("expected static rows");
    };
    assert_eq!(package_keys.len(), 2);
    assert_eq!(packages.len(), 1);
    assert!(package_keys.iter().any(|key| {
        key.public_key == fixture.package_key_active
            && key.key_id.as_deref() == Some("active-key")
            && key.status == RepositoryPackageKeyStatus::Active
    }));
    assert!(package_keys.iter().any(|key| {
        key.public_key == fixture.package_key_retired
            && key.key_id.as_deref() == Some("retired-key")
            && key.status == RepositoryPackageKeyStatus::Retired
    }));

    let row = &packages[0];
    assert_eq!(row.package.name, "acme-widget");
    assert_eq!(row.package.version, "1.4.2");
    assert_eq!(row.package.architecture.as_deref(), Some("x86_64"));
    assert_eq!(
        row.package.download_url,
        fixture.root().join(PACKAGE_PATH).display().to_string()
    );
    assert!(row.provides.iter().any(|provide| {
        provide.capability == "acme-widget"
            && provide.version.as_deref() == Some("1.4.2")
            && provide.raw.is_none()
    }));
    assert!(row.provides.iter().any(|provide| {
        provide.capability == "widget-api"
            && provide.version.as_deref() == Some("3.0.0")
            && provide.kind == "generic"
    }));
    assert_eq!(row.requirement_groups.len(), 2);
    let clauses = row
        .requirement_group_clauses
        .iter()
        .flatten()
        .collect::<Vec<_>>();
    assert!(clauses.iter().any(|requirement| {
        requirement.capability == "libfoo"
            && requirement.version_constraint.as_deref() == Some(">= 2.0.0")
            && requirement.raw.is_none()
    }));
    assert!(clauses.iter().any(|requirement| {
        requirement.capability == "libbar"
            && requirement.version_constraint.is_none()
            && requirement.raw.is_none()
    }));
}

#[tokio::test]
async fn index_version_must_equal_verified_targets_version() {
    let mut fixture = StaticSyncFixture::new();
    fixture.write_valid_index(6);
    let err = fetch_static_sync_snapshot(&fixture.repo, &fixture.verified())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("index_version 6"));
    assert!(err.to_string().contains("targets version 7"));
}

#[tokio::test]
async fn tuf_targets_are_required_for_index_keys_and_packages() {
    for missing in ["index.json", "keys/package-keys.json", PACKAGE_PATH] {
        let fixture = StaticSyncFixture::new();
        let mut verified = fixture.verified();
        verified.targets.remove(missing);

        let err = fetch_static_sync_snapshot(&fixture.repo, &verified)
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains(missing),
            "expected missing target path {missing}, got {err}"
        );
    }
}

#[tokio::test]
async fn index_target_mismatch_fails_before_parse() {
    let fixture = StaticSyncFixture::new();
    let verified = fixture.verified();
    fixture.write_bytes("index.json", b"not json");

    let err = fetch_static_sync_snapshot(&fixture.repo, &verified)
        .await
        .unwrap_err();

    assert_target_mismatch_before_parse(err, "index.json");
}

#[tokio::test]
async fn package_keys_target_mismatch_fails_before_parse() {
    let fixture = StaticSyncFixture::new();
    let verified = fixture.verified();
    fixture.write_bytes("keys/package-keys.json", b"not json");

    let err = fetch_static_sync_snapshot(&fixture.repo, &verified)
        .await
        .unwrap_err();

    assert_target_mismatch_before_parse(err, "keys/package-keys.json");
}

#[tokio::test]
async fn package_target_hash_and_length_must_match_index_entry() {
    let fixture = StaticSyncFixture::new();

    let mut bad_hash = fixture.verified();
    set_target_hash(&mut bad_hash, PACKAGE_PATH, &"0".repeat(64));
    let err = fetch_static_sync_snapshot(&fixture.repo, &bad_hash)
        .await
        .unwrap_err();
    assert!(err.to_string().contains(PACKAGE_PATH));
    assert!(err.to_string().contains("hash"));

    let mut bad_length = fixture.verified();
    bad_length.targets.get_mut(PACKAGE_PATH).unwrap().length += 1;
    let err = fetch_static_sync_snapshot(&fixture.repo, &bad_length)
        .await
        .unwrap_err();
    assert!(err.to_string().contains(PACKAGE_PATH));
    assert!(err.to_string().contains("length"));
}

#[tokio::test]
async fn package_path_traversal_fails_through_static_sync_conversion() {
    let mut fixture = StaticSyncFixture::new();
    let traversal_path = "packages/acme-widget/%2e%2e/acme-widget-1.4.2-1-x86_64.ccs";
    fixture.write_index_with_path(traversal_path);

    let err = fetch_static_sync_snapshot(&fixture.repo, &fixture.verified())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("dot-dot"));
}

#[tokio::test]
async fn package_keys_with_unknown_status_fail() {
    let mut fixture = StaticSyncFixture::new();
    fixture.write_package_keys_with_status("compromised");

    let err = fetch_static_sync_snapshot(&fixture.repo, &fixture.verified())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("unknown variant"));
    assert!(err.to_string().contains("compromised"));
}
