// apps/remi/src/server/universe_publish.rs

//! Construction, signing, verification, and atomic publication of one Remi universe.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use conary_core::canonical::{CanonicalMapSnapshot, validate_canonical_map_snapshot};
use conary_core::db::models::{
    RemiCatalogPhysicalAttestation, RemiCatalogResource, RemiCatalogResourceKind,
};
use conary_core::repository::catalog::{CATALOG_CONTENT_SCHEMA_V1, ProfileRevisionV2};
use conary_core::repository::universe::{
    REMI_UNIVERSE_SCHEMA_V2, RemiUniverseCanonicalMapObjectV2, RemiUniverseCatalogObjectV2,
    RemiUniverseManifestV2, RemiUniverseProfileV2, verify_remi_universe_manifest_target,
};
use conary_core::trust::verify::{
    extract_role_keys, verify_metadata_hash, verify_not_expired, verify_root, verify_signatures,
    verify_static_snapshot_consistency,
};
use conary_core::trust::{
    MetaFile, Role, Signed, SnapshotMetadata, TUF_SPEC_VERSION, TargetDescription, TargetsMetadata,
    TimestampMetadata, sign_tuf_metadata,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use tokio::sync::RwLock;

use super::ServerState;
use super::database_writer::DatabaseWriter;
use super::handlers::canonical::load_canonical_map_snapshot;
use super::signing_authority::{
    UniverseSigningRole, load_universe_role_key, load_universe_root_metadata,
};
use super::universe_validation::validate_canonical_candidate;

pub(crate) const UNIVERSE_MANIFEST_FILE: &str = "manifest.json";
pub(crate) const UNIVERSE_CANONICAL_MAP_FILE: &str = "canonical-map.json";
pub(crate) const UNIVERSE_ROOT_FILE: &str = "root.json";
pub(crate) const UNIVERSE_TARGETS_FILE: &str = "targets.json";
pub(crate) const UNIVERSE_SNAPSHOT_FILE: &str = "snapshot.json";
pub(crate) const UNIVERSE_TIMESTAMP_FILE: &str = "timestamp.json";

const UNIVERSE_FILES: [&str; 6] = [
    UNIVERSE_CANONICAL_MAP_FILE,
    UNIVERSE_MANIFEST_FILE,
    UNIVERSE_ROOT_FILE,
    UNIVERSE_SNAPSHOT_FILE,
    UNIVERSE_TARGETS_FILE,
    UNIVERSE_TIMESTAMP_FILE,
];
const UNIVERSE_RENEWAL_WINDOW: Duration = Duration::hours(6);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UniversePublicationOutcome {
    Unavailable,
    Unchanged {
        manifest_sha256: String,
        sequence: u64,
    },
    Activated {
        manifest_sha256: String,
        sequence: u64,
    },
}

/// Publish from the configured server roots without holding the state lock
/// across filesystem or SQLite work.
pub(crate) async fn publish_current_universe_from_state(
    state: &Arc<RwLock<ServerState>>,
) -> Result<UniversePublicationOutcome> {
    let (db_path, catalog_dir, candidate_dir, keys_root, database_writer) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.config.catalog_dir.clone(),
            guard.config.catalog_candidate_dir.clone(),
            guard.config.release_publish.repository_keys_dir.clone(),
            guard.database_writer.clone(),
        )
    };
    let outcome = tokio::task::spawn_blocking(move || {
        publish_current_universe_from_roots(
            &db_path,
            &catalog_dir,
            &candidate_dir,
            keys_root.as_deref(),
            &database_writer,
        )
    })
    .await
    .context("signed Remi universe publication task did not complete")??;

    if !matches!(outcome, UniversePublicationOutcome::Unavailable) {
        let (db_path, catalog_authority, search_engine) = {
            let guard = state.read().await;
            (
                guard.config.db_path.clone(),
                guard.catalog_authority.clone(),
                guard.search_engine.clone(),
            )
        };
        if let Some(search_engine) = search_engine {
            let rebuild_engine = Arc::clone(&search_engine);
            let rebuild = tokio::task::spawn_blocking(move || {
                let universe = match super::public_universe::PublicUniverseSnapshot::load(&db_path)?
                {
                    super::public_universe::PublicUniverseLoadOutcome::Current(universe) => {
                        universe
                    }
                    super::public_universe::PublicUniverseLoadOutcome::NoActiveUniverse => {
                        anyhow::bail!("activated Remi universe pointer is absent")
                    }
                    super::public_universe::PublicUniverseLoadOutcome::ObsoleteUniverseSchema {
                        ..
                    } => {
                        anyhow::bail!("activated Remi universe schema is obsolete")
                    }
                    super::public_universe::PublicUniverseLoadOutcome::ObsoleteProfileSchema => {
                        anyhow::bail!("activated Remi universe contains obsolete profile revisions")
                    }
                };
                rebuild_engine
                    .rebuild_from_universe(&db_path, &catalog_authority, &universe)
                    .context("rebuild search projection for activated Remi universe")?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            if let Err(error) = rebuild
                .context("activated-universe search rebuild task did not complete")
                .and_then(|result| result)
            {
                search_engine.mark_unavailable();
                tracing::error!(%error, "Activated Remi universe has no current search projection");
            }
        }
    }

    Ok(outcome)
}

#[derive(Debug)]
struct UniverseInputs {
    base_manifest_sha256: Option<String>,
    base_sequence: u64,
    base_promotion_evidence_sha256: Option<String>,
    base_conversion_crawl_sha256: Option<String>,
    active_manifest: Option<RemiUniverseManifestV2>,
    profiles: Vec<ProfileRevisionV2>,
    profile_physical_attestations: BTreeMap<String, RemiCatalogPhysicalAttestation>,
    canonical_map: CanonicalMapSnapshot,
}

type ProfilePhysicalAttestations = BTreeMap<String, RemiCatalogPhysicalAttestation>;

pub(crate) struct SignedUniverseCandidate {
    pub(crate) manifest: RemiUniverseManifestV2,
    pub(crate) manifest_sha256: String,
    pub(crate) manifest_bytes: Vec<u8>,
    pub(crate) canonical_map_bytes: Vec<u8>,
    pub(crate) root: Signed<conary_core::trust::RootMetadata>,
    pub(crate) root_bytes: Vec<u8>,
    pub(crate) targets: Signed<TargetsMetadata>,
    pub(crate) targets_bytes: Vec<u8>,
    pub(crate) snapshot: Signed<SnapshotMetadata>,
    pub(crate) snapshot_bytes: Vec<u8>,
    pub(crate) timestamp: Signed<TimestampMetadata>,
    pub(crate) timestamp_bytes: Vec<u8>,
}

/// Publish the exact active profile set and canonical map as one signed public
/// universe. Files become durable before the one-row pointer transaction.
#[cfg(test)]
pub(crate) fn publish_current_universe(
    db_path: &Path,
    catalog_dir: &Path,
    candidate_dir: &Path,
    keys_root: Option<&Path>,
    database_writer: &DatabaseWriter,
) -> Result<UniversePublicationOutcome> {
    publish_current_universe_from_roots(
        db_path,
        catalog_dir,
        candidate_dir,
        keys_root,
        database_writer,
    )
}

#[cfg(test)]
fn publish_initial_universe_for_test(
    db_path: &Path,
    catalog_dir: &Path,
    candidate_dir: &Path,
    keys_root: &Path,
    database_writer: &DatabaseWriter,
) -> Result<UniversePublicationOutcome> {
    let mut inputs = database_writer.execute(|| load_inputs(db_path))?;
    anyhow::ensure!(
        inputs.active_manifest.is_none() && !inputs.profiles.is_empty(),
        "test initial universe requires profiles and no active universe"
    );
    inputs.base_promotion_evidence_sha256 = Some("e".repeat(64));
    inputs.base_conversion_crawl_sha256 = Some("c".repeat(64));
    let canonical_map_bytes = canonical_bytes(&inputs.canonical_map)?;
    validate_canonical_candidate(
        catalog_dir,
        &inputs.canonical_map,
        registered_profile_pairs(&inputs.profiles, &inputs.profile_physical_attestations)?,
    )?;
    let candidate = build_candidate(
        0,
        inputs.profiles.clone(),
        canonical_map_bytes,
        load_universe_root_metadata(keys_root)?,
        keys_root,
    )?;
    let bundle = publish_candidate_files(
        candidate_dir,
        catalog_dir,
        &candidate,
        &inputs.profile_physical_attestations,
    )?;
    database_writer.execute(|| activate_candidate(db_path, &inputs, &candidate, &bundle))?;
    Ok(UniversePublicationOutcome::Activated {
        manifest_sha256: candidate.manifest_sha256,
        sequence: candidate.manifest.sequence,
    })
}

fn publish_current_universe_from_roots(
    db_path: &Path,
    catalog_dir: &Path,
    candidate_dir: &Path,
    keys_root: Option<&Path>,
    database_writer: &DatabaseWriter,
) -> Result<UniversePublicationOutcome> {
    let inputs = database_writer.execute(|| load_inputs(db_path))?;
    if inputs.profiles.is_empty() {
        return Ok(UniversePublicationOutcome::Unavailable);
    }
    let keys_root = keys_root.context(
        "release_publish.repository_keys_dir is required to sign the public Remi universe",
    )?;
    let canonical_map_bytes = canonical_bytes(&inputs.canonical_map)?;
    let canonical_map_sha256 = conary_core::hash::sha256(&canonical_map_bytes);
    if inputs.base_manifest_sha256.is_none() {
        return Ok(UniversePublicationOutcome::Unavailable);
    }
    if let (Some(active), Some(manifest_sha256)) =
        (&inputs.active_manifest, &inputs.base_manifest_sha256)
    {
        if !same_authority(active, &inputs.profiles, &canonical_map_sha256) {
            bail!("evidence-free universe publication cannot change active profile authority");
        }
        verify_published_bundle(
            catalog_dir,
            active,
            manifest_sha256,
            &inputs.profile_physical_attestations,
        )?;
        if active_bundle_is_fresh(catalog_dir, active, manifest_sha256, Utc::now())? {
            return Ok(UniversePublicationOutcome::Unchanged {
                manifest_sha256: manifest_sha256.clone(),
                sequence: inputs.base_sequence,
            });
        }
    }

    validate_canonical_candidate(
        catalog_dir,
        &inputs.canonical_map,
        registered_profile_pairs(&inputs.profiles, &inputs.profile_physical_attestations)?,
    )
    .context("validate canonical contracts against the candidate universe")?;
    let root = load_universe_root_metadata(keys_root)?;
    let candidate = build_candidate(
        inputs.base_sequence,
        inputs.profiles.clone(),
        canonical_map_bytes,
        root,
        keys_root,
    )?;
    let bundle = publish_candidate_files(
        candidate_dir,
        catalog_dir,
        &candidate,
        &inputs.profile_physical_attestations,
    )?;

    database_writer
        .execute(|| activate_candidate(db_path, &inputs, &candidate, &bundle))
        .with_context(|| {
            format!(
                "activate durable signed universe {}",
                candidate.manifest_sha256
            )
        })?;
    Ok(UniversePublicationOutcome::Activated {
        manifest_sha256: candidate.manifest_sha256,
        sequence: candidate.manifest.sequence,
    })
}

fn load_inputs(db_path: &Path) -> Result<UniverseInputs> {
    let conn = conary_core::db::open_fast(db_path)?;
    let active = conn
        .query_row(
            "SELECT active.manifest_sha256, active.sequence,
                    revision.promotion_evidence_sha256,
                    revision.conversion_crawl_sha256, revision.manifest_json
             FROM remi_active_universe_revision active
             JOIN remi_universe_revisions revision
               ON revision.manifest_sha256 = active.manifest_sha256
             WHERE active.singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let (
        base_manifest_sha256,
        base_sequence,
        base_promotion_evidence_sha256,
        base_conversion_crawl_sha256,
        active_manifest,
    ) = match active {
        Some((sha256, sequence, promotion_evidence, conversion_crawl, manifest_json)) => {
            let sequence =
                u64::try_from(sequence).context("active universe sequence is negative")?;
            let manifest = match super::universe_revision_inspection::inspect_stored_universe_manifest_v2(
                &sha256,
                i64::try_from(sequence).context("active universe sequence exceeds SQLite INTEGER range")?,
                &manifest_json,
            )? {
                super::universe_revision_inspection::StoredUniverseManifestV2::Current(manifest) => Some(manifest),
                super::universe_revision_inspection::StoredUniverseManifestV2::ObsoleteUniverseSchema { .. } => None,
                super::universe_revision_inspection::StoredUniverseManifestV2::ObsoleteProfileSchema => None,
            };
            (
                Some(sha256),
                sequence,
                Some(promotion_evidence),
                Some(conversion_crawl),
                manifest,
            )
        }
        None => (None, 0, None, None, None),
    };

    let mut statement = conn.prepare(
        "SELECT resource.resource_sha256
         FROM remi_active_profile_revisions active
         JOIN remi_catalog_resources resource
           ON resource.resource_sha256 = active.profile_revision_sha256
         WHERE resource.resource_kind = 'profile_revision' AND resource.durable = 1
         ORDER BY active.source_profile COLLATE BINARY",
    )?;
    let resource_sha256s = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let mut profiles = Vec::new();
    let mut profile_physical_attestations = BTreeMap::new();
    for resource_sha256 in resource_sha256s {
        let resource = RemiCatalogResource::find_by_sha256(&conn, &resource_sha256)?
            .with_context(|| format!("active profile resource {resource_sha256} is absent"))?;
        anyhow::ensure!(
            resource.kind == RemiCatalogResourceKind::ProfileRevision && resource.durable,
            "active profile resource {resource_sha256} lacks durable profile authority"
        );
        let revision = serde_json::from_str::<ProfileRevisionV2>(&resource.manifest_json)
            .context("parse active profile revision")?;
        revision.validate()?;
        anyhow::ensure!(
            revision.manifest_sha256()? == resource_sha256
                && revision.catalog.sha256 == resource.artifact_sha256
                && i64::try_from(revision.catalog.size)? == resource.artifact_size
                && revision.logical_digest_sha256 == resource.logical_digest_sha256,
            "active profile resource {resource_sha256} metadata drifted"
        );
        if conary_core::repository::supported_profiles::profile_by_public_id(&revision.profile)
            .is_none()
        {
            continue;
        }
        anyhow::ensure!(
            profile_physical_attestations
                .insert(resource_sha256, resource.physical_attestation)
                .is_none(),
            "active public profile resource identity repeats"
        );
        profiles.push(revision);
    }
    let canonical_map = load_canonical_map_snapshot(&conn)?;
    Ok(UniverseInputs {
        base_manifest_sha256,
        base_sequence,
        base_promotion_evidence_sha256,
        base_conversion_crawl_sha256,
        active_manifest,
        profiles,
        profile_physical_attestations,
        canonical_map,
    })
}

fn registered_profile_pairs<'a>(
    profiles: &'a [ProfileRevisionV2],
    physical_attestations: &'a ProfilePhysicalAttestations,
) -> Result<Vec<(&'a ProfileRevisionV2, &'a RemiCatalogPhysicalAttestation)>> {
    anyhow::ensure!(
        profiles.len() == physical_attestations.len(),
        "profile physical authority count differs from the candidate universe"
    );
    profiles
        .iter()
        .map(|revision| {
            let revision_sha256 = revision.manifest_sha256()?;
            let physical_attestation = physical_attestations
                .get(&revision_sha256)
                .with_context(|| {
                    format!(
                        "profile '{}' revision {revision_sha256} lacks persisted physical authority",
                        revision.profile
                    )
                })?;
            Ok((revision, physical_attestation))
        })
        .collect()
}

fn same_authority(
    active: &RemiUniverseManifestV2,
    profiles: &[ProfileRevisionV2],
    canonical_map_sha256: &str,
) -> bool {
    active.canonical_map.sha256 == canonical_map_sha256
        && active.profiles.len() == profiles.len()
        && active
            .profiles
            .iter()
            .zip(profiles)
            .all(|(left, right)| left.revision == *right)
}

fn active_bundle_is_fresh(
    catalog_dir: &Path,
    active: &RemiUniverseManifestV2,
    manifest_sha256: &str,
    now: chrono::DateTime<Utc>,
) -> Result<bool> {
    let timestamp_bytes =
        fs::read(universe_bundle_path(catalog_dir, manifest_sha256).join(UNIVERSE_TIMESTAMP_FILE))
            .with_context(|| format!("read active universe timestamp for {manifest_sha256}"))?;
    let timestamp: Signed<TimestampMetadata> =
        serde_json::from_slice(&timestamp_bytes).context("parse active universe timestamp")?;
    Ok(!requires_renewal(
        now,
        active.expires_at,
        timestamp.signed.expires,
    ))
}

fn requires_renewal(
    now: chrono::DateTime<Utc>,
    manifest_expires: chrono::DateTime<Utc>,
    timestamp_expires: chrono::DateTime<Utc>,
) -> bool {
    manifest_expires <= now + UNIVERSE_RENEWAL_WINDOW
        || timestamp_expires <= now + UNIVERSE_RENEWAL_WINDOW
}

include!("universe_publish/candidate.rs");
include!("universe_publish/durable_bundle.rs");
include!("universe_publish/activation.rs");

#[cfg(test)]
mod tests {
    use std::os::unix::fs::DirBuilderExt;

    use conary_core::db::models::{
        CanonicalMappingAuthority, CanonicalPackage, MetadataTable, PackageImplementation,
        set_metadata,
    };

    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::signing_authority::ensure_universe_authority;

    use super::*;

    struct PublicationFixture {
        catalogs: ActiveCatalogFixture,
        candidate_dir: PathBuf,
        keys_root: PathBuf,
        database_writer: DatabaseWriter,
    }

    impl PublicationFixture {
        fn new() -> Self {
            let catalogs = ActiveCatalogFixture::new();
            let root = catalogs
                .catalog_dir()
                .parent()
                .expect("fixture root")
                .to_path_buf();
            let candidate_dir = root.join("universe-candidates");
            fs::create_dir(&candidate_dir).expect("create universe candidate root");
            let keys_root = root.join("repository-keys");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&keys_root)
                .expect("create universe key root");
            ensure_universe_authority(&keys_root).expect("provision universe authority");
            Self {
                catalogs,
                candidate_dir,
                keys_root,
                database_writer: DatabaseWriter::default(),
            }
        }

        fn publish(&self) -> Result<UniversePublicationOutcome> {
            let outcome = publish_current_universe(
                self.catalogs.db_path(),
                self.catalogs.catalog_dir(),
                &self.candidate_dir,
                Some(&self.keys_root),
                &self.database_writer,
            )?;
            if outcome == UniversePublicationOutcome::Unavailable {
                publish_initial_universe_for_test(
                    self.catalogs.db_path(),
                    self.catalogs.catalog_dir(),
                    &self.candidate_dir,
                    &self.keys_root,
                    &self.database_writer,
                )
            } else {
                Ok(outcome)
            }
        }

        fn publish_without_serving_cache_seed(&self) -> Result<UniversePublicationOutcome> {
            let outcome = publish_current_universe_from_roots(
                self.catalogs.db_path(),
                self.catalogs.catalog_dir(),
                &self.candidate_dir,
                Some(&self.keys_root),
                &self.database_writer,
            )?;
            if outcome == UniversePublicationOutcome::Unavailable {
                publish_initial_universe_for_test(
                    self.catalogs.db_path(),
                    self.catalogs.catalog_dir(),
                    &self.candidate_dir,
                    &self.keys_root,
                    &self.database_writer,
                )
            } else {
                Ok(outcome)
            }
        }

        fn set_canonical_mapping(&self, canonical: &str, profile: &str, package: &str) {
            let conn = self.catalogs.connection();
            conn.execute("DELETE FROM package_implementations", [])
                .expect("clear canonical implementations");
            conn.execute("DELETE FROM canonical_packages", [])
                .expect("clear canonical packages");
            let mut canonical_package =
                CanonicalPackage::new(canonical.to_string(), "package".to_string());
            let canonical_id = canonical_package
                .insert(&conn)
                .expect("insert canonical package");
            let mut implementation = PackageImplementation::new(
                canonical_id,
                profile.to_string(),
                package.to_string(),
                CanonicalMappingAuthority::Contract,
            );
            implementation
                .insert(&conn)
                .expect("insert canonical implementation");
            set_metadata(&conn, MetadataTable::Server, "canonical_map_revision", "1")
                .expect("set canonical revision");
            set_metadata(
                &conn,
                MetadataTable::Server,
                "last_canonical_rebuild",
                "2026-08-23T00:00:00Z",
            )
            .expect("set canonical generation time");
        }
    }

    #[test]
    fn duplicate_target_path_is_rejected() {
        let mut targets = BTreeMap::new();
        insert_target(
            &mut targets,
            "objects/sha256/a".to_string(),
            "a".repeat(64),
            1,
        )
        .unwrap();
        assert!(
            insert_target(
                &mut targets,
                "objects/sha256/a".to_string(),
                "b".repeat(64),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn evidence_free_publication_refuses_profile_authority_change() {
        let fixture = PublicationFixture::new();
        let fedora_v1 = fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-bash-v1",
            )],
        );
        let ubuntu_v1 = fixture.catalogs.activate(
            "ubuntu-26.04",
            1,
            vec![package(
                "ubuntu-26.04",
                "bash",
                "5.3",
                "1ubuntu1",
                Some("amd64"),
                101,
                "ubuntu-bash-v1",
            )],
        );

        let first = fixture.publish().expect("publish initial universe");
        let UniversePublicationOutcome::Activated {
            manifest_sha256: first_sha256,
            sequence: 1,
        } = first
        else {
            panic!("initial publication did not activate sequence 1");
        };
        let conn = fixture.catalogs.connection();
        let manifest_json = conn
            .query_row(
                "SELECT manifest_json FROM remi_universe_revisions
                 WHERE manifest_sha256 = ?1",
                [&first_sha256],
                |row| row.get::<_, String>(0),
            )
            .expect("load universe manifest");
        let manifest: RemiUniverseManifestV2 =
            serde_json::from_str(&manifest_json).expect("parse universe manifest");
        assert_eq!(manifest.sequence, 1);
        assert_eq!(
            manifest
                .profiles
                .iter()
                .map(|profile| (
                    profile.revision.profile.as_str(),
                    profile.profile_revision_sha256.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                ("fedora-44", fedora_v1.as_str()),
                ("ubuntu-26.04", ubuntu_v1.as_str()),
            ]
        );
        assert_eq!(
            fixture.publish().expect("repeat publication"),
            UniversePublicationOutcome::Unchanged {
                manifest_sha256: first_sha256.clone(),
                sequence: 1,
            }
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );

        let fedora_v2 = fixture.catalogs.activate(
            "fedora-44",
            2,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "2.fc44",
                Some("x86_64"),
                102,
                "fedora-bash-v2",
            )],
        );
        let error = fixture
            .publish()
            .expect_err("evidence-free authority change must fail");
        assert!(
            format!("{error:#}").contains(
                "evidence-free universe publication cannot change active profile authority"
            ),
            "{error:#}"
        );
        assert_ne!(fedora_v2, fedora_v1);
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256 FROM remi_active_universe_revision WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            first_sha256
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }

    #[test]
    fn obsolete_universe_schema_is_replaced_without_deserializing_it() {
        let fixture = PublicationFixture::new();
        fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-obsolete-universe",
            )],
        );
        fixture.publish().expect("publish initial universe");
        let obsolete_sha256 = fixture
            .catalogs
            .replace_active_universe_with_obsolete_schema();

        assert!(matches!(
            crate::server::public_universe::PublicUniverseSnapshot::load(
                fixture.catalogs.db_path()
            )
            .expect("classify obsolete universe"),
            crate::server::public_universe::PublicUniverseLoadOutcome::ObsoleteUniverseSchema {
                found: 1,
                required: REMI_UNIVERSE_SCHEMA_V2,
            }
        ));

        let UniversePublicationOutcome::Activated {
            manifest_sha256,
            sequence: 3,
        } = fixture
            .publish()
            .expect("publish current-schema replacement")
        else {
            panic!("obsolete universe was not replaced")
        };
        assert_ne!(manifest_sha256, obsolete_sha256);
        assert!(matches!(
            crate::server::public_universe::PublicUniverseSnapshot::load(
                fixture.catalogs.db_path()
            )
            .expect("load replacement universe"),
            crate::server::public_universe::PublicUniverseLoadOutcome::Current(_)
        ));
    }

    #[tokio::test]
    async fn activated_outcome_survives_search_rebuild_failure() {
        let fixture = PublicationFixture::new();
        fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-search-failure",
            )],
        );
        fixture.publish().expect("publish initial universe");
        fixture
            .catalogs
            .replace_active_universe_with_obsolete_schema();

        let root = fixture
            .catalogs
            .catalog_dir()
            .parent()
            .expect("fixture root");
        let mut config = crate::server::ServerConfig::default();
        config.db_path = fixture.catalogs.db_path().to_path_buf();
        config.catalog_dir = fixture.catalogs.catalog_dir().to_path_buf();
        config.catalog_candidate_dir = fixture.candidate_dir.clone();
        config.chunk_dir = root.join("chunks");
        config.cache_dir = root.join("cache");
        config.release_publish.repository_keys_dir = Some(fixture.keys_root.clone());
        fs::create_dir_all(&config.chunk_dir).expect("create chunk root");
        fs::create_dir_all(&config.cache_dir).expect("create cache root");

        let mut server_state = crate::server::ServerState::new(config).expect("server state");
        server_state.catalog_authority =
            crate::server::catalog_authority::CatalogAuthority::from_paths(
                fixture.catalogs.db_path().to_path_buf(),
                root.join("missing-search-catalogs"),
                server_state.database_writer.clone(),
            );
        let search_dir = root.join("search");
        let search_engine =
            Arc::new(crate::server::SearchEngine::new(&search_dir).expect("create search engine"));
        server_state.search_engine = Some(Arc::clone(&search_engine));
        let state = Arc::new(RwLock::new(server_state));

        let outcome = publish_current_universe_from_state(&state)
            .await
            .expect("activation outcome must not be replaced by search failure");
        let UniversePublicationOutcome::Activated { sequence: 3, .. } = outcome else {
            panic!("obsolete universe replacement did not activate")
        };
        let crate::server::public_universe::PublicUniverseLoadOutcome::Current(universe) =
            crate::server::public_universe::PublicUniverseSnapshot::load(
                fixture.catalogs.db_path(),
            )
            .expect("load activated universe")
        else {
            panic!("activated replacement is not current")
        };
        assert!(matches!(
            search_engine.search_public_universe(universe.identity(), "bash", None, 10),
            Err(crate::server::search::PublicSearchError::Unavailable)
        ));
    }

    #[test]
    fn publication_validation_does_not_seed_the_serving_reader_cache() {
        let fixture = PublicationFixture::new();
        let revision = fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-cache-seed",
            )],
        );

        fixture
            .publish_without_serving_cache_seed()
            .expect("publish universe without seeding a serving reader");

        assert!(
            !fixture
                .catalogs
                .authority()
                .has_verified_profile_reader_for_test("fedora-44", &revision)
        );
    }

    #[test]
    fn tampered_active_bundle_fails_closed_without_advancing_pointer() {
        let fixture = PublicationFixture::new();
        fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-bash",
            )],
        );
        let first = fixture.publish().expect("publish initial universe");
        let UniversePublicationOutcome::Activated {
            manifest_sha256,
            sequence: 1,
        } = first
        else {
            panic!("initial publication did not activate sequence 1");
        };
        let canonical_path = universe_bundle_path(fixture.catalogs.catalog_dir(), &manifest_sha256)
            .join(UNIVERSE_CANONICAL_MAP_FILE);
        fs::write(&canonical_path, b"{}\n").expect("tamper canonical map");

        let error = fixture.publish().expect_err("tampered bundle must fail");
        assert!(
            format!("{error:#}").contains("invalid canonical map JSON"),
            "{error:#}"
        );
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (manifest_sha256, 1)
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }

    #[test]
    fn evidence_free_canonical_change_preserves_the_active_universe() {
        let fixture = PublicationFixture::new();
        fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-bash",
            )],
        );
        let first = fixture.publish().expect("publish initial universe");
        let UniversePublicationOutcome::Activated {
            manifest_sha256,
            sequence: 1,
        } = first
        else {
            panic!("initial publication did not activate sequence 1");
        };
        fixture.set_canonical_mapping("shell", "fedora-44", "missing-shell");

        let error = fixture
            .publish()
            .expect_err("evidence-free canonical authority change must fail");
        assert!(
            format!("{error:#}").contains(
                "evidence-free universe publication cannot change active profile authority"
            ),
            "{error:#}"
        );
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (manifest_sha256, 1)
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            fs::read_dir(fixture.catalogs.catalog_dir().join("universes"))
                .expect("read durable universe bundles")
                .count(),
            1
        );
    }

    #[test]
    fn unchanged_authority_renews_before_timestamp_expiry() {
        let now = "2026-08-22T12:00:00Z".parse().unwrap();
        assert!(!requires_renewal(
            now,
            now + Duration::days(7),
            now + Duration::hours(7),
        ));
        assert!(requires_renewal(
            now,
            now + Duration::days(7),
            now + Duration::hours(6),
        ));
        assert!(requires_renewal(
            now,
            now + Duration::hours(5),
            now + Duration::days(1),
        ));
    }
}
