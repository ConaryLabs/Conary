// conary-core/src/repository/sync/remi.rs

use crate::db::models::{
    CanonicalPackage, PackageImplementation, Repository, RepositoryPackage, RepositoryProvide,
    RepositoryRequirement, RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryRequirementExpression,
    RepositoryRequirementGroup as TypedRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::metadata::PackageSecurityAdvisoryMetadata;
use crate::repository::package_relation::validate_native_relation;
use crate::repository::retry::RetryConfig;
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use super::apply_trusted_package_security_advisory;
use super::native::persist_native_sync_rows;
use super::types::{
    CanonicalMapSnapshot, RemiMetadataResponse, RemiPackageEntry, SyncedPackageRow,
};

pub(super) fn remi_sync_row(
    repo_id: i64,
    endpoint: String,
    distro: String,
    entry: RemiPackageEntry,
) -> Result<SyncedPackageRow> {
    let RemiPackageEntry {
        name,
        version,
        release,
        converted: _,
        architecture,
        provides: wire_provides,
        requirement_groups: wire_requirement_groups,
        metadata,
    } = entry;
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
        0,
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

    package.distro = Some(public_profile_id.to_string());

    if !wire_provides
        .iter()
        .any(|provide| provide.kind == "package" && provide.capability == name)
    {
        return Err(Error::ConfigError(format!(
            "Remi metadata for '{name}' has no normalized package self-provide"
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
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut requirement_groups = Vec::with_capacity(wire_requirement_groups.len());
    let mut requirement_group_clauses = Vec::with_capacity(wire_requirement_groups.len());
    for (group_index, wire_group) in wire_requirement_groups.into_iter().enumerate() {
        let group_token =
            i64::try_from(group_index + 1).expect("Remi requirement group count cannot exceed i64");
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

/// Synchronize repository directly from a Remi metadata API
///
/// For repos with `default_strategy = "remi"`, fetches the package index from
/// the Remi server's `/v1/{distro}/metadata` endpoint instead of parsing
/// traditional repo formats (repomd.xml, Packages, etc.).
pub(super) async fn fetch_remi_sync_rows(repo: &Repository) -> Result<Vec<SyncedPackageRow>> {
    let configured_target = repo.default_strategy_distro.as_deref().ok_or_else(|| {
        Error::ConfigError(format!(
            "Repository '{}' has strategy 'remi' but no distro configured (use --remi-distro)",
            repo.name
        ))
    })?;
    let profile = crate::repository::supported_profiles::profile_for_remi_target(configured_target)
        .ok_or_else(|| {
            Error::ConfigError(format!("unsupported Remi target: {configured_target}"))
        })?;
    let route_slug = profile.remi_route_slug();

    let endpoint = repo
        .default_strategy_endpoint
        .as_deref()
        .unwrap_or(&repo.url)
        .trim_end_matches('/');

    let metadata_url = format!("{endpoint}/v1/{route_slug}/metadata");
    info!(
        "Syncing repository {} from Remi metadata: {}",
        repo.name, metadata_url
    );

    let client = RepositoryClient::new()?;
    let response =
        fetch_remi_metadata_with_retry(&client, &metadata_url, &RetryConfig::quick()).await?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    let mut seen = HashSet::new();
    let mut synced_packages = Vec::new();
    for entry in response.packages {
        let key = (
            entry.name.clone(),
            entry.version.clone(),
            entry.release.clone(),
            entry.architecture.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        synced_packages.push(remi_sync_row(
            repo_id,
            endpoint.to_string(),
            configured_target.to_string(),
            entry,
        )?);
    }

    Ok(synced_packages)
}

pub(super) fn persist_remi_sync_rows(
    conn: &Connection,
    repo: &mut Repository,
    synced_packages: Vec<SyncedPackageRow>,
) -> Result<usize> {
    let mut repo_packages: Vec<RepositoryPackage> = synced_packages
        .iter()
        .map(|row| row.package.clone())
        .collect();
    let count = persist_native_sync_rows(conn, repo, &mut repo_packages, synced_packages)?;

    info!(
        "Synchronized {} packages from Remi repository {}",
        count, repo.name
    );
    Ok(count)
}

pub(super) async fn sync_repository_remi(
    conn: &Connection,
    repo: &mut Repository,
) -> Result<usize> {
    let synced_packages = fetch_remi_sync_rows(repo).await?;
    persist_remi_sync_rows(conn, repo, synced_packages)
}

async fn fetch_remi_metadata_with_retry(
    client: &RepositoryClient,
    metadata_url: &str,
    retry_policy: &RetryConfig,
) -> Result<RemiMetadataResponse> {
    let max_attempts = retry_policy.max_attempts.max(1);
    let mut last_error = None;

    for attempt in 1..=max_attempts {
        match fetch_remi_metadata_once(client, metadata_url).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                if attempt < max_attempts {
                    let delay = retry_policy.delay_for_attempt(attempt);
                    warn!(
                        "Remi metadata fetch attempt {}/{} failed: {}; retrying in {:?}",
                        attempt, max_attempts, error, delay
                    );
                    tokio::time::sleep(delay).await;
                }
                last_error = Some(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        Error::DownloadError(format!(
            "Failed to fetch Remi metadata from {metadata_url}: no attempts were made"
        ))
    }))
}

async fn fetch_remi_metadata_once(
    client: &RepositoryClient,
    metadata_url: &str,
) -> Result<RemiMetadataResponse> {
    let bytes = client.download_to_bytes(metadata_url).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!(
            "Failed to parse Remi metadata from {}: {}",
            metadata_url, error
        ))
    })
}

/// Fetch the canonical package map from a Remi endpoint and persist it locally.
///
/// Downloads the full canonical map from `{endpoint}/v1/canonical/map` and upserts
/// each entry into `canonical_packages` and `package_implementations`. This is
/// non-fatal: callers should log failures at debug level and continue.
pub(super) async fn fetch_canonical_map_snapshot(endpoint: &str) -> Result<CanonicalMapSnapshot> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    debug!("Fetching canonical map from {}", url);

    let client = RepositoryClient::new()?;
    let bytes = client.download_to_bytes(&url).await?;

    serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!("Failed to parse canonical map from {url}: {error}"))
    })
}

