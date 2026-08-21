// apps/remi/src/server/catalog_authority/tests.rs

use std::fs;
use std::path::PathBuf;

use conary_core::db::models::{RemiCatalogResource, RemiCatalogResourceKind};
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, CatalogContentV1, CatalogScopeV1, CatalogSourceEvidenceV1,
    ProfileRevisionV1, ProfileSourceMemberV1, SourceStreamKindV1, SourceStreamV1,
    publish_profile_catalog_bundle, write_catalog_candidate, write_profile_catalog_manifest,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;

use super::open_active_profile_from_connection;

const PROFILE: &str = "test-profile";

struct Fixture {
    root: TempDir,
    catalog_dir: PathBuf,
    conn: Connection,
}

struct Revision {
    digest: String,
    bundle_dir: PathBuf,
    artifact_path: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().expect("create fixture root");
    let catalog_dir = root.path().join("catalogs");
    let conn = Connection::open_in_memory().expect("open fixture database");
    conn.execute_batch(
        "CREATE TABLE remi_catalog_resources (
             resource_sha256 TEXT PRIMARY KEY,
             resource_kind TEXT NOT NULL,
             source_profile TEXT NOT NULL,
             artifact_sha256 TEXT NOT NULL,
             artifact_size INTEGER NOT NULL,
             logical_digest_sha256 TEXT NOT NULL,
             manifest_json TEXT NOT NULL,
             durable INTEGER NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE remi_active_profile_revisions (
             source_profile TEXT PRIMARY KEY,
             profile_revision_sha256 TEXT NOT NULL,
             fencing_epoch INTEGER NOT NULL,
             activation_run_id TEXT NOT NULL,
             owner_instance_uuid TEXT NOT NULL,
             activated_at INTEGER NOT NULL
         );",
    )
    .expect("create authority metadata tables");
    Fixture {
        root,
        catalog_dir,
        conn,
    }
}

fn add_revision(fixture: &Fixture, marker: char, fencing_epoch: i64) -> Revision {
    let source_snapshot_sha256 = marker.to_string().repeat(64);
    fs::create_dir_all(&fixture.catalog_dir).expect("create catalog root");
    let candidate_dir = fixture
        .root
        .path()
        .join(format!("candidate-{marker}-{fencing_epoch}"));
    fs::create_dir_all(&candidate_dir).expect("create candidate directory");
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: PROFILE.to_string(),
        },
        vec![CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: 0,
            source_identity: format!("source-{marker}"),
            repository_identity: format!("repository-{marker}"),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        }],
        Vec::new(),
    )
    .expect("build profile catalog content");
    let binding = write_catalog_candidate(candidate_dir.join(CATALOG_FILE_NAME), &content)
        .expect("write profile catalog artifact");
    let manifest = ProfileRevisionV1 {
        schema_version: 1,
        profile: PROFILE.to_string(),
        projection_version: 1,
        members: vec![ProfileSourceMemberV1 {
            ordinal: 0,
            source_identity: format!("source-{marker}"),
            repository_identity: format!("repository-{marker}"),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "stable".to_string(),
            },
            priority: 0,
            required: true,
            source_snapshot_sha256,
        }],
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    };
    write_profile_catalog_manifest(&candidate_dir, &manifest)
        .expect("write profile catalog manifest");
    let bundle_dir =
        publish_profile_catalog_bundle(&candidate_dir, &fixture.catalog_dir, &manifest)
            .expect("publish profile catalog bundle");
    let digest = manifest.manifest_sha256().expect("hash profile revision");
    let manifest_json = String::from_utf8(
        conary_core::json::canonical_json(&manifest).expect("canonicalize profile revision"),
    )
    .expect("profile revision JSON is UTF-8");
    RemiCatalogResource {
        resource_sha256: digest.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: PROFILE.to_string(),
        artifact_sha256: manifest.catalog.sha256.clone(),
        artifact_size: i64::try_from(manifest.catalog.size).expect("artifact size fits SQLite"),
        logical_digest_sha256: manifest.logical_digest_sha256.clone(),
        manifest_json,
        durable: true,
        created_at: fencing_epoch,
    }
    .insert(&fixture.conn)
    .expect("insert profile resource");
    fixture
        .conn
        .execute(
            "INSERT INTO remi_active_profile_revisions (
                 source_profile, profile_revision_sha256, fencing_epoch,
                 activation_run_id, owner_instance_uuid, activated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_profile) DO UPDATE SET
                 profile_revision_sha256 = excluded.profile_revision_sha256,
                 fencing_epoch = excluded.fencing_epoch,
                 activation_run_id = excluded.activation_run_id,
                 owner_instance_uuid = excluded.owner_instance_uuid,
                 activated_at = excluded.activated_at",
            params![
                PROFILE,
                &digest,
                fencing_epoch,
                format!("00000000-0000-0000-0000-{fencing_epoch:012}"),
                format!("11111111-1111-1111-1111-{fencing_epoch:012}"),
                fencing_epoch,
            ],
        )
        .expect("activate profile revision");
    Revision {
        digest,
        artifact_path: bundle_dir.join(CATALOG_FILE_NAME),
        bundle_dir,
    }
}

