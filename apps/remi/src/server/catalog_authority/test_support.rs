// apps/remi/src/server/catalog_authority/test_support.rs

//! Current-schema immutable catalog fixtures shared by Remi reader tests.

use std::path::{Path, PathBuf};

use conary_core::db::models::{
    RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionMember, RemiRuntimeSession,
};
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogScopeV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V1, ProfileRevisionV1,
    ProfileSourceMemberV1, SourceStreamKindV1, SourceStreamV1, publish_profile_catalog_bundle,
    write_catalog_candidate, write_profile_catalog_manifest,
};
use conary_core::repository::dependency_model::DebianMultiArch;
use conary_core::repository::versioning::VersionScheme;

use super::CatalogAuthority;
use crate::server::database_writer::DatabaseWriter;

pub(crate) struct ActiveCatalogFixture {
    root: tempfile::TempDir,
    db_path: PathBuf,
    catalog_dir: PathBuf,
    authority: CatalogAuthority,
}

impl ActiveCatalogFixture {
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().expect("create active catalog fixture");
        let db_path = root.path().join("metadata/conary.db");
        let catalog_dir = root.path().join("catalogs");
        std::fs::create_dir_all(db_path.parent().expect("metadata parent"))
            .expect("create metadata directory");
        std::fs::create_dir_all(&catalog_dir).expect("create catalog directory");
        conary_core::db::init(&db_path).expect("initialize current schema");
        let conn = conary_core::db::open_fast(&db_path).expect("open fixture database");
        RemiRuntimeSession::begin(&conn, 1).expect("install fixture runtime session");
        let database_writer = DatabaseWriter::default();
        let authority =
            CatalogAuthority::from_paths(db_path.clone(), catalog_dir.clone(), database_writer);
        Self {
            root,
            db_path,
            catalog_dir,
            authority,
        }
    }

    pub(crate) fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub(crate) fn authority(&self) -> &CatalogAuthority {
        &self.authority
    }

    pub(crate) fn connection(&self) -> rusqlite::Connection {
        conary_core::db::open_fast(&self.db_path).expect("open fixture database")
    }

    pub(crate) fn activate(
        &self,
        profile: &str,
        fencing_epoch: i64,
        packages: Vec<CatalogPackageRecordV1>,
    ) -> String {
        let source_identity = format!("source-{profile}");
        let repository_identity = format!("repository-{profile}");
        let source_manifest_json = String::from_utf8(
            conary_core::json::canonical_json(&serde_json::json!({
                "fencing_epoch": fencing_epoch,
                "fixture": "active-catalog-source",
                "profile": profile,
            }))
            .expect("serialize source fixture resource"),
        )
        .expect("source fixture JSON is UTF-8");
        let source_snapshot_sha256 = conary_core::hash::sha256(source_manifest_json.as_bytes());
        let packages = packages
            .into_iter()
            .map(|mut package| {
                package.package_key_sha256.clear();
                package.origin = CatalogPackageOriginV1::Profile {
                    member_ordinal: 0,
                    source_identity: source_identity.clone(),
                    repository_identity: repository_identity.clone(),
                    source_snapshot_sha256: source_snapshot_sha256.clone(),
                };
                package
            })
            .collect();
        let content = CatalogContentV1::new(
            CatalogScopeV1::Profile {
                profile: profile.to_string(),
            },
            vec![CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: 0,
                source_identity: source_identity.clone(),
                repository_identity: repository_identity.clone(),
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            }],
            packages,
        )
        .expect("build active profile catalog");
        let candidate_dir = self
            .root
            .path()
            .join(format!("candidate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&candidate_dir).expect("create catalog candidate");
        let binding = write_catalog_candidate(candidate_dir.join(CATALOG_FILE_NAME), &content)
            .expect("write catalog candidate");
        let manifest = ProfileRevisionV1 {
            schema_version: PROFILE_REVISION_SCHEMA_V1,
            profile: profile.to_string(),
            projection_version: 1,
            members: vec![ProfileSourceMemberV1 {
                ordinal: 0,
                source_identity: source_identity.clone(),
                repository_identity: repository_identity.clone(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "stable".to_string(),
                },
                priority: 0,
                required: true,
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            }],
            catalog: binding.artifact.clone(),
            logical_digest_sha256: binding.logical_digest_sha256.clone(),
            counts: binding.counts,
        };
        write_profile_catalog_manifest(&candidate_dir, &manifest).expect("write profile manifest");
        publish_profile_catalog_bundle(&candidate_dir, &self.catalog_dir, &manifest)
            .expect("publish profile catalog");

        let profile_revision_sha256 = manifest.manifest_sha256().expect("hash profile revision");
        let profile_manifest_json = String::from_utf8(
            conary_core::json::canonical_json(&manifest).expect("serialize profile revision"),
        )
        .expect("profile fixture JSON is UTF-8");
        let conn = self.connection();
        RemiCatalogResource {
            resource_sha256: source_snapshot_sha256.clone(),
            kind: RemiCatalogResourceKind::SourceSnapshot,
            source_profile: profile.to_string(),
            artifact_sha256: conary_core::hash::sha256(
                format!("source-artifact-{profile}-{fencing_epoch}").as_bytes(),
            ),
            artifact_size: 1,
            logical_digest_sha256: conary_core::hash::sha256(
                format!("source-logical-{profile}-{fencing_epoch}").as_bytes(),
            ),
            manifest_json: source_manifest_json,
            durable: true,
            created_at: fencing_epoch,
        }
        .insert(&conn)
        .expect("insert source resource");
        RemiCatalogResource {
            resource_sha256: profile_revision_sha256.clone(),
            kind: RemiCatalogResourceKind::ProfileRevision,
            source_profile: profile.to_string(),
            artifact_sha256: manifest.catalog.sha256.clone(),
            artifact_size: i64::try_from(manifest.catalog.size).expect("artifact size fits"),
            logical_digest_sha256: manifest.logical_digest_sha256.clone(),
            manifest_json: profile_manifest_json,
            durable: true,
            created_at: fencing_epoch,
        }
        .insert(&conn)
        .expect("insert profile resource");
        RemiProfileRevisionMember {
            profile_revision_sha256: profile_revision_sha256.clone(),
            ordinal: 0,
            source_snapshot_sha256,
            source_identity,
            repository_identity,
            stream_kind: "release".to_string(),
            stream_identity: "stable".to_string(),
            priority: 0,
            required: true,
        }
        .insert(&conn)
        .expect("insert profile member");
        let run_id = uuid::Uuid::new_v4().to_string();
        let owner_instance_uuid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 input_profile_digest, candidate_profile_digest, state,
                 started_at, heartbeat_at, lease_expires_at, finished_at
             ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'published', ?4, ?4, ?4, ?4)",
            rusqlite::params![
                run_id,
                profile,
                owner_instance_uuid,
                fencing_epoch,
                profile_revision_sha256,
            ],
        )
        .expect("insert activation run");
        conn.execute(
            "INSERT INTO remi_active_profile_revisions (
                 source_profile, profile_revision_sha256, fencing_epoch,
                 activation_run_id, owner_instance_uuid, activated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?3)
             ON CONFLICT(source_profile) DO UPDATE SET
                 profile_revision_sha256 = excluded.profile_revision_sha256,
                 fencing_epoch = excluded.fencing_epoch,
                 activation_run_id = excluded.activation_run_id,
                 owner_instance_uuid = excluded.owner_instance_uuid,
                 activated_at = excluded.activated_at",
            rusqlite::params![
                profile,
                profile_revision_sha256,
                fencing_epoch,
                run_id,
                owner_instance_uuid,
            ],
        )
        .expect("activate profile revision");
        profile_revision_sha256
    }
}

pub(crate) fn package(
    profile: &str,
    name: &str,
    version: &str,
    release: &str,
    architecture: Option<&str>,
    size: u64,
    checksum_marker: &str,
) -> CatalogPackageRecordV1 {
    let version_scheme = conary_core::repository::supported_profiles::profile_by_public_id(profile)
        .map_or(VersionScheme::Rpm, |profile| profile.version_scheme());
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: format!("source-{profile}"),
            repository_identity: format!("repository-{profile}"),
            source_snapshot_sha256: conary_core::hash::sha256(
                format!("package-snapshot-{profile}").as_bytes(),
            ),
        },
        source_profile: profile.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        package_release: release.to_string(),
        architecture: architecture.map(str::to_string),
        debian_multi_arch: (version_scheme == VersionScheme::Debian).then_some(DebianMultiArch::No),
        description: None,
        checksum: conary_core::hash::sha256(checksum_marker.as_bytes()),
        size,
        download_url: format!("https://example.invalid/{name}-{version}-{release}.rpm"),
        metadata: None,
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme,
        provides: Vec::new(),
        requirement_groups: Vec::new(),
    }
}
