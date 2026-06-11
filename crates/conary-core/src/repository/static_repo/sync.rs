// conary-core/src/repository/static_repo/sync.rs

use crate::db::models::{
    Repository, RepositoryPackage, RepositoryPackageKey, RepositoryPackageKeyStatus,
    RepositoryProvide, RepositoryRequirement,
};
use crate::error::{Error, Result};
use crate::hash::sha256;
use crate::repository::sync::types::{RepositorySyncSnapshot, SyncedPackageRow};
use crate::trust::metadata::{TargetDescription, VerifiedTufState};

use super::{PackageKeyStatus, PackageKeysFile, RepoLocation, StaticIndex, StaticPackageEntry};

const INDEX_PATH: &str = "index.json";
const PACKAGE_KEYS_PATH: &str = "keys/package-keys.json";
const MAX_STATIC_INDEX_BYTES: u64 = 50 * 1024 * 1024;
const MAX_PACKAGE_KEYS_BYTES: u64 = 10 * 1024 * 1024;

pub(in crate::repository) async fn fetch_static_sync_snapshot(
    repo: &Repository,
    verified: &VerifiedTufState,
) -> Result<RepositorySyncSnapshot> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let location = RepoLocation::parse(&repo.url)
        .map_err(|error| Error::ConfigError(format!("Invalid static repository URL: {error}")))?;

    let index_target = required_target(verified, INDEX_PATH)?;
    let index_bytes =
        fetch_verified_target(&location, INDEX_PATH, index_target, MAX_STATIC_INDEX_BYTES).await?;
    let index = parse_static_index(&index_bytes)?;

    if index.index_version != verified.targets_version {
        return Err(Error::TrustError(format!(
            "Static index index_version {} does not match verified targets version {}",
            index.index_version, verified.targets_version
        )));
    }

    let package_keys_target = required_target(verified, PACKAGE_KEYS_PATH)?;
    let package_keys_bytes = fetch_verified_target(
        &location,
        PACKAGE_KEYS_PATH,
        package_keys_target,
        MAX_PACKAGE_KEYS_BYTES,
    )
    .await?;
    let package_keys = parse_package_keys(&package_keys_bytes)?;
    index
        .validate_with_keys(&package_keys)
        .map_err(|error| Error::ParseError(format!("Invalid static package keys: {error}")))?;

    let package_key_rows = package_keys
        .keys
        .iter()
        .map(|key| RepositoryPackageKey {
            repository_id: repo_id,
            public_key: key.public_key.clone(),
            key_id: key.key_id.clone(),
            status: match key.status {
                PackageKeyStatus::Active => RepositoryPackageKeyStatus::Active,
                PackageKeyStatus::Retired => RepositoryPackageKeyStatus::Retired,
            },
            synced_at: None,
        })
        .collect();

    let package_rows = index
        .packages
        .iter()
        .map(|package| static_package_row(repo_id, &location, package, verified))
        .collect::<Result<Vec<_>>>()?;

    Ok(RepositorySyncSnapshot::StaticRows {
        packages: package_rows,
        package_keys: package_key_rows,
    })
}

async fn fetch_verified_target(
    location: &RepoLocation,
    path: &str,
    target: &TargetDescription,
    limit: u64,
) -> Result<Vec<u8>> {
    let bytes = location
        .fetch_bytes(path, limit)
        .await
        .map_err(|error| Error::DownloadError(format!("Failed to fetch static {path}: {error}")))?;
    verify_target_bytes(path, target, &bytes)?;
    Ok(bytes)
}

fn parse_static_index(bytes: &[u8]) -> Result<StaticIndex> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| Error::ParseError(format!("Invalid static index UTF-8: {error}")))?;
    StaticIndex::parse(text)
        .map_err(|error| Error::ParseError(format!("Invalid static index: {error}")))
}

fn parse_package_keys(bytes: &[u8]) -> Result<PackageKeysFile> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        Error::ParseError(format!("Invalid static package keys UTF-8: {error}"))
    })?;
    PackageKeysFile::parse(text)
        .map_err(|error| Error::ParseError(format!("Invalid static package keys: {error}")))
}

fn required_target<'a>(
    verified: &'a VerifiedTufState,
    path: &str,
) -> Result<&'a TargetDescription> {
    verified.targets.get(path).ok_or_else(|| {
        Error::TrustError(format!(
            "Static repository verified targets are missing required target {path}"
        ))
    })
}

fn verify_target_bytes(path: &str, target: &TargetDescription, bytes: &[u8]) -> Result<()> {
    if target.length != bytes.len() as u64 {
        return Err(Error::TrustError(format!(
            "Static target {path} length mismatch: targets.json has {}, fetched {}",
            target.length,
            bytes.len()
        )));
    }

    let expected = target.hashes.get("sha256").ok_or_else(|| {
        Error::TrustError(format!(
            "Static target {path} is missing required sha256 hash"
        ))
    })?;
    let actual = sha256(bytes);
    if expected != &actual {
        return Err(Error::TrustError(format!(
            "Static target {path} hash mismatch: expected {expected}, got {actual}"
        )));
    }

    Ok(())
}

