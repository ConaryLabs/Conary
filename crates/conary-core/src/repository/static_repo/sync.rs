// conary-core/src/repository/static_repo/sync.rs

use crate::db::models::{
    Repository, RepositoryPackage, RepositoryPackageKey, RepositoryPackageKeyStatus,
    RepositoryProvide,
};
use crate::error::{Error, Result};
use crate::hash::sha256;
use crate::repository::sync::types::{RepositorySyncSnapshot, SyncedPackageRow};
use crate::repository::sync::{capability_kind_to_db, convert_requirement_groups};
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
    fetch_static_sync_snapshot_with_network_policy(repo, verified, false).await
}

pub(in crate::repository) async fn fetch_static_sync_snapshot_public_network(
    repo: &Repository,
    verified: &VerifiedTufState,
) -> Result<RepositorySyncSnapshot> {
    fetch_static_sync_snapshot_with_network_policy(repo, verified, true).await
}

async fn fetch_static_sync_snapshot_with_network_policy(
    repo: &Repository,
    verified: &VerifiedTufState,
    public_network_only: bool,
) -> Result<RepositorySyncSnapshot> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let location = RepoLocation::parse(&repo.url)
        .map_err(|error| Error::ConfigError(format!("Invalid static repository URL: {error}")))?;

    let index_target = required_target(verified, INDEX_PATH)?;
    let index_bytes = fetch_verified_target(
        &location,
        INDEX_PATH,
        index_target,
        MAX_STATIC_INDEX_BYTES,
        public_network_only,
    )
    .await?;
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
        public_network_only,
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
    public_network_only: bool,
) -> Result<Vec<u8>> {
    let fetched = if public_network_only {
        location.fetch_bytes_public_network(path, limit).await
    } else {
        location.fetch_bytes(path, limit).await
    };
    let bytes = fetched
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
        entry.version_scheme,
        entry.sha256.clone(),
        i64::try_from(entry.size).map_err(|_| {
            Error::ParseError(format!("package.size {} exceeds i64::MAX", entry.size))
        })?,
        location
            .join_display(&entry.path)
            .map_err(|error| Error::ParseError(format!("Invalid static package path: {error}")))?,
    );
    package.package_release = entry.release.clone();
    package.architecture = Some(entry.arch.clone());
    package.description = entry.description.clone();
    package.metadata = Some(
        serde_json::json!({
            "release": entry.release,
            "static_path": entry.path,
        })
        .to_string(),
    );

    let provides = entry
        .provides
        .iter()
        .map(|provide| {
            RepositoryProvide::new(
                0,
                provide.name.clone(),
                provide.version.clone(),
                capability_kind_to_db(provide.kind),
                provide.native_text.clone(),
                entry.version_scheme,
            )
            .with_version_relation(provide.version_relation)
            .with_architecture_qualifier(provide.architecture_qualifier.clone())
        })
        .collect();
    let mut all_groups = entry.requirements.clone();
    all_groups.extend(entry.relations.clone());
    let (requirement_groups, requirement_group_clauses) =
        convert_requirement_groups(0, &all_groups);

    Ok(SyncedPackageRow {
        package,
        provides,
        requirement_groups,
        requirement_group_clauses,
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

#[cfg(test)]
mod tests;