fn open(fixture: &Fixture) -> anyhow::Result<super::PinnedProfileCatalog> {
    open_active_profile_from_connection(&fixture.conn, &fixture.catalog_dir, PROFILE)
}

#[test]
fn opens_valid_active_profile_catalog() {
    let fixture = fixture();
    let revision = add_revision(&fixture, 'a', 1);

    let pinned = open(&fixture).expect("open active profile catalog");

    assert_eq!(pinned.source_profile(), PROFILE);
    assert_eq!(pinned.profile_revision_sha256(), revision.digest);
    assert_eq!(pinned.fencing_epoch(), 1);
    assert_eq!(pinned.manifest().profile, PROFILE);
    assert_eq!(pinned.reader().binding().counts, pinned.manifest().counts);
    assert_eq!(
        pinned.catalog_path(),
        fs::canonicalize(&revision.artifact_path)
            .expect("canonicalize verified catalog path")
            .as_path()
    );
}

#[test]
fn rejects_pointer_without_exact_resource() {
    let fixture = fixture();
    let _revision = add_revision(&fixture, 'a', 1);
    let missing_digest = "f".repeat(64);
    fixture
        .conn
        .execute(
            "UPDATE remi_active_profile_revisions
             SET profile_revision_sha256 = ?1, fencing_epoch = 2
             WHERE source_profile = ?2",
            params![missing_digest, PROFILE],
        )
        .expect("replace active pointer with invalid revision");

    let error = open(&fixture).expect_err("invalid pointer must fail closed");
    assert!(
        format!("{error:#}").contains("has no catalog resource"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn rejects_tampered_catalog_artifact() {
    let fixture = fixture();
    let revision = add_revision(&fixture, 'a', 1);
    let mut bytes = fs::read(&revision.artifact_path).expect("read catalog artifact");
    let offset = bytes.len() / 2;
    bytes[offset] ^= 0xff;
    fs::write(&revision.artifact_path, bytes).expect("tamper catalog artifact");

    let error = open(&fixture).expect_err("tampered catalog must fail closed");
    assert!(
        format!("{error:#}").contains("Checksum mismatch"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn rejects_missing_catalog_bundle() {
    let fixture = fixture();
    let revision = add_revision(&fixture, 'a', 1);
    fs::remove_dir_all(&revision.bundle_dir).expect("remove catalog bundle");

    let error = open(&fixture).expect_err("missing catalog must fail closed");
    assert!(
        format!("{error:#}").contains("No such file")
            || format!("{error:#}").contains("verify active profile catalog bundle"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn old_handle_remains_pinned_after_new_activation() {
    let fixture = fixture();
    let first = add_revision(&fixture, 'a', 1);
    let old_handle = open(&fixture).expect("open first active profile");
    let second = add_revision(&fixture, 'b', 2);
    let new_handle = open(&fixture).expect("open second active profile");

    assert_eq!(old_handle.profile_revision_sha256(), first.digest);
    assert_eq!(old_handle.fencing_epoch(), 1);
    assert_eq!(new_handle.profile_revision_sha256(), second.digest);
    assert_eq!(new_handle.fencing_epoch(), 2);
    assert_ne!(old_handle.catalog_path(), new_handle.catalog_path());
    assert_eq!(old_handle.reader().source_evidence().unwrap().len(), 1);
    assert_eq!(new_handle.reader().source_evidence().unwrap().len(), 1);
    assert_eq!(old_handle.manifest().members[0].source_identity, "source-a");
    assert_eq!(new_handle.manifest().members[0].source_identity, "source-b");
}