fn static_package_row(
    repo_id: i64,
    location: &RepoLocation,
    entry: &StaticPackageEntry,
    verified: &VerifiedTufState,
) -> Result<SyncedPackageRow> {
    verify_package_target(entry, verified)?;

    let mut package = RepositoryPackage::new(
        repo_id,
        entry.name.clone(),
        entry.version.clone(),
        entry.sha256.clone(),
        i64::try_from(entry.size).map_err(|_| {
            Error::ParseError(format!("package.size {} exceeds i64::MAX", entry.size))
        })?,
        location
            .join_display(&entry.path)
            .map_err(|error| Error::ParseError(format!("Invalid static package path: {error}")))?,
    );
    package.architecture = Some(entry.arch.clone());
    package.description = entry.description.clone();
    package.dependencies = if entry.dependencies.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&entry.dependencies)?)
    };
    package.metadata = Some(
        serde_json::json!({
            "release": entry.release,
            "static_path": entry.path,
        })
        .to_string(),
    );

    let provides = vec![RepositoryProvide::new(
        0,
        entry.name.clone(),
        Some(entry.version.clone()),
        "package".to_string(),
        Some(entry.name.clone()),
    )];
    let requirements = entry
        .dependencies
        .iter()
        .filter_map(|dependency| static_requirement(dependency))
        .collect();

    Ok(SyncedPackageRow {
        package,
        provides,
        requirements,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    })
}

fn verify_package_target(entry: &StaticPackageEntry, verified: &VerifiedTufState) -> Result<()> {
    let target = required_target(verified, &entry.path)?;

    if target.length != entry.size {
        return Err(Error::TrustError(format!(
            "Static package target {} length mismatch: index has {}, targets.json has {}",
            entry.path, entry.size, target.length
        )));
    }

    let target_sha = target.hashes.get("sha256").ok_or_else(|| {
        Error::TrustError(format!(
            "Static package target {} is missing required sha256 hash",
            entry.path
        ))
    })?;
    if target_sha != &entry.sha256 {
        return Err(Error::TrustError(format!(
            "Static package target {} hash mismatch: index has {}, targets.json has {}",
            entry.path, entry.sha256, target_sha
        )));
    }

    Ok(())
}

fn static_requirement(raw: &str) -> Option<RepositoryRequirement> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let (capability, version_constraint) = split_dependency(raw);
    Some(RepositoryRequirement::new(
        0,
        capability,
        version_constraint,
        "package".to_string(),
        "runtime".to_string(),
        Some(raw.to_string()),
    ))
}

fn split_dependency(raw: &str) -> (String, Option<String>) {
    const OPS: [&str; 5] = ["<=", ">=", "=", "<", ">"];

    for op in OPS {
        if let Some((name, version)) = raw.split_once(op) {
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                continue;
            }
            return (name.to_string(), Some(format!("{op} {version}")));
        }
    }

    (raw.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::fetch_static_sync_snapshot;
    use crate::ccs::signing::SigningKeyPair;
    use crate::db::models::{Repository, RepositoryPackageKeyStatus};
    use crate::hash::sha256;
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
            let index = serde_json::json!({
                "schema": 1,
                "name": "acme-tools",
                "index_version": index_version,
                "generated": "2026-06-10T18:00:00Z",
                "packages": [{
                    "name": "acme-widget",
                    "version": "1.4.2",
                    "release": "1",
                    "arch": "x86_64",
                    "path": PACKAGE_PATH,
                    "sha256": package_sha,
                    "size": self.package_bytes.len() as u64,
                    "description": "Widget frobnicator",
                    "dependencies": ["libfoo >= 2.0", "libbar"]
                }]
            });
            self.write_bytes(
                "index.json",
                serde_json::to_string(&index).unwrap().as_bytes(),
            );
        }

        fn write_index_with_path(&mut self, path: &str) {
            let package_sha = sha256(&self.package_bytes);
            let index = serde_json::json!({
                "schema": 1,
                "name": "acme-tools",
                "index_version": 7,
                "generated": "2026-06-10T18:00:00Z",
                "packages": [{
                    "name": "acme-widget",
                    "version": "1.4.2",
                    "release": "1",
                    "arch": "x86_64",
                    "path": path,
                    "sha256": package_sha,
                    "size": self.package_bytes.len() as u64
                }]
            });
            self.write_bytes(
                "index.json",
                serde_json::to_string(&index).unwrap().as_bytes(),
            );
        }

        fn write_valid_package_keys(&mut self) {
            let keys = serde_json::json!({
                "schema": 1,
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
                "schema": 1,
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
                target_for_bytes(
                    &std::fs::read(self.root().join("keys/package-keys.json")).unwrap(),
                ),
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
    async fn verified_static_index_maps_packages_keys_dependencies_and_self_provides() {
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
            row.package.dependencies.as_deref(),
            Some(r#"["libfoo >= 2.0","libbar"]"#)
        );
        assert_eq!(
            row.package.download_url,
            fixture.root().join(PACKAGE_PATH).display().to_string()
        );
        assert!(row.provides.iter().any(|provide| {
            provide.capability == "acme-widget"
                && provide.version.as_deref() == Some("1.4.2")
                && provide.raw.as_deref() == Some("acme-widget")
        }));
        assert!(row.requirements.iter().any(|requirement| {
            requirement.capability == "libfoo"
                && requirement.version_constraint.as_deref() == Some(">= 2.0")
                && requirement.raw.as_deref() == Some("libfoo >= 2.0")
        }));
        assert!(row.requirements.iter().any(|requirement| {
            requirement.capability == "libbar"
                && requirement.version_constraint.is_none()
                && requirement.raw.as_deref() == Some("libbar")
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
}