pub(super) fn persist_canonical_map(conn: &Connection, map: &CanonicalMapSnapshot) -> Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut count = 0u64;

    for entry in &map.entries {
        let mut canonical = CanonicalPackage::new(entry.canonical.clone(), "package".to_string());
        let Some(canonical_id) = canonical.insert_or_ignore(&tx)? else {
            continue;
        };

        for (distro, distro_name) in &entry.implementations {
            let mut implementation = PackageImplementation::new(
                canonical_id,
                distro.clone(),
                distro_name.clone(),
                "remi".to_string(),
            );
            implementation.insert_or_ignore(&tx)?;
            count += 1;
        }
    }

    tx.commit()?;
    Ok(count)
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
    use crate::db::models::RepositoryRequirementGroup as DbRequirementGroup;
    use crate::db::testing::create_test_db;
    use crate::repository::package_relation::parse_native_relation;
    use crate::repository::remi_metadata::{RemiProvide, RemiRequirement, RemiRequirementGroup};

    fn remi_entry_for_tests(
        name: &str,
        version: &str,
        version_scheme: VersionScheme,
    ) -> RemiPackageEntry {
        RemiPackageEntry {
            name: name.to_string(),
            version: version.to_string(),
            release: None,
            converted: false,
            architecture: Some("x86_64".to_string()),
            provides: vec![RemiProvide {
                capability: name.to_string(),
                version: Some(version.to_string()),
                kind: "package".to_string(),
                raw: Some(name.to_string()),
                version_scheme,
            }],
            requirement_groups: Vec::new(),
            metadata: None,
        }
    }

    #[test]
    fn remi_sync_row_preserves_wire_architecture() {
        let row = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "fedora-44".to_string(),
            remi_entry_for_tests("qemu-img", "2:10.1.0-7.fc44", VersionScheme::Rpm),
        )
        .unwrap();

        assert_eq!(row.package.architecture.as_deref(), Some("x86_64"));
    }

    #[test]
    fn remi_sync_row_preserves_release_and_exact_download_url() {
        let row = remi_sync_row(
            7,
            "https://remi.example.test".to_string(),
            "fedora-44".to_string(),
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
            {
                let mut entry = remi_entry_for_tests("nano", "8.7.1-1", VersionScheme::Debian);
                entry.architecture = Some("amd64".to_string());
                entry
            },
        )
        .unwrap();

        assert_eq!(row.package.distro.as_deref(), Some("ubuntu-26.04"));
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
                remi_entry_for_tests("bash", "5.2.0", scheme),
            )
            .unwrap();

            assert_eq!(row.package.distro.as_deref(), Some(public_id));
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
        repository.default_strategy_distro = Some("fedora-44".to_string());
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
            entry,
        )
        .unwrap();

        persist_remi_sync_rows(&conn, &mut repository, vec![row]).unwrap();

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
            entry,
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid rpm constraint"));
    }

    #[tokio::test]
    async fn remi_metadata_fetch_retries_truncated_json() {
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
                    r#"{"packages":[{"name":"qemu-img""#
                } else {
                    r#"{"packages":[{"name":"qemu-img","version":"2:10.1.0-7.fc44","converted":false,"architecture":"x86_64","provides":[],"requirement_groups":[]}]}"#
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
        let metadata = fetch_remi_metadata_with_retry(
            &client,
            &format!("http://{addr}/v1/fedora/metadata"),
            &retry,
        )
        .await
        .unwrap();

        assert_eq!(metadata.packages.len(), 1);
        assert_eq!(metadata.packages[0].name, "qemu-img");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }
}
