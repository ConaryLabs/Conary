// conary-core/src/repository/sync/remi.rs

use crate::canonical::exchange::{
    CanonicalMapSnapshot, expected_checksum, parse_snapshot, replace_remi_snapshot,
};
use crate::db::models::{
    Repository, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryRequirementExpression,
    RepositoryRequirementGroup as TypedRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::metadata::PackageSecurityAdvisoryMetadata;
use crate::repository::package_relation::validate_native_relation;
use crate::repository::remi_metadata::{
    REMI_SPARSE_MIN_PACKAGE_SIZE, RemiSparseResolutionVersionEntry,
};
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use super::apply_trusted_package_security_advisory;
use super::native::append_synced_package_rows;
use super::support::run_blocking_sync;
use super::types::SyncedPackageRow;

mod sparse;

use sparse::RemiSparseSync;
#[cfg(test)]
use sparse::fetch_sparse_page_with_retry;

pub(super) fn remi_sync_row(
    repo_id: i64,
    endpoint: String,
    distro: String,
    name: String,
    entry: RemiSparseResolutionVersionEntry,
) -> Result<SyncedPackageRow> {
    let RemiSparseResolutionVersionEntry {
        version,
        release,
        architecture,
        provides: wire_provides,
        requirement_groups: wire_requirement_groups,
        size,
        metadata,
    } = entry;
    if version.is_empty() {
        return Err(Error::ConfigError(format!(
            "Remi sparse entry for '{name}' has an empty package version"
        )));
    }
    if size < REMI_SPARSE_MIN_PACKAGE_SIZE {
        return Err(Error::ConfigError(format!(
            "Remi sparse entry for '{name}' version '{version}' has non-downloadable size {size}"
        )));
    }
    let profile = crate::repository::supported_profiles::profile_for_remi_target(&distro)
        .ok_or_else(|| Error::ConfigError(format!("unsupported Remi target: {distro}")))?;
    let route_slug = profile.remi_route_slug();
    let public_profile_id = profile.id();
    let package_release = release
        .or_else(|| {
            metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/identity/release"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let mut query = vec![format!("version={}", urlencoding::encode(&version))];
    if !package_release.is_empty() {
        query.push(format!("release={}", urlencoding::encode(&package_release)));
    }
    if let Some(architecture) = architecture.as_deref() {
        query.push(format!("arch={}", urlencoding::encode(architecture)));
    }
    let download_url = format!(
        "{endpoint}/v1/{route_slug}/packages/{}/download?{}",
        urlencoding::encode(&name),
        query.join("&")
    );

    let scheme = profile.version_scheme();
    let mut package = RepositoryPackage::new(
        repo_id,
        name.clone(),
        version.clone(),
        scheme,
        "remi:server-verified".to_string(),
        size,
        download_url,
    );
    package.package_release = package_release;
    package.architecture = architecture;

    let mut metadata = metadata.unwrap_or(serde_json::Value::Null);
    if let Some(advisory_value) = metadata.get("security_advisory").cloned() {
        match serde_json::from_value::<PackageSecurityAdvisoryMetadata>(advisory_value) {
            Ok(advisory) => {
                match apply_trusted_package_security_advisory(
                    &mut package,
                    &advisory,
                    "remi",
                    "unknown",
                ) {
                    Ok(normalized) => {
                        if let Some(object) = metadata.as_object_mut() {
                            object.insert("security_advisory".to_string(), normalized);
                        }
                    }
                    Err(error) => {
                        warn!(
                            "Ignoring untrusted Remi security advisory metadata for {} {}: {}",
                            name, version, error
                        );
                    }
                }
            }
            Err(error) => {
                warn!(
                    "Ignoring malformed Remi security advisory metadata for {} {}: {}",
                    name, version, error
                );
            }
        }
    }
    package.metadata = match metadata {
        serde_json::Value::Null => None,
        ref value => Some(value.to_string()),
    };

    package.source_profile = Some(public_profile_id.to_string());

    if !wire_provides
        .iter()
        .any(|provide| provide.kind == "package" && provide.capability == name)
    {
        return Err(Error::ConfigError(format!(
            "Remi sparse entry for '{name}' has no normalized package self-provide"
        )));
    }
    let provides = wire_provides
        .into_iter()
        .map(|provide| {
            if provide.version_scheme != scheme {
                return Err(Error::ConfigError(format!(
                    "Remi provide '{}' scheme '{}' disagrees with target '{}' scheme '{}'",
                    provide.capability,
                    provide.version_scheme.as_str(),
                    public_profile_id,
                    scheme.as_str()
                )));
            }
            Ok(RepositoryProvide::new(
                0,
                provide.capability,
                provide.version,
                provide.kind,
                provide.raw,
                provide.version_scheme,
            )
            .with_version_relation(provide.version_relation)
            .with_architecture_qualifier(provide.architecture_qualifier))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut requirement_groups = Vec::with_capacity(wire_requirement_groups.len());
    let mut requirement_group_clauses = Vec::with_capacity(wire_requirement_groups.len());
    for (group_index, wire_group) in wire_requirement_groups.into_iter().enumerate() {
        let group_token = group_index
            .checked_add(1)
            .and_then(|token| i64::try_from(token).ok())
            .ok_or_else(|| {
                Error::ConfigError(format!(
                    "Remi sparse entry for '{name}' has too many requirement groups"
                ))
            })?;
        validate_remi_relation_group(&wire_group, scheme)?;
        let mut group = DbRequirementGroup::new(
            0,
            wire_group.kind,
            wire_group.behavior,
            wire_group.expression_json,
        );
        group.description = wire_group.description;
        group.native_text = wire_group.native_text;
        requirement_groups.push(group);
        requirement_group_clauses.push(
            wire_group
                .clauses
                .into_iter()
                .map(|requirement| {
                    RepositoryRequirement::new(
                        0,
                        group_token,
                        requirement.capability,
                        requirement.version_constraint,
                        requirement.kind,
                        requirement.dependency_type,
                        requirement.raw,
                    )
                })
                .collect(),
        );
    }

    Ok(SyncedPackageRow {
        package,
        provides,
        requirement_groups,
        requirement_group_clauses,
    })
}

fn validate_remi_relation_group(
    group: &crate::repository::remi_metadata::RemiRequirementGroup,
    scheme: VersionScheme,
) -> Result<()> {
    let Some(kind) = RepositoryRequirementKind::from_str_exact(&group.kind) else {
        return Err(Error::ConfigError(format!(
            "Remi requirement group has unknown kind '{}'",
            group.kind
        )));
    };
    if !kind.is_negative_relation() {
        return Ok(());
    }

    let expression = serde_json::from_str::<RepositoryRequirementExpression>(
        &group.expression_json,
    )
    .map_err(|error| {
        Error::ConfigError(format!(
            "Remi {} relation has invalid expression JSON: {error}",
            kind.as_str()
        ))
    })?;
    let behavior = match group.behavior.as_str() {
        "hard" => ConditionalRequirementBehavior::Hard,
        "conditional" => ConditionalRequirementBehavior::Conditional,
        other => {
            return Err(Error::ConfigError(format!(
                "Remi {} relation has unknown behavior '{other}'",
                kind.as_str()
            )));
        }
    };
    let alternatives = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
    let first = alternatives.first().cloned().ok_or_else(|| {
        Error::ConfigError(format!(
            "Remi {} relation has no expression atoms",
            kind.as_str()
        ))
    })?;

    let mut indexed = group
        .clauses
        .iter()
        .map(|clause| (clause.capability.clone(), clause.version_constraint.clone()))
        .collect::<Vec<_>>();
    indexed.sort();
    let mut authoritative = alternatives
        .iter()
        .map(|clause| (clause.name.clone(), clause.version_constraint.clone()))
        .collect::<Vec<_>>();
    authoritative.sort();
    if indexed != authoritative {
        return Err(Error::ConfigError(format!(
            "Remi {} relation clause index disagrees with its authoritative expression",
            kind.as_str()
        )));
    }

    let mut relation = TypedRequirementGroup::simple(kind, first)
        .with_behavior(behavior)
        .with_expression(expression);
    relation.alternatives = alternatives;
    relation.native_text = group.native_text.clone();
    validate_native_relation(&relation, scheme).map_err(Error::ConfigError)
}

#[derive(Debug, Clone, Copy)]
struct RemiSyncStage {
    repository_id: i64,
    staging_repository_id: i64,
}

fn begin_remi_sync_stage(conn: &Connection, repo: &Repository) -> Result<RemiSyncStage> {
    let repository_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut staging = repo.clone();
    staging.id = None;
    staging.name = format!("__conary_sparse_sync_{repository_id}_{nonce}");
    staging.enabled = false;
    staging.last_sync = None;
    staging.created_at = None;
    staging.tuf_enabled = false;
    staging.tuf_root_version = None;
    staging.tuf_root_url = None;
    let staging_repository_id = staging.insert(conn)?;
    Ok(RemiSyncStage {
        repository_id,
        staging_repository_id,
    })
}

fn append_remi_sync_page(
    conn: &Connection,
    stage: RemiSyncStage,
    mut rows: Vec<SyncedPackageRow>,
) -> Result<usize> {
    for row in &mut rows {
        row.package.repository_id = stage.staging_repository_id;
    }
    let tx = conn.unchecked_transaction()?;
    let count = append_synced_package_rows(&tx, rows)?;
    tx.commit()?;
    Ok(count)
}

fn finish_remi_sync_stage(
    conn: &Connection,
    repo: &mut Repository,
    stage: RemiSyncStage,
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let count = tx.query_row(
        "SELECT COUNT(*) FROM repository_packages WHERE repository_id = ?1",
        [stage.staging_repository_id],
        |row| row.get::<_, i64>(0),
    )?;
    let count = usize::try_from(count)
        .map_err(|_| Error::InitError("sparse sync package count exceeds usize".to_string()))?;
    RepositoryPackage::delete_by_repository(&tx, stage.repository_id)?;
    tx.execute(
        "UPDATE repository_packages SET repository_id = ?1 WHERE repository_id = ?2",
        [stage.repository_id, stage.staging_repository_id],
    )?;
    Repository::delete(&tx, stage.staging_repository_id)?;
    super::link_canonical_ids(&tx, stage.repository_id)?;
    repo.last_sync = Some(super::current_timestamp());
    repo.update(&tx)?;
    tx.commit()?;
    Ok(count)
}

fn abort_remi_sync_stage(conn: &Connection, stage: RemiSyncStage) {
    if let Err(error) = Repository::delete(conn, stage.staging_repository_id) {
        debug!(
            "Failed to remove sparse-sync staging repository {}: {}",
            stage.staging_repository_id, error
        );
    }
}

pub(super) async fn sync_repository_remi(
    conn: &Connection,
    repo: &mut Repository,
) -> Result<usize> {
    info!(
        "Syncing repository {} from the paginated Remi sparse index",
        repo.name
    );
    let mut fetcher = RemiSparseSync::new(repo)?;
    let stage = begin_remi_sync_stage(conn, repo)?;
    loop {
        let rows = match fetcher.next_rows().await {
            Ok(Some(rows)) => rows,
            Ok(None) => break,
            Err(error) => {
                abort_remi_sync_stage(conn, stage);
                return Err(error);
            }
        };
        if let Err(error) = append_remi_sync_page(conn, stage, rows) {
            abort_remi_sync_stage(conn, stage);
            return Err(error);
        }
    }
    let count = match finish_remi_sync_stage(conn, repo, stage) {
        Ok(count) => count,
        Err(error) => {
            abort_remi_sync_stage(conn, stage);
            return Err(error);
        }
    };
    info!(
        "Synchronized {} packages from Remi repository {}",
        count, repo.name
    );
    Ok(count)
}

pub(super) async fn sync_repository_remi_from_db_path(
    db_path: std::path::PathBuf,
    repo: Repository,
) -> Result<usize> {
    info!(
        "Syncing repository {} from the paginated Remi sparse index",
        repo.name
    );
    let mut fetcher = RemiSparseSync::new(&repo)?;
    let begin_path = db_path.clone();
    let begin_repo = repo.clone();
    let stage = run_blocking_sync(move || {
        let conn = crate::db::open_fast(&begin_path)?;
        begin_remi_sync_stage(&conn, &begin_repo)
    })
    .await?;

    loop {
        let rows = match fetcher.next_rows().await {
            Ok(Some(rows)) => rows,
            Ok(None) => break,
            Err(error) => {
                let abort_path = db_path.clone();
                let _ = run_blocking_sync(move || {
                    let conn = crate::db::open_fast(&abort_path)?;
                    abort_remi_sync_stage(&conn, stage);
                    Ok(())
                })
                .await;
                return Err(error);
            }
        };
        let page_path = db_path.clone();
        if let Err(error) = run_blocking_sync(move || {
            let conn = crate::db::open_fast(&page_path)?;
            append_remi_sync_page(&conn, stage, rows)
        })
        .await
        {
            let abort_path = db_path.clone();
            let _ = run_blocking_sync(move || {
                let conn = crate::db::open_fast(&abort_path)?;
                abort_remi_sync_stage(&conn, stage);
                Ok(())
            })
            .await;
            return Err(error);
        }
    }

    let finish_path = db_path.clone();
    let repository_id = stage.repository_id;
    let finish_result = run_blocking_sync(move || {
        let conn = crate::db::open_fast(&finish_path)?;
        let mut persisted = Repository::find_by_id(&conn, repository_id)?.ok_or_else(|| {
            Error::NotFound(format!(
                "Repository {repository_id} not found during sparse sync"
            ))
        })?;
        finish_remi_sync_stage(&conn, &mut persisted, stage)
    })
    .await;
    let count = match finish_result {
        Ok(count) => count,
        Err(error) => {
            let abort_path = db_path.clone();
            let _ = run_blocking_sync(move || {
                let conn = crate::db::open_fast(&abort_path)?;
                abort_remi_sync_stage(&conn, stage);
                Ok(())
            })
            .await;
            return Err(error);
        }
    };
    info!(
        "Synchronized {} packages from Remi repository {}",
        count, repo.name
    );
    Ok(count)
}

/// Fetch the canonical package map from a Remi endpoint and persist it locally.
///
/// Downloads and checksum-verifies the full map from
/// `{endpoint}/v1/canonical/map`. Persistence atomically replaces Remi-owned
/// rows while preserving non-conflicting Contract authority.
pub(super) async fn fetch_canonical_map_snapshot(endpoint: &str) -> Result<CanonicalMapSnapshot> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    debug!("Fetching canonical map from {}", url);

    let client = RepositoryClient::new()?;
    let (headers, bytes) = client.download_to_bytes_with_headers(&url).await?;
    let checksum = expected_checksum(&headers)?;
    crate::hash::verify_sha256(&bytes, &checksum).map_err(|error| Error::ChecksumMismatch {
        expected: error.expected,
        actual: error.actual,
    })?;

    parse_snapshot(&bytes).map_err(|error| {
        Error::ParseError(format!("Failed to parse canonical map from {url}: {error}"))
    })
}

pub(super) fn persist_canonical_map(conn: &Connection, map: &CanonicalMapSnapshot) -> Result<u64> {
    replace_remi_snapshot(conn, map).map(|count| count as u64)
}

pub(super) async fn fetch_and_persist_canonical_map(
    conn: &Connection,
    endpoint: &str,
) -> Result<u64> {
    let map = fetch_canonical_map_snapshot(endpoint).await?;
    persist_canonical_map(conn, &map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{
        RepositoryPackageKey, RepositoryPackageKeyStatus,
        RepositoryRequirementGroup as DbRequirementGroup,
    };
    use crate::db::testing::create_test_db;
    use crate::repository::package_relation::parse_native_relation;
    use crate::repository::remi_metadata::{
        REMI_SPARSE_SYNC_PAGE_SIZE, RemiProvide, RemiRequirement, RemiRequirementGroup,
        RemiSparsePackagePage, RemiSparseResolutionEntry, RemiSparseResolutionVersionEntry,
    };
    use std::io::{Read, Write};

    fn remi_entry_for_tests(
        name: &str,
        version: &str,
        version_scheme: VersionScheme,
    ) -> RemiSparseResolutionVersionEntry {
        RemiSparseResolutionVersionEntry {
            version: version.to_string(),
            release: None,
            architecture: Some("x86_64".to_string()),
            provides: vec![RemiProvide {
                capability: name.to_string(),
                version: Some(version.to_string()),
                version_relation: Some(
                    crate::repository::dependency_model::ProvideVersionRelation::Equal,
                ),
                kind: "package".to_string(),
                raw: Some(name.to_string()),
                version_scheme,
                architecture_qualifier:
                    crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
            }],
            requirement_groups: Vec::new(),
            size: 1024,
            metadata: None,
        }
    }

    #[test]
    fn remi_sync_row_preserves_wire_architecture() {
        let row = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "fedora-44".to_string(),
            "qemu-img".to_string(),
            remi_entry_for_tests("qemu-img", "2:10.1.0-7.fc44", VersionScheme::Rpm),
        )
        .unwrap();

        assert_eq!(row.package.architecture.as_deref(), Some("x86_64"));
    }

    #[test]
    fn remi_sync_row_rejects_non_downloadable_package_identity() {
        let mut empty_version =
            remi_entry_for_tests("qemu-img", "2:10.1.0-7.fc44", VersionScheme::Rpm);
        empty_version.version.clear();
        let error = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "fedora-44".to_string(),
            "qemu-img".to_string(),
            empty_version,
        )
        .unwrap_err();
        assert!(error.to_string().contains("empty package version"));

        let mut zero_size = remi_entry_for_tests("qemu-img", "2:10.1.0-7.fc44", VersionScheme::Rpm);
        zero_size.size = 0;
        let error = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "fedora-44".to_string(),
            "qemu-img".to_string(),
            zero_size,
        )
        .unwrap_err();
        assert!(error.to_string().contains("non-downloadable size 0"));
    }

    #[test]
    fn remi_sync_row_preserves_release_and_exact_download_url() {
        let row = remi_sync_row(
            7,
            "https://remi.example.test".to_string(),
            "fedora-44".to_string(),
            "hello".to_string(),
            {
                let mut entry = remi_entry_for_tests("hello", "1.0.0", VersionScheme::Rpm);
                entry.release = Some("2".to_string());
                entry.architecture = Some("noarch".to_string());
                entry
            },
        )
        .unwrap();

        assert_eq!(row.package.package_release, "2");
        assert_eq!(
            row.package.download_url,
            "https://remi.example.test/v1/fedora/packages/hello/download?version=1.0.0&release=2&arch=noarch"
        );
    }

    #[test]
    fn remi_sync_row_records_public_distro_and_version_scheme() {
        let row = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "ubuntu-26.04".to_string(),
            "nano".to_string(),
            {
                let mut entry = remi_entry_for_tests("nano", "8.7.1-1", VersionScheme::Debian);
                entry.architecture = Some("amd64".to_string());
                entry
            },
        )
        .unwrap();

        assert_eq!(row.package.source_profile.as_deref(), Some("ubuntu-26.04"));
        assert_eq!(row.package.version_scheme, VersionScheme::Debian);
    }

    #[test]
    fn remi_sync_row_accepts_public_profile_id_and_uses_route_slug() {
        for (public_id, route_slug) in [("fedora-44", "fedora"), ("ubuntu-26.04", "ubuntu")] {
            let scheme = if public_id == "fedora-44" {
                VersionScheme::Rpm
            } else {
                VersionScheme::Debian
            };
            let row = remi_sync_row(
                1,
                "https://remi.example.test".to_string(),
                public_id.to_string(),
                "bash".to_string(),
                remi_entry_for_tests("bash", "5.2.0", scheme),
            )
            .unwrap();

            assert_eq!(row.package.source_profile.as_deref(), Some(public_id));
            assert!(
                row.package
                    .download_url
                    .contains(&format!("/v1/{route_slug}/"))
            );
        }
    }

    #[test]
    fn remi_sync_row_rejects_route_slug_as_package_identity() {
        let error = remi_sync_row(
            1,
            "https://remi.example.test".to_string(),
            "ubuntu".to_string(),
            "bash".to_string(),
            remi_entry_for_tests("bash", "5.2.0", VersionScheme::Debian),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("unsupported Remi target: ubuntu")
        );
    }

    fn wire_relation(relation: &TypedRequirementGroup) -> RemiRequirementGroup {
        RemiRequirementGroup {
            kind: relation.kind.as_str().to_string(),
            behavior: match relation.behavior {
                ConditionalRequirementBehavior::Hard => "hard",
                ConditionalRequirementBehavior::Conditional => "conditional",
            }
            .to_string(),
            description: relation.description.clone(),
            native_text: relation.native_text.clone(),
            expression_json: serde_json::to_string(&relation.expression).unwrap(),
            clauses: relation
                .alternatives
                .iter()
                .map(|clause| RemiRequirement {
                    capability: clause.name.clone(),
                    version_constraint: clause.version_constraint.clone(),
                    kind: "package".to_string(),
                    dependency_type: "runtime".to_string(),
                    raw: clause.native_text.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn remi_wire_relation_persists_exact_obsolete_authority() {
        let (_temp, conn) = create_test_db();
        let mut repository = Repository::new("fedora".to_string(), "https://remi.test".to_string());
        repository.source_profile = Some("fedora-44".to_string());
        let repository_id = repository.insert(&conn).unwrap();
        let relation = parse_native_relation(
            RepositoryRequirementKind::Obsolete,
            VersionScheme::Rpm,
            "oldpkg < 2",
        )
        .unwrap();
        let mut entry = remi_entry_for_tests("newpkg", "2", VersionScheme::Rpm);
        entry.requirement_groups = vec![wire_relation(&relation)];
        let row = remi_sync_row(
            repository_id,
            "https://remi.test".to_string(),
            "fedora-44".to_string(),
            "newpkg".to_string(),
            entry,
        )
        .unwrap();

        append_synced_package_rows(&conn, vec![row]).unwrap();

        let package = RepositoryPackage::find_by_repository(&conn, repository_id)
            .unwrap()
            .into_iter()
            .find(|package| package.name == "newpkg")
            .unwrap();
        let stored =
            DbRequirementGroup::find_by_repository_package(&conn, package.id.unwrap()).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].kind, "obsolete");
        assert_eq!(stored[0].native_text.as_deref(), Some("oldpkg < 2"));
        assert_eq!(
            serde_json::from_str::<RepositoryRequirementExpression>(&stored[0].expression_json,)
                .unwrap(),
            relation.expression
        );
    }

    #[test]
    fn remi_wire_relation_rejects_malformed_exact_constraint() {
        let mut relation = parse_native_relation(
            RepositoryRequirementKind::Obsolete,
            VersionScheme::Rpm,
            "oldpkg < 2",
        )
        .unwrap();
        let RepositoryRequirementExpression::Atom(clause) = &mut relation.expression else {
            panic!("fixture must remain atomic");
        };
        clause.version_constraint = Some(">=".to_string());
        relation.alternatives = vec![clause.clone()];
        let mut entry = remi_entry_for_tests("newpkg", "2", VersionScheme::Rpm);
        entry.requirement_groups = vec![wire_relation(&relation)];

        let error = remi_sync_row(
            7,
            "https://remi.test".to_string(),
            "fedora-44".to_string(),
            "newpkg".to_string(),
            entry,
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid rpm constraint"));
    }

    #[tokio::test]
    async fn remi_sparse_page_fetch_retries_truncated_json() {
        use crate::repository::retry::RetryConfig;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let server_attempts = attempts.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let read = stream.read(&mut buf).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }

                let attempt = server_attempts.fetch_add(1, Ordering::SeqCst);
                let body = if attempt == 0 {
                    r#"{"distro":"fedora""#
                } else {
                    r#"{"distro":"fedora","packages":[{"name":"qemu-img","distro":"fedora","versions":[{"version":"2:10.1.0-7.fc44","architecture":"x86_64","provides":[],"requirement_groups":[],"size":1024}]}],"total":1,"page":1,"per_page":128}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let retry = RetryConfig {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter_factor: 0.0,
        };
        let client = RepositoryClient::new().unwrap();
        let page = fetch_sparse_page_with_retry(
            &client,
            &format!("http://{addr}/v1/index/fedora?include=versions"),
            &retry,
        )
        .await
        .unwrap();

        assert_eq!(page.packages[0].name, "qemu-img");
        assert_eq!(page.packages[0].versions.len(), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn remi_sparse_sync_fetches_one_complete_http_page() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = serde_json::to_string(&RemiSparsePackagePage {
            distro: "fedora".to_string(),
            packages: vec![sparse_test_entry("alpha"), sparse_test_entry("beta")],
            total: 2,
            page: 1,
            per_page: REMI_SPARSE_SYNC_PAGE_SIZE,
        })
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let read = stream.read(&mut buf).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });

        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let repo = sparse_test_repo(format!("http://{addr}"), &conn);
        let mut fetcher = RemiSparseSync::new(&repo).unwrap();
        let rows = fetcher.next_rows().await.unwrap().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(fetcher.next_rows().await.unwrap().is_none());

        let request = server.await.unwrap();
        assert!(
            request
                .starts_with("GET /v1/index/fedora?page=1&per_page=128&include=versions HTTP/1.1"),
            "unexpected sparse sync request: {request}"
        );
    }

    fn sparse_test_repo(endpoint: String, conn: &Connection) -> Repository {
        let mut repo = Repository::new("fedora-sparse".to_string(), endpoint.clone());
        repo.default_strategy = Some("remi".to_string());
        repo.default_strategy_endpoint = Some(endpoint);
        repo.source_profile = Some("fedora-44".to_string());
        repo.insert(conn).unwrap();
        repo
    }

    fn sparse_test_entry(name: &str) -> RemiSparseResolutionEntry {
        RemiSparseResolutionEntry {
            name: name.to_string(),
            distro: "fedora".to_string(),
            versions: vec![remi_entry_for_tests(name, "1.0-1.fc44", VersionScheme::Rpm)],
        }
    }

    fn consume_synthetic_page(
        fetcher: &mut RemiSparseSync,
        package_count: usize,
    ) -> Result<Vec<SyncedPackageRow>> {
        let page = fetcher.next_page;
        let start = (page - 1) * REMI_SPARSE_SYNC_PAGE_SIZE;
        let end = start
            .saturating_add(REMI_SPARSE_SYNC_PAGE_SIZE)
            .min(package_count);
        let entries = (start..end)
            .map(|index| format!("pkg-{index:08}"))
            .map(|name| sparse_test_entry(&name))
            .collect();
        fetcher.consume_page(RemiSparsePackagePage {
            distro: "fedora".to_string(),
            packages: entries,
            total: package_count,
            page,
            per_page: REMI_SPARSE_SYNC_PAGE_SIZE,
        })
    }

    #[test]
    fn sparse_wire_contract_round_trips_complete_resolution_page() {
        let emitted = serde_json::json!({
            "distro": "fedora",
            "packages": [{
                "name": "hello",
                "distro": "fedora",
                "versions": [{
                    "version": "1.0",
                    "release": "2",
                    "provides": [{
                        "capability": "hello",
                        "version": "1.0",
                        "version_relation": "equal",
                        "kind": "package",
                        "raw": "hello = 1.0",
                        "version_scheme": "rpm",
                        "architecture_qualifier": {"kind": "implicit"}
                    }],
                    "requirement_groups": [{
                        "kind": "depends",
                        "behavior": "hard",
                        "description": null,
                        "native_text": "glibc >= 2.39",
                        "expression_json": "{\"kind\":\"atom\",\"clause\":{\"name\":\"glibc\",\"version_constraint\":\">= 2.39\",\"capability_kind\":null,\"native_text\":null}}",
                        "clauses": [{
                            "capability": "glibc",
                            "version_constraint": ">= 2.39",
                            "kind": "package",
                            "dependency_type": "runtime",
                            "raw": "glibc >= 2.39"
                        }]
                    }],
                    "architecture": "x86_64",
                    "size": 4096,
                    "metadata": {
                        "security_advisory": {
                            "id": "FEDORA-2026-0001",
                            "source": "remi",
                            "source_trust": "trusted",
                            "fixed_version": "1.0"
                        }
                    }
                }]
            }],
            "total": 1,
            "page": 1,
            "per_page": REMI_SPARSE_SYNC_PAGE_SIZE
        });
        let page: RemiSparsePackagePage = serde_json::from_value(emitted).unwrap();
        assert_eq!(page.packages[0].versions[0].release.as_deref(), Some("2"));
        assert_eq!(
            page.packages[0].versions[0].requirement_groups[0].clauses[0].capability,
            "glibc"
        );
        assert_eq!(
            page.packages[0].versions[0].metadata.as_ref().unwrap()["security_advisory"]["id"],
            "FEDORA-2026-0001"
        );
    }

    #[test]
    fn sparse_sync_atomically_replaces_rows_and_cleans_staging_repository() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let mut repo = sparse_test_repo("https://remi.test".to_string(), &conn);
        RepositoryPackageKey::replace_for_repository(
            &conn,
            repo.id.unwrap(),
            &[RepositoryPackageKey {
                repository_id: repo.id.unwrap(),
                public_key: "release-tracked-key".to_string(),
                key_id: Some("targets".to_string()),
                status: RepositoryPackageKeyStatus::Active,
                synced_at: None,
            }],
        )
        .unwrap();
        let mut old = RepositoryPackage::new(
            repo.id.unwrap(),
            "retired".to_string(),
            "0".to_string(),
            VersionScheme::Rpm,
            "old".to_string(),
            1,
            "https://old.invalid".to_string(),
        );
        old.insert(&conn).unwrap();

        let mut fetcher = RemiSparseSync::new(&repo).unwrap();
        let stage = begin_remi_sync_stage(&conn, &repo).unwrap();
        while fetcher.names_seen < 300 {
            let rows = consume_synthetic_page(&mut fetcher, 300).unwrap();
            append_remi_sync_page(&conn, stage, rows).unwrap();
        }
        let count = finish_remi_sync_stage(&conn, &mut repo, stage).unwrap();

        assert_eq!(count, 300);
        let packages = RepositoryPackage::find_by_repository(&conn, repo.id.unwrap()).unwrap();
        assert_eq!(packages.len(), 300);
        assert!(!packages.iter().any(|package| package.name == "retired"));
        assert_eq!(
            RepositoryPackageKey::trusted_keys_for_repository(&conn, repo.id.unwrap()).unwrap(),
            vec!["release-tracked-key".to_string()]
        );
        assert_eq!(Repository::list_all(&conn).unwrap().len(), 1);
    }

    #[test]
    fn sparse_sync_failure_preserves_previous_snapshot() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let repo = sparse_test_repo("https://remi.test".to_string(), &conn);
        let mut old = RepositoryPackage::new(
            repo.id.unwrap(),
            "still-current".to_string(),
            "1".to_string(),
            VersionScheme::Rpm,
            "old".to_string(),
            1,
            "https://old.invalid".to_string(),
        );
        old.insert(&conn).unwrap();

        let mut fetcher = RemiSparseSync::new(&repo).unwrap();
        let stage = begin_remi_sync_stage(&conn, &repo).unwrap();
        let mut rows = consume_synthetic_page(&mut fetcher, 2).unwrap();
        rows[1].package.name = rows[0].package.name.clone();
        let error = append_remi_sync_page(&conn, stage, rows).unwrap_err();
        abort_remi_sync_stage(&conn, stage);

        assert!(error.to_string().contains("UNIQUE constraint failed"));
        let packages = RepositoryPackage::find_by_repository(&conn, repo.id.unwrap()).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "still-current");
        assert_eq!(Repository::list_all(&conn).unwrap().len(), 1);
    }

    #[test]
    fn sparse_sync_peak_rss_is_independent_of_distribution_package_count() {
        const CHILD_ENV: &str = "CONARY_ISSUE_156_PACKAGE_COUNT";
        const TEST_NAME: &str = "repository::sync::remi::tests::sparse_sync_peak_rss_is_independent_of_distribution_package_count";
        if std::env::var_os(CHILD_ENV).is_none() {
            const FEDORA_SPARSE_NAME_COUNT: usize = 67_430;
            let package_counts = [512usize, FEDORA_SPARSE_NAME_COUNT];
            let mut measurements = Vec::new();
            for package_count in package_counts {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", TEST_NAME, "--nocapture"])
                    .env(CHILD_ENV, package_count.to_string())
                    .output()
                    .unwrap();
                std::io::stdout().write_all(&output.stdout).unwrap();
                std::io::stderr().write_all(&output.stderr).unwrap();
                assert!(output.status.success(), "sparse RSS child failed");
                let stdout = String::from_utf8_lossy(&output.stdout);
                let high_water_kib = stdout
                    .lines()
                    .find_map(|line| line.strip_prefix("ISSUE156_VM_HWM_KIB="))
                    .and_then(|value| value.parse::<u64>().ok())
                    .expect("sparse RSS child did not report VmHWM");
                measurements.push(high_water_kib);
            }
            let growth_kib = measurements[1].saturating_sub(measurements[0]);
            assert!(
                growth_kib < 32 * 1024,
                "VmHWM grew by {growth_kib} KiB when package count grew from {} to {}",
                package_counts[0],
                package_counts[1]
            );
            return;
        }

        let package_count = std::env::var(CHILD_ENV).unwrap().parse::<usize>().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("sparse-sync.db");
        let conn = Connection::open(&db_path).unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let mut repo = sparse_test_repo("https://remi.test".to_string(), &conn);
        let mut fetcher = RemiSparseSync::new(&repo).unwrap();
        let stage = begin_remi_sync_stage(&conn, &repo).unwrap();
        while fetcher.names_seen < package_count {
            let rows = consume_synthetic_page(&mut fetcher, package_count).unwrap();
            assert!(rows.len() <= REMI_SPARSE_SYNC_PAGE_SIZE);
            append_remi_sync_page(&conn, stage, rows).unwrap();
        }
        let count = finish_remi_sync_stage(&conn, &mut repo, stage).unwrap();
        assert_eq!(count, package_count);
        drop(conn);

        let high_water_kib = vm_hwm_kib().unwrap();
        println!("ISSUE156_PACKAGE_COUNT={package_count}");
        println!("ISSUE156_VM_HWM_KIB={high_water_kib}");
        assert!(
            high_water_kib < 256 * 1024,
            "VmHWM {high_water_kib} KiB exceeded fixed 262144 KiB bound"
        );
    }

    fn vm_hwm_kib() -> Option<u64> {
        let mut status = String::new();
        std::fs::File::open("/proc/self/status")
            .ok()?
            .read_to_string(&mut status)
            .ok()?;
        status.lines().find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
    }
}
