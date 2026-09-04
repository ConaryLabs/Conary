// conary-core/src/repository/universe/index/tests.rs

use super::*;
use crate::canonical::{CanonicalMapEntry, CanonicalMapSnapshot};
use crate::db::models::{Repository, RepositoryPackage, RepositoryProvide};
use crate::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1,
    CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V3, ProfileRevisionV2, ProfileSourceMemberV2,
    SourceStreamKindV1, SourceStreamV1, write_catalog_candidate,
};
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::universe::{
    REMI_UNIVERSE_SCHEMA_V2, RemiUniverseCanonicalMapObjectV2, RemiUniverseCatalogObjectV2,
    RemiUniverseProfileV2,
};
use crate::repository::versioning::VersionScheme;
use crate::resolver::PackageIdentity;
use std::collections::BTreeMap;

const ENDPOINT: &str = "https://remi.example.test";
const PROFILE: &str = "fedora-44";

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn profile_members() -> Vec<ProfileSourceMemberV2> {
    let mut declared = crate::repository::supported_profiles::profile_by_public_id(PROFILE)
        .unwrap()
        .members()
        .iter()
        .collect::<Vec<_>>();
    declared.sort_by_key(|member| std::cmp::Reverse(member.precedence));
    declared
        .into_iter()
        .enumerate()
        .map(|(ordinal, member)| ProfileSourceMemberV2 {
            ordinal: ordinal as u32,
            source_identity: "fedora-project".to_string(),
            repository_identity: member.repository_identity.clone(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            role: member.role,
            precedence: member.precedence,
            required: true,
            source_snapshot_sha256: digest('1'),
        })
        .collect()
}

fn profile_evidence() -> Vec<CatalogSourceEvidenceV1> {
    profile_members()
        .into_iter()
        .map(|member| CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: member.ordinal,
            source_identity: member.source_identity,
            repository_identity: member.repository_identity,
            source_snapshot_sha256: member.source_snapshot_sha256,
        })
        .collect()
}

#[test]
fn private_index_defers_secondary_indexes_until_after_bulk_replay() {
    let index = Connection::open_in_memory().unwrap();
    index.execute_batch(CLIENT_INDEX_SCHEMA).unwrap();
    let before = index
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'index' AND sql IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(before, 0);

    index.execute_batch(CLIENT_INDEX_INDEXES).unwrap();
    let after = index
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'index' AND sql IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(after, 11);
}

#[test]
fn private_index_finalization_requires_an_append_only_candidate() {
    let index = Connection::open_in_memory().unwrap();
    index
        .execute_batch(
            "PRAGMA page_size = 4096;
                 CREATE TABLE discarded (payload BLOB NOT NULL);
                 INSERT INTO discarded VALUES (zeroblob(1048576));
                 DROP TABLE discarded;",
        )
        .unwrap();
    let freelist_pages: i64 = index
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .unwrap();
    assert!(freelist_pages > 0);

    let error = finalize_candidate(&index).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("client universe append-only candidate has")
    );
}

fn package(version: &str) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-44-updates-x86_64".to_string(),
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
) -> (RemiUniverseManifestV2, ClientUniverseIndex) {
    let catalog_path = root.join(format!("catalog-{sequence}.sqlite"));
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: PROFILE.to_string(),
        },
        profile_evidence(),
        vec![package(version)],
    )
    .unwrap();
    let binding = write_catalog_candidate(&catalog_path, &content).unwrap();
    let revision = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V3,
        profile: PROFILE.to_string(),
        target_architecture:
            crate::repository::supported_profiles::ProfileTargetArchitecture::X86_64,
        projection_version: 1,
        members: profile_members(),
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
    let manifest = RemiUniverseManifestV2 {
        schema_version: REMI_UNIVERSE_SCHEMA_V2,
        sequence,
        metadata_root_sha256: digest('3'),
        generated_at,
        expires_at: generated_at + chrono::Duration::days(7),
        profiles: vec![RemiUniverseProfileV2 {
            ordinal: 0,
            profile_revision_sha256: revision.manifest_sha256().unwrap(),
            catalog: RemiUniverseCatalogObjectV2 {
                schema_version: CATALOG_CONTENT_SCHEMA_V1,
                sha256: binding.artifact.sha256.clone(),
                size: binding.artifact.size,
                logical_digest_sha256: binding.logical_digest_sha256,
            },
            revision,
        }],
        canonical_map: RemiUniverseCanonicalMapObjectV2 {
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

fn activate(conn: &Connection, manifest: &RemiUniverseManifestV2, index: &ClientUniverseIndex) {
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
        let manifest: RemiUniverseManifestV2 =
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
        let private =
            Connection::open_with_flags(&index.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
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
        crate::repository::catalog::CatalogCandidateWriter::create(&catalog_path, scope).unwrap();
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
    let evidence = profile_evidence();
    let binding = writer.finish(evidence).unwrap();
    let revision = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V3,
        profile: PROFILE.to_string(),
        target_architecture:
            crate::repository::supported_profiles::ProfileTargetArchitecture::X86_64,
        projection_version: 1,
        members: profile_members(),
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
    let manifest = RemiUniverseManifestV2 {
        schema_version: REMI_UNIVERSE_SCHEMA_V2,
        sequence: 1,
        metadata_root_sha256: digest('3'),
        generated_at,
        expires_at: generated_at + chrono::Duration::days(7),
        profiles: vec![RemiUniverseProfileV2 {
            ordinal: 0,
            profile_revision_sha256: revision.manifest_sha256().unwrap(),
            catalog: RemiUniverseCatalogObjectV2 {
                schema_version: CATALOG_CONTENT_SCHEMA_V1,
                sha256: binding.artifact.sha256,
                size: binding.artifact.size,
                logical_digest_sha256: binding.logical_digest_sha256,
            },
            revision,
        }],
        canonical_map: RemiUniverseCanonicalMapObjectV2 {
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
