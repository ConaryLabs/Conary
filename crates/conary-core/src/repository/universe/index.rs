// crates/conary-core/src/repository/universe/index.rs

//! Bounded construction of the private immutable client resolution index.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::canonical::CanonicalMapEntry;
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogBindingV1, CatalogReader, CatalogScopeV1};

use super::RemiUniverseManifestV1;

#[path = "index/replay.rs"]
mod replay;

const CLIENT_INDEX_SCHEMA: &str = r#"
PRAGMA page_size = 4096;
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;

CREATE TABLE universe_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    manifest_sha256 TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    canonical_map_sha256 TEXT NOT NULL,
    package_count INTEGER NOT NULL CHECK(package_count >= 0),
    provide_count INTEGER NOT NULL CHECK(provide_count >= 0),
    requirement_group_count INTEGER NOT NULL CHECK(requirement_group_count >= 0),
    requirement_count INTEGER NOT NULL CHECK(requirement_count >= 0)
) STRICT;

CREATE TABLE repository_packages (
    id INTEGER PRIMARY KEY CHECK(id < 0),
    repository_id INTEGER NOT NULL CHECK(repository_id > 0),
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    package_release TEXT NOT NULL,
    architecture TEXT,
    debian_multi_arch TEXT,
    description TEXT,
    checksum TEXT NOT NULL,
    size INTEGER NOT NULL CHECK(size >= 0),
    download_url TEXT NOT NULL,
    metadata TEXT,
    synced_at TEXT NOT NULL,
    is_security_update INTEGER NOT NULL CHECK(is_security_update IN (0, 1)),
    severity TEXT,
    cve_ids TEXT,
    advisory_id TEXT,
    advisory_url TEXT,
    source_profile TEXT NOT NULL,
    version_scheme TEXT NOT NULL,
    canonical_id INTEGER,
    UNIQUE(repository_id, name, version, package_release, architecture)
) STRICT;
CREATE INDEX repository_packages_name ON repository_packages(name, version, id);
CREATE INDEX repository_packages_repo ON repository_packages(repository_id, name, id);
CREATE INDEX repository_packages_canonical ON repository_packages(canonical_id, id);

CREATE TABLE repository_provides (
    id INTEGER PRIMARY KEY CHECK(id < 0),
    repository_package_id INTEGER NOT NULL REFERENCES repository_packages(id) ON DELETE CASCADE,
    capability TEXT NOT NULL,
    version TEXT,
    version_relation TEXT,
    kind TEXT NOT NULL,
    raw TEXT,
    version_scheme TEXT NOT NULL,
    architecture_qualifier_kind TEXT NOT NULL,
    architecture TEXT,
    provenance TEXT NOT NULL,
    CHECK((version IS NULL) = (version_relation IS NULL))
) STRICT;
CREATE INDEX repository_provides_package ON repository_provides(repository_package_id, id);
CREATE INDEX repository_provides_capability ON repository_provides(capability, kind, id);
CREATE INDEX repository_provides_raw ON repository_provides(raw, id)
    WHERE raw IS NOT NULL AND raw != '';

CREATE TABLE repository_requirement_groups (
    id INTEGER PRIMARY KEY CHECK(id < 0),
    repository_package_id INTEGER NOT NULL REFERENCES repository_packages(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    behavior TEXT NOT NULL,
    description TEXT,
    native_text TEXT,
    expression_json TEXT NOT NULL
) STRICT;
CREATE INDEX repository_requirement_groups_package
    ON repository_requirement_groups(repository_package_id, id);

CREATE TABLE repository_requirements (
    id INTEGER PRIMARY KEY CHECK(id < 0),
    repository_package_id INTEGER NOT NULL REFERENCES repository_packages(id) ON DELETE CASCADE,
    group_id INTEGER NOT NULL REFERENCES repository_requirement_groups(id) ON DELETE CASCADE,
    capability TEXT NOT NULL,
    version_constraint TEXT,
    kind TEXT NOT NULL,
    dependency_type TEXT NOT NULL,
    raw TEXT
) STRICT;
CREATE INDEX repository_requirements_package ON repository_requirements(repository_package_id, id);
CREATE INDEX repository_requirements_capability ON repository_requirements(capability, kind, id);
CREATE INDEX repository_requirements_group ON repository_requirements(group_id, id);

CREATE TABLE canonical_packages (
    id INTEGER PRIMARY KEY CHECK(id < 0),
    name TEXT NOT NULL UNIQUE,
    appstream_id TEXT,
    description TEXT,
    kind TEXT NOT NULL,
    category TEXT
) STRICT;

CREATE TABLE package_implementations (
    id INTEGER PRIMARY KEY CHECK(id < 0),
    canonical_id INTEGER NOT NULL,
    distro TEXT NOT NULL,
    distro_name TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source = 'remi'),
    UNIQUE(distro, distro_name),
    UNIQUE(canonical_id, distro)
) STRICT;
CREATE INDEX package_implementations_canonical ON package_implementations(canonical_id, id);

CREATE TABLE canonical_resolution (
    distro TEXT NOT NULL,
    distro_name TEXT NOT NULL,
    canonical_id INTEGER NOT NULL,
    PRIMARY KEY(distro, distro_name)
) STRICT;
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientUniverseIndex {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub package_count: u64,
}

#[derive(Default)]
struct NegativeIds {
    canonical: i64,
    implementation: i64,
}

impl NegativeIds {
    fn next(slot: &mut i64, label: &str) -> Result<i64> {
        *slot = slot.checked_sub(1).ok_or_else(|| {
            Error::InternalError(format!("client universe {label} ID space exhausted"))
        })?;
        Ok(*slot)
    }
}

pub(crate) fn build_client_universe_index(
    operational: &Connection,
    manifest: &RemiUniverseManifestV1,
    canonical_map_path: &Path,
    catalog_objects: &BTreeMap<String, PathBuf>,
    indices_root: &Path,
) -> Result<ClientUniverseIndex> {
    manifest.validate()?;
    fs::create_dir_all(indices_root)?;
    fs::set_permissions(indices_root, fs::Permissions::from_mode(0o700))?;
    let manifest_sha256 = manifest.manifest_sha256()?;
    let destination = indices_root.join(format!("{manifest_sha256}.sqlite"));
    if destination.exists() {
        return inspect_existing_index(&destination, &manifest_sha256);
    }

    let candidate = tempfile::Builder::new()
        .prefix("universe-index-")
        .tempfile_in(indices_root)?;
    fs::set_permissions(candidate.path(), fs::Permissions::from_mode(0o600))?;
    let mut index = Connection::open(candidate.path())?;
    index.execute_batch(CLIENT_INDEX_SCHEMA)?;
    index.pragma_update(
        None,
        "application_id",
        crate::db::remi_universe::client_index_application_id(),
    )?;
    let repositories = remi_profile_repositories(operational, manifest)?;
    let mut catalogs = Vec::with_capacity(manifest.profiles.len());
    for (ordinal, profile) in manifest.profiles.iter().enumerate() {
        let object = catalog_objects
            .get(&profile.catalog.sha256)
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "downloaded universe omits catalog object {}",
                    profile.catalog.sha256
                ))
            })?;
        let binding = CatalogBindingV1 {
            scope: CatalogScopeV1::Profile {
                profile: profile.revision.profile.clone(),
            },
            artifact: profile.revision.catalog.clone(),
            logical_digest_sha256: profile.revision.logical_digest_sha256.clone(),
            counts: profile.revision.counts,
        };
        let reader = CatalogReader::open_verified_signed_artifact(object, &binding)?;
        let alias = format!("universe_catalog_{ordinal}");
        replay::attach_catalog(&index, &alias, reader.path())?;
        catalogs.push((
            alias,
            repositories.get(&profile.revision.profile).copied(),
            reader,
        ));
    }

    let tx = index.transaction()?;
    let mut ids = NegativeIds::default();
    crate::canonical::stream::for_each_entry(
        canonical_map_path,
        manifest.canonical_map.revision,
        manifest.canonical_map.entry_count,
        &mut |entry| insert_canonical_entry(operational, &tx, entry, &mut ids),
    )?;
    replay::prepare(&tx)?;
    let mut offsets = replay::RowOffsets::default();
    let mut counts = replay::ReplayCounts::default();
    for (alias, repository_id, reader) in &catalogs {
        let Some(repository_id) = repository_id else {
            continue;
        };
        counts.add(replay::copy_catalog(
            &tx,
            alias,
            *repository_id,
            &manifest.generated_at.to_rfc3339(),
            reader.binding().counts,
            &mut offsets,
        )?)?;
    }
    tx.execute(
        "INSERT INTO universe_metadata (
             singleton, schema_version, manifest_sha256, sequence,
             canonical_map_sha256, package_count, provide_count,
             requirement_group_count, requirement_count
         ) VALUES (1, 1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            &manifest_sha256,
            checked_i64(manifest.sequence, "universe sequence")?,
            &manifest.canonical_map.sha256,
            checked_i64(counts.packages, "package count")?,
            checked_i64(counts.provides, "provide count")?,
            checked_i64(counts.requirement_groups, "requirement-group count")?,
            checked_i64(counts.requirements, "requirement count")?,
        ],
    )?;
    tx.commit()?;
    for (alias, _, _) in &catalogs {
        replay::detach_catalog(&index, alias)?;
    }
    index.execute_batch("PRAGMA optimize; VACUUM;")?;
    let integrity: String = index.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(Error::InitError(format!(
            "client universe index failed integrity_check: {integrity}"
        )));
    }
    drop(index);
    candidate.as_file().sync_all()?;
    fs::set_permissions(candidate.path(), fs::Permissions::from_mode(0o400))?;
    let temporary = candidate.into_temp_path();
    temporary
        .persist(&destination)
        .map_err(|error| error.error)?;
    if let Some(parent) = destination.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    inspect_existing_index(&destination, &manifest_sha256)
}

fn remi_profile_repositories(
    operational: &Connection,
    manifest: &RemiUniverseManifestV1,
) -> Result<BTreeMap<String, i64>> {
    let mut repositories = BTreeMap::new();
    let endpoint = canonical_endpoint(operational)?;
    for profile in &manifest.profiles {
        let mut statement = operational.prepare(
            "SELECT id FROM repositories
             WHERE enabled = 1 AND default_strategy = 'remi'
               AND default_strategy_endpoint = ?1 AND source_profile = ?2
             ORDER BY id",
        )?;
        let ids = statement
            .query_map(params![&endpoint, &profile.revision.profile], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match ids.as_slice() {
            [] => {}
            [repository_id] => {
                repositories.insert(profile.revision.profile.clone(), *repository_id);
            }
            _ => {
                return Err(Error::ConflictError(format!(
                    "multiple enabled Remi repositories claim profile '{}'",
                    profile.revision.profile
                )));
            }
        }
    }
    let configured_profiles = operational
        .prepare(
            "SELECT source_profile FROM repositories
             WHERE enabled = 1 AND default_strategy = 'remi'
               AND default_strategy_endpoint = ?1
             ORDER BY source_profile, id",
        )?
        .query_map([&endpoint], |row| row.get::<_, Option<String>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for profile in configured_profiles {
        let profile = profile.ok_or_else(|| {
            Error::ConfigError("enabled Remi repository has no exact source profile".to_string())
        })?;
        if !repositories.contains_key(&profile) {
            return Err(Error::ConflictError(format!(
                "signed Remi universe omits configured source profile '{profile}'"
            )));
        }
    }
    Ok(repositories)
}

fn canonical_endpoint(operational: &Connection) -> Result<String> {
    let endpoints = operational
        .prepare(
            "SELECT default_strategy_endpoint FROM repositories
             WHERE enabled = 1 AND default_strategy = 'remi'
               AND default_strategy_endpoint IS NOT NULL
             GROUP BY default_strategy_endpoint ORDER BY default_strategy_endpoint",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match endpoints.as_slice() {
        [endpoint] => Ok(endpoint.clone()),
        [] => Err(Error::ConfigError(
            "client universe has no enabled Remi endpoint".to_string(),
        )),
        _ => Err(Error::ConflictError(
            "one client universe cannot activate across multiple Remi endpoints".to_string(),
        )),
    }
}

fn insert_canonical_entry(
    operational: &Connection,
    index: &Connection,
    entry: CanonicalMapEntry,
    ids: &mut NegativeIds,
) -> Result<()> {
    let main = operational
        .query_row(
            "SELECT id, kind, category FROM canonical_packages WHERE name = ?1",
            [&entry.canonical],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let canonical_id = if let Some((id, kind, category)) = main {
        if kind != entry.kind || category != entry.category {
            return Err(Error::ConflictError(format!(
                "universe canonical '{}' conflicts with local contract authority",
                entry.canonical
            )));
        }
        id
    } else {
        let id = NegativeIds::next(&mut ids.canonical, "canonical package")?;
        index.execute(
            "INSERT INTO canonical_packages (
                 id, name, appstream_id, description, kind, category
             ) VALUES (?1, ?2, NULL, NULL, ?3, ?4)",
            params![id, &entry.canonical, &entry.kind, &entry.category],
        )?;
        id
    };
    for (profile, name) in entry.implementations {
        let exact_contract = operational
            .query_row(
                "SELECT implementation.canonical_id
                 FROM package_implementations implementation
                 WHERE implementation.distro = ?1
                   AND implementation.distro_name = ?2
                   AND implementation.source = 'contract'",
                params![&profile, &name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let contract_for_canonical = operational
            .query_row(
                "SELECT implementation.distro_name
                 FROM package_implementations implementation
                 WHERE implementation.canonical_id = ?1
                   AND implementation.distro = ?2
                   AND implementation.source = 'contract'",
                params![canonical_id, &profile],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match (exact_contract, contract_for_canonical) {
            (Some(contract_id), Some(contract_name))
                if contract_id == canonical_id && contract_name == name => {}
            (None, None) => {
                let id = NegativeIds::next(&mut ids.implementation, "canonical implementation")?;
                index.execute(
                    "INSERT INTO package_implementations (
                         id, canonical_id, distro, distro_name, source
                     ) VALUES (?1, ?2, ?3, ?4, 'remi')",
                    params![id, canonical_id, &profile, &name],
                )?;
            }
            _ => {
                return Err(Error::ConflictError(format!(
                    "universe implementation '{profile}:{name}' conflicts with local contract authority"
                )));
            }
        }
        index.execute(
            "INSERT INTO canonical_resolution (distro, distro_name, canonical_id)
             VALUES (?1, ?2, ?3)",
            params![profile, name, canonical_id],
        )?;
    }
    Ok(())
}

fn checked_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        Error::ConfigError(format!(
            "client universe {label} exceeds SQLite integer range"
        ))
    })
}

fn inspect_existing_index(path: &Path, manifest_sha256: &str) -> Result<ClientUniverseIndex> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "client universe index {} must be a regular file",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o277 != 0 {
        return Err(Error::InvalidPath(format!(
            "client universe index {} must be immutable and private",
            path.display()
        )));
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let application_id: i64 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if application_id != crate::db::remi_universe::client_index_application_id() {
        return Err(Error::ConfigError(
            "client universe index has the wrong application ID".to_string(),
        ));
    }
    let (stored_manifest, package_count) = connection.query_row(
        "SELECT manifest_sha256, package_count FROM universe_metadata WHERE singleton = 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    if stored_manifest != manifest_sha256 {
        return Err(Error::ConflictError(
            "client universe index path disagrees with its manifest".to_string(),
        ));
    }
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let sha256 = crate::hash::sha256_reader_hex(&mut file)?;
    Ok(ClientUniverseIndex {
        path: path.canonicalize()?,
        sha256,
        size: metadata.len(),
        package_count: u64::try_from(package_count).map_err(|_| {
            Error::ConfigError("client universe index has a negative package count".to_string())
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{CanonicalMapEntry, CanonicalMapSnapshot};
    use crate::db::models::{Repository, RepositoryPackage, RepositoryProvide};
    use crate::repository::catalog::{
        CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1,
        CatalogPackageRecordV1, CatalogProvideRecordV1, CatalogRequirementAtomV1,
        CatalogRequirementGroupV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V1,
        ProfileRevisionV1, ProfileSourceMemberV1, SourceStreamKindV1, SourceStreamV1,
        write_catalog_candidate,
    };
    use crate::repository::dependency_model::{
        CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
        RepositoryRequirementClause, RepositoryRequirementExpression,
    };
    use crate::repository::universe::{
        REMI_UNIVERSE_SCHEMA_V1, RemiUniverseCanonicalMapObjectV1, RemiUniverseCatalogObjectV1,
        RemiUniverseProfileV1,
    };
    use crate::repository::versioning::VersionScheme;
    use crate::resolver::PackageIdentity;
    use std::collections::BTreeMap;

    const ENDPOINT: &str = "https://remi.example.test";
    const PROFILE: &str = "fedora-44";

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn package(version: &str) -> CatalogPackageRecordV1 {
        CatalogPackageRecordV1 {
            package_key_sha256: String::new(),
            origin: CatalogPackageOriginV1::Profile {
                member_ordinal: 0,
                source_identity: "fedora-project".to_string(),
                repository_identity: "everything".to_string(),
                source_snapshot_sha256: digest('1'),
            },
            source_profile: PROFILE.to_string(),
            name: "demo".to_string(),
            version: version.to_string(),
            package_release: "1.fc44".to_string(),
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            description: Some("signed universe fixture".to_string()),
            checksum: digest('2'),
            size: 4096,
            download_url: "https://remi.example.test/demo.rpm".to_string(),
            metadata: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            version_scheme: VersionScheme::Rpm,
            provides: vec![CatalogProvideRecordV1 {
                capability: "demo".to_string(),
                version: Some(version.to_string()),
                version_relation: Some(ProvideVersionRelation::Equal),
                kind: "package".to_string(),
                raw: None,
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                provenance: CapabilityProvenance::ExactIdentity,
            }],
            requirement_groups: Vec::new(),
        }
    }

    fn build_index(
        operational: &Connection,
        root: &Path,
        version: &str,
        sequence: u64,
    ) -> (RemiUniverseManifestV1, ClientUniverseIndex) {
        let catalog_path = root.join(format!("catalog-{sequence}.sqlite"));
        let content = CatalogContentV1::new(
            CatalogScopeV1::Profile {
                profile: PROFILE.to_string(),
            },
            vec![CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: 0,
                source_identity: "fedora-project".to_string(),
                repository_identity: "everything".to_string(),
                source_snapshot_sha256: digest('1'),
            }],
            vec![package(version)],
        )
        .unwrap();
        let binding = write_catalog_candidate(&catalog_path, &content).unwrap();
        let revision = ProfileRevisionV1 {
            schema_version: PROFILE_REVISION_SCHEMA_V1,
            profile: PROFILE.to_string(),
            projection_version: 1,
            members: vec![ProfileSourceMemberV1 {
                ordinal: 0,
                source_identity: "fedora-project".to_string(),
                repository_identity: "everything".to_string(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "44".to_string(),
                },
                priority: 100,
                required: true,
                source_snapshot_sha256: digest('1'),
            }],
            catalog: binding.artifact.clone(),
            logical_digest_sha256: binding.logical_digest_sha256.clone(),
            counts: binding.counts,
        };
        let canonical_map = CanonicalMapSnapshot {
            schema_version: crate::canonical::CANONICAL_MAP_SCHEMA_VERSION,
            revision: 1,
            generated_at: Some("2026-08-22T12:00:00Z".to_string()),
            entries: vec![CanonicalMapEntry {
                canonical: "demo-app".to_string(),
                kind: "package".to_string(),
                category: None,
                implementations: BTreeMap::from([(PROFILE.to_string(), "demo".to_string())]),
            }],
        };
        let canonical_bytes = crate::json::canonical_json(&canonical_map).unwrap();
        let canonical_path = root.join(format!("canonical-{sequence}.json"));
        fs::write(&canonical_path, &canonical_bytes).unwrap();
        let generated_at = "2026-08-22T12:00:00Z".parse().unwrap();
        let manifest = RemiUniverseManifestV1 {
            schema_version: REMI_UNIVERSE_SCHEMA_V1,
            sequence,
            metadata_root_sha256: digest('3'),
            generated_at,
            expires_at: generated_at + chrono::Duration::days(7),
            profiles: vec![RemiUniverseProfileV1 {
                ordinal: 0,
                profile_revision_sha256: revision.manifest_sha256().unwrap(),
                catalog: RemiUniverseCatalogObjectV1 {
                    schema_version: CATALOG_CONTENT_SCHEMA_V1,
                    sha256: binding.artifact.sha256.clone(),
                    size: binding.artifact.size,
                    logical_digest_sha256: binding.logical_digest_sha256,
                },
                revision,
            }],
            canonical_map: RemiUniverseCanonicalMapObjectV1 {
                schema_version: crate::canonical::CANONICAL_MAP_SCHEMA_VERSION,
                sha256: crate::hash::sha256(&canonical_bytes),
                size: canonical_bytes.len() as u64,
                revision: canonical_map.revision,
                entry_count: canonical_map.entries.len() as u64,
            },
        };
        let index = build_client_universe_index(
            operational,
            &manifest,
            &canonical_path,
            &BTreeMap::from([(binding.artifact.sha256, catalog_path)]),
            &root.join("remi-universes/indices"),
        )
        .unwrap();
        (manifest, index)
    }

    fn activate(conn: &Connection, manifest: &RemiUniverseManifestV1, index: &ClientUniverseIndex) {
        conn.execute(
            "INSERT OR IGNORE INTO remi_client_universe_trust (
                 endpoint, trusted_root_sha256, trusted_root_json, root_version, fencing_epoch
             ) VALUES (?1, ?2, '{}', 1, ?3)",
            params![ENDPOINT, digest('3'), manifest.sequence as i64],
        )
        .unwrap();
        conn.execute(
            "UPDATE remi_client_universe_trust SET fencing_epoch = ?1 WHERE endpoint = ?2",
            params![manifest.sequence as i64, ENDPOINT],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remi_client_universe_revisions (
                 endpoint, manifest_sha256, sequence, manifest_json, index_sha256,
                 index_size, index_path, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ENDPOINT,
                manifest.manifest_sha256().unwrap(),
                manifest.sequence as i64,
                serde_json::to_string(manifest).unwrap(),
                &index.sha256,
                index.size as i64,
                index.path.to_string_lossy(),
                manifest.generated_at.timestamp(),
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remi_active_client_universe (
                 singleton, endpoint, manifest_sha256, sequence, fencing_epoch, activated_at
             ) VALUES (1, ?1, ?2, ?3, ?3, ?4)
             ON CONFLICT(singleton) DO UPDATE SET
                 endpoint = excluded.endpoint,
                 manifest_sha256 = excluded.manifest_sha256,
                 sequence = excluded.sequence,
                 fencing_epoch = excluded.fencing_epoch,
                 activated_at = excluded.activated_at",
            params![
                ENDPOINT,
                manifest.manifest_sha256().unwrap(),
                manifest.sequence as i64,
                manifest.generated_at.timestamp(),
            ],
        )
        .unwrap();
    }

    #[test]
    fn immutable_index_is_the_resolution_authority_and_readers_pin_activation() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("conary.db");
        crate::db::init(&db_path).unwrap();
        let operational = crate::db::open(&db_path).unwrap();
        let mut repository = Repository::new("remi-fedora".to_string(), ENDPOINT.to_string());
        repository.default_strategy = Some("remi".to_string());
        repository.default_strategy_endpoint = Some(ENDPOINT.to_string());
        repository.source_profile = Some(PROFILE.to_string());
        repository.insert(&operational).unwrap();

        let universe_root = root.path().join("remi-universes");
        fs::create_dir(&universe_root).unwrap();
        fs::set_permissions(&universe_root, fs::Permissions::from_mode(0o700)).unwrap();
        let (first_manifest, first_index) = build_index(&operational, root.path(), "1.0", 1);
        activate(&operational, &first_manifest, &first_index);
        drop(operational);

        let pinned = crate::db::open(&db_path).unwrap();
        assert_eq!(
            RepositoryPackage::find_by_name(&pinned, "demo").unwrap()[0].version,
            "1.0"
        );
        assert_eq!(
            RepositoryProvide::find_by_capability(&pinned, "demo")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            PackageIdentity::find_all_by_name(&pinned, "demo")
                .unwrap()
                .len(),
            1
        );

        let writer = crate::db::open_fast(&db_path).unwrap();
        let (second_manifest, second_index) = build_index(&writer, root.path(), "2.0", 2);
        activate(&writer, &second_manifest, &second_index);
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(writer);

        fs::remove_file(&first_index.path).unwrap();
        assert_eq!(
            RepositoryPackage::find_by_name(&pinned, "demo").unwrap()[0].version,
            "1.0"
        );
        let current = crate::db::open(&db_path).unwrap();
        assert_eq!(
            RepositoryPackage::find_by_name(&current, "demo").unwrap()[0].version,
            "2.0"
        );
        let canonical = crate::db::models::CanonicalPackage::find_by_name(&current, "demo-app")
            .unwrap()
            .unwrap();
        assert!(canonical.id.unwrap() < 0);

        let mut native = Repository::new(
            "native-fedora".to_string(),
            "https://mirror.example.test/fedora".to_string(),
        );
        native.source_profile = Some(PROFILE.to_string());
        let native_id = native.insert(&current).unwrap();
        let mut native_package = RepositoryPackage::new(
            native_id,
            "demo".to_string(),
            "2.0".to_string(),
            VersionScheme::Rpm,
            digest('4'),
            4096,
            "https://mirror.example.test/demo.rpm".to_string(),
        );
        let native_package_id = native_package.insert(&current).unwrap();
        assert_eq!(
            current
                .query_row(
                    "SELECT canonical_id FROM repository_packages WHERE id = ?1",
                    [native_package_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap(),
            None
        );
        assert_eq!(
            RepositoryPackage::find_by_id(&current, native_package_id)
                .unwrap()
                .unwrap()
                .canonical_id,
            canonical.id
        );
    }

    #[test]
    fn private_index_replay_has_fixed_peak_rss_across_independent_cardinality() {
        const CHILD_ENV: &str = "CONARY_SLICE4_INDEX_RSS_ROOT";
        const MARKER: &str = "SLICE4_INDEX_VM_HWM_KIB=";
        if let Some(root) = std::env::var_os(CHILD_ENV) {
            let root = PathBuf::from(root);
            let db_path = root.join("conary.db");
            let manifest: RemiUniverseManifestV1 =
                serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
            let catalog_path = root.join("cardinality.sqlite");
            let canonical_path = root.join("canonical.json");
            let catalog_sha256 = manifest.profiles[0].catalog.sha256.clone();
            let operational = crate::db::open_fast(&db_path).unwrap();
            let index = build_client_universe_index(
                &operational,
                &manifest,
                &canonical_path,
                &BTreeMap::from([(catalog_sha256, catalog_path)]),
                &root.join("indices"),
            )
            .unwrap();
            let private = Connection::open_with_flags(
                &index.path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            for (table, expected) in [
                ("repository_packages", 512_i64),
                ("repository_provides", 10_000_i64),
                ("repository_requirement_groups", 1_i64),
                ("repository_requirements", 10_000_i64),
            ] {
                let actual = private
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap();
                assert_eq!(actual, expected, "{table}");
            }
            let high_water_kib = vm_hwm_kib().unwrap();
            println!("{MARKER}{high_water_kib}");
            assert!(
                high_water_kib < 256 * 1024,
                "VmHWM {high_water_kib} KiB exceeded fixed 262144 KiB bound"
            );
            return;
        }

        let target_root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap().join("target"));
        fs::create_dir_all(&target_root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("slice4-index-rss-")
            .tempdir_in(target_root)
            .unwrap();
        let root = directory.path();
        let db_path = root.join("conary.db");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open_fast(&db_path).unwrap();
        let mut repository = Repository::new("remi-fedora".to_string(), ENDPOINT.to_string());
        repository.default_strategy = Some("remi".to_string());
        repository.default_strategy_endpoint = Some(ENDPOINT.to_string());
        repository.source_profile = Some(PROFILE.to_string());
        repository.insert(&conn).unwrap();
        drop(conn);

        let catalog_path = root.join("cardinality.sqlite");
        let scope = CatalogScopeV1::Profile {
            profile: PROFILE.to_string(),
        };
        let mut writer =
            crate::repository::catalog::CatalogCandidateWriter::create(&catalog_path, scope)
                .unwrap();
        for version in 0..512 {
            let mut package = package(&format!("{version:05}"));
            package.name = "cardinality".to_string();
            package.version = format!("{version:05}");
            package.checksum = crate::hash::sha256(package.version.as_bytes());
            package.download_url = format!("https://example.test/{version:05}.rpm");
            package.provides.clear();
            package.requirement_groups.clear();
            if version == 0 {
                package.metadata = Some(
                    serde_json::to_string(&serde_json::json!({
                        "presentation": "m".repeat(4 * 1024 * 1024)
                    }))
                    .unwrap(),
                );
                package.provides = (0..10_000)
                    .map(|ordinal| CatalogProvideRecordV1 {
                        capability: format!("generated-provide-{ordinal:05}"),
                        version: None,
                        version_relation: None,
                        kind: "package".to_string(),
                        raw: None,
                        version_scheme: VersionScheme::Rpm,
                        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                        provenance: CapabilityProvenance::AuthorDeclared,
                    })
                    .collect();
                let expression =
                    RepositoryRequirementExpression::Atom(RepositoryRequirementClause::versioned(
                        "expression-owner".to_string(),
                        format!("= {}", "7".repeat(4 * 1024 * 1024)),
                    ));
                package.requirement_groups = vec![CatalogRequirementGroupV1 {
                    kind: "depends".to_string(),
                    behavior: "hard".to_string(),
                    description: None,
                    native_text: None,
                    expression_json: serde_json::to_string(&expression).unwrap(),
                    atoms: (0..10_000)
                        .map(|ordinal| CatalogRequirementAtomV1 {
                            capability: format!("generated-requirement-{ordinal:05}"),
                            version_constraint: None,
                            kind: "package".to_string(),
                            dependency_type: "runtime".to_string(),
                            raw: None,
                        })
                        .collect(),
                }];
            }
            writer.package(package).unwrap();
        }
        let evidence = vec![CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "everything".to_string(),
            source_snapshot_sha256: digest('1'),
        }];
        let binding = writer.finish(evidence).unwrap();
        let revision = ProfileRevisionV1 {
            schema_version: PROFILE_REVISION_SCHEMA_V1,
            profile: PROFILE.to_string(),
            projection_version: 1,
            members: vec![ProfileSourceMemberV1 {
                ordinal: 0,
                source_identity: "fedora-project".to_string(),
                repository_identity: "everything".to_string(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "44".to_string(),
                },
                priority: 100,
                required: true,
                source_snapshot_sha256: digest('1'),
            }],
            catalog: binding.artifact.clone(),
            logical_digest_sha256: binding.logical_digest_sha256.clone(),
            counts: binding.counts,
        };
        let canonical = CanonicalMapSnapshot {
            schema_version: crate::canonical::CANONICAL_MAP_SCHEMA_VERSION,
            revision: 0,
            generated_at: None,
            entries: Vec::new(),
        };
        let canonical_bytes = crate::json::canonical_json(&canonical).unwrap();
        fs::write(root.join("canonical.json"), &canonical_bytes).unwrap();
        let generated_at = chrono::Utc::now();
        let manifest = RemiUniverseManifestV1 {
            schema_version: REMI_UNIVERSE_SCHEMA_V1,
            sequence: 1,
            metadata_root_sha256: digest('3'),
            generated_at,
            expires_at: generated_at + chrono::Duration::days(7),
            profiles: vec![RemiUniverseProfileV1 {
                ordinal: 0,
                profile_revision_sha256: revision.manifest_sha256().unwrap(),
                catalog: RemiUniverseCatalogObjectV1 {
                    schema_version: CATALOG_CONTENT_SCHEMA_V1,
                    sha256: binding.artifact.sha256,
                    size: binding.artifact.size,
                    logical_digest_sha256: binding.logical_digest_sha256,
                },
                revision,
            }],
            canonical_map: RemiUniverseCanonicalMapObjectV1 {
                schema_version: canonical.schema_version,
                sha256: crate::hash::sha256(&canonical_bytes),
                size: canonical_bytes.len() as u64,
                revision: canonical.revision,
                entry_count: 0,
            },
        };
        fs::write(
            root.join("manifest.json"),
            crate::json::canonical_json(&manifest).unwrap(),
        )
        .unwrap();

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repository::universe::index::tests::private_index_replay_has_fixed_peak_rss_across_independent_cardinality",
                "--nocapture",
            ])
            .env(CHILD_ENV, root)
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        std::io::Write::write_all(&mut std::io::stderr(), &output.stderr).unwrap();
        assert!(output.status.success(), "private-index RSS child failed");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(MARKER),
            "private-index RSS child did not report VmHWM"
        );
    }

    fn vm_hwm_kib() -> Option<u64> {
        let mut status = String::new();
        std::io::Read::read_to_string(
            &mut std::fs::File::open("/proc/self/status").ok()?,
            &mut status,
        )
        .ok()?;
        status.lines().find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
    }
}
