// conary-core/src/repository/catalog/candidate/tests.rs

use super::*;
use crate::repository::catalog::{
    CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2, CatalogCopyScratchV1, CatalogFinalizationScratchV2,
    CatalogMetadataScratchV1, CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
    CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogProfileCandidateScratchV1,
    CatalogProjectionSpoolScratchV1, CatalogProvideRecordV1, CatalogRequirementAtomV1,
    CatalogRequirementGroupV1, CatalogScratchCapacityError, CatalogSourceEvidenceV1,
};
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryRequirementClause,
};
use crate::repository::dependency_source::{CapabilityProvenance, SourcePackageFormat};
use crate::repository::versioning::VersionScheme;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct RecordingAdmission {
    requirement: Mutex<Option<CatalogFinalizationScratchV2>>,
    lease_drops: Arc<AtomicUsize>,
    refuse: bool,
}

struct RecordingLease(Arc<AtomicUsize>);

impl Drop for RecordingLease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl CatalogScratchAdmission for RecordingAdmission {
    fn reserve_source_candidate(
        &self,
        _candidate_path: &Path,
        _requirement: CatalogSourceCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("finalization-only writer must not request source growth admission")
    }

    fn reserve_profile_candidate(
        &self,
        _candidate_path: &Path,
        _requirement: CatalogProfileCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("finalization-only writer must not request profile growth admission")
    }

    fn reserve_metadata(
        &self,
        _work_directory: &Path,
        _requirement: CatalogMetadataScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("candidate writer must not request metadata admission")
    }

    fn stream_metadata(
        &self,
        _work_directory: &Path,
        _requirement: CatalogMetadataStreamScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        panic!("candidate writer must not request streamed metadata admission")
    }

    fn stream_projection_spool(
        &self,
        _work_directory: &Path,
        _requirement: CatalogProjectionSpoolScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        panic!("candidate writer must not request projection spool admission")
    }

    fn reserve_finalization(
        &self,
        _candidate_path: &Path,
        requirement: CatalogFinalizationScratchV2,
    ) -> Result<Box<dyn Send>> {
        *self.requirement.lock().unwrap() = Some(requirement);
        if self.refuse {
            return Err(CatalogScratchCapacityError {
                required_bytes: requirement.required_additional_bytes,
                available_bytes: requirement.required_additional_bytes - 1,
                reserved_bytes: 0,
            }
            .into());
        }
        Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
    }

    fn reserve_copy(
        &self,
        _destination_root: &Path,
        _requirement: CatalogCopyScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("candidate writer must not request a catalog-copy reservation")
    }
}

fn scope() -> CatalogScopeV1 {
    CatalogScopeV1::Source {
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: "fedora-everything-x86_64".to_string(),
    }
}

fn evidence() -> Vec<CatalogSourceEvidenceV1> {
    vec![CatalogSourceEvidenceV1::AuthenticatedObject {
        role: crate::repository::catalog::SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.gz".to_string(),
        sha256: "a".repeat(64),
        size: 1,
    }]
}

fn rpm_package(
    name: &str,
    checksum: &str,
    paths: &[&str],
    required_path: Option<&str>,
) -> CatalogPackageRecordV1 {
    let mut provides = vec![CatalogProvideRecordV1 {
        capability: name.to_string(),
        version: Some("1-1".to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        kind: "package".to_string(),
        raw: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    provides.extend(paths.iter().map(|path| rpm_file_provide(path)));
    let requirement_groups = required_path
        .map(|path| {
            let clause = RepositoryRequirementClause::name_only(path.to_string());
            vec![CatalogRequirementGroupV1 {
                kind: "depends".to_string(),
                behavior: "hard".to_string(),
                description: None,
                native_text: Some(path.to_string()),
                expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(
                    clause.clone(),
                ))
                .unwrap(),
                atoms: vec![CatalogRequirementAtomV1 {
                    capability: path.to_string(),
                    version_constraint: None,
                    kind: "file".to_string(),
                    dependency_type: "runtime".to_string(),
                    raw: Some(path.to_string()),
                }],
            }]
        })
        .unwrap_or_default();
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Source {
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        },
        source_profile: "fedora-44".to_string(),
        name: name.to_string(),
        version: "1-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: None,
        checksum: checksum.to_string(),
        size: 1,
        download_url: format!("https://repo.test/{name}.rpm"),
        metadata: None,
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Rpm,
        provides,
        requirement_groups,
    }
}

fn rpm_file_provide(path: &str) -> CatalogProvideRecordV1 {
    CatalogProvideRecordV1 {
        capability: path.to_string(),
        version: None,
        version_relation: None,
        kind: "file".to_string(),
        raw: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::SourceDerivedFile {
            format: SourcePackageFormat::Rpm,
        },
    }
}

#[test]
fn rpm_primary_file_audit_reads_the_candidate_projection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.sqlite3");
    let mut writer = CatalogCandidateWriter::create(&path, scope()).unwrap();
    writer
        .package(rpm_package("provider", "a", &["/usr/bin/provided"], None))
        .unwrap();
    writer
        .package(rpm_package("consumer", "b", &[], Some("/usr/bin/provided")))
        .unwrap();

    writer
        .validate_rpm_primary_file_requirements("https://repo.test/fedora")
        .unwrap();

    let missing_path = root.path().join("missing.sqlite3");
    let mut missing = CatalogCandidateWriter::create(&missing_path, scope()).unwrap();
    missing
        .package(rpm_package("consumer", "c", &[], Some("/usr/lib/missing")))
        .unwrap();
    let error = missing
        .validate_rpm_primary_file_requirements("https://repo.test/fedora")
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("consumer 1-1"), "{message}");
    assert!(message.contains("/usr/lib/missing"), "{message}");
    assert!(message.contains("no filelists record"), "{message}");
}

#[test]
fn rpm_filelists_checksum_join_is_indexed_only_during_construction() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.sqlite3");
    let mut writer = CatalogCandidateWriter::create(&path, scope()).unwrap();
    writer
        .package(rpm_package("alpha", "a", &[], None))
        .unwrap();
    writer
        .package(rpm_package("bravo", "b", &[], None))
        .unwrap();

    let details = {
        let mut statement = writer
            .connection()
            .unwrap()
            .prepare(
                "EXPLAIN QUERY PLAN
                     SELECT package_key_sha256, name, version, architecture
                     FROM catalog_packages WHERE checksum = ?1
                     ORDER BY package_key_sha256 LIMIT 1",
            )
            .unwrap();
        statement
            .query_map(["b"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(
        details
            .iter()
            .any(|detail| detail.contains("catalog_ingest_packages_checksum")),
        "{details:?}"
    );

    let provide_indexes = |connection: &Connection| {
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = 'index'
                       AND name IN ('catalog_provides_capability', 'catalog_provides_raw')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert_eq!(provide_indexes(writer.connection().unwrap()), 2);
    assert_eq!(
        writer
            .extend_package_provides(
                "rpm_filelists",
                "a",
                "alpha",
                "1-1",
                Some("x86_64"),
                vec![rpm_file_provide("/usr/bin/alpha")],
            )
            .unwrap(),
        CatalogProvideMerge {
            matched_packages: 1,
            added: 1,
            already_known: 0,
        }
    );
    assert_eq!(provide_indexes(writer.connection().unwrap()), 0);
    writer
        .extend_package_provides(
            "rpm_filelists",
            "b",
            "bravo",
            "1-1",
            Some("x86_64"),
            vec![rpm_file_provide("/usr/bin/bravo")],
        )
        .unwrap();
    writer.finish_package_join("rpm_filelists").unwrap();

    writer.finish(evidence()).unwrap();
    let reopened = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let published_indexes: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'index' AND name = 'catalog_ingest_packages_checksum'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(published_indexes, 0);
    assert_eq!(provide_indexes(&reopened), 2);
}

#[test]
fn arch_fragments_pair_canonically_inside_the_candidate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.sqlite3");
    let writer = CatalogCandidateWriter::create(&path, scope()).unwrap();
    writer
        .stage_arch_package_fragment(
            "zeta-2-1".to_string(),
            ArchPackageFragmentKind::Depends,
            "%DEPENDS%\nlibc\n".to_string(),
        )
        .unwrap();
    writer
        .stage_arch_package_fragment(
            "alpha-1-1".to_string(),
            ArchPackageFragmentKind::Desc,
            "%NAME%\nalpha\n".to_string(),
        )
        .unwrap();
    writer
        .stage_arch_package_fragment(
            "zeta-2-1".to_string(),
            ArchPackageFragmentKind::Desc,
            "%NAME%\nzeta\n".to_string(),
        )
        .unwrap();

    let alpha = writer.take_arch_package_record().unwrap().unwrap();
    assert_eq!(alpha.directory, "alpha-1-1");
    assert_eq!(alpha.desc, "%NAME%\nalpha\n");
    assert_eq!(alpha.depends, None);
    let zeta = writer.take_arch_package_record().unwrap().unwrap();
    assert_eq!(zeta.directory, "zeta-2-1");
    assert_eq!(zeta.depends.as_deref(), Some("%DEPENDS%\nlibc\n"));
    assert_eq!(writer.take_arch_package_record().unwrap(), None);
    assert!(
        !table_exists(
            writer.connection().unwrap(),
            "catalog_ingest_arch_fragments"
        )
        .unwrap()
    );

    writer.finish(evidence()).unwrap();
    let reopened = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    assert!(!table_exists(&reopened, "catalog_ingest_arch_fragments").unwrap());
}

#[test]
fn arch_fragment_duplicates_orphans_and_unfinished_state_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let duplicate_path = root.path().join("duplicate.sqlite3");
    let duplicate = CatalogCandidateWriter::create(&duplicate_path, scope()).unwrap();
    duplicate
        .stage_arch_package_fragment(
            "pkg".to_string(),
            ArchPackageFragmentKind::Desc,
            "first".to_string(),
        )
        .unwrap();
    let duplicate_error = duplicate
        .stage_arch_package_fragment(
            "pkg".to_string(),
            ArchPackageFragmentKind::Desc,
            "second".to_string(),
        )
        .unwrap_err();
    assert!(
        duplicate_error
            .to_string()
            .contains("repeats desc metadata")
    );

    let orphan_path = root.path().join("orphan.sqlite3");
    let orphan = CatalogCandidateWriter::create(&orphan_path, scope()).unwrap();
    orphan
        .stage_arch_package_fragment(
            "pkg".to_string(),
            ArchPackageFragmentKind::Depends,
            "depends".to_string(),
        )
        .unwrap();
    let orphan_error = orphan.take_arch_package_record().unwrap_err();
    assert!(orphan_error.to_string().contains("without desc metadata"));

    let unfinished_path = root.path().join("unfinished.sqlite3");
    let unfinished = CatalogCandidateWriter::create(&unfinished_path, scope()).unwrap();
    unfinished
        .stage_arch_package_fragment(
            "pkg".to_string(),
            ArchPackageFragmentKind::Desc,
            "desc".to_string(),
        )
        .unwrap();
    let unfinished_error = unfinished.finish(evidence()).unwrap_err();
    assert!(
        unfinished_error
            .to_string()
            .contains("unfinished Arch package fragments")
    );
    assert!(!unfinished_path.exists());
}

#[test]
fn finalization_admits_exact_sqlite_page_facts_and_releases_lease() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.sqlite3");
    let lease_drops = Arc::new(AtomicUsize::new(0));
    let admission = Arc::new(RecordingAdmission {
        requirement: Mutex::new(None),
        lease_drops: Arc::clone(&lease_drops),
        refuse: false,
    });
    let writer =
        CatalogCandidateWriter::create_with_scratch_admission(&path, scope(), admission.clone())
            .unwrap();
    writer.finish(evidence()).unwrap();

    let requirement = admission.requirement.lock().unwrap().unwrap();
    assert_eq!(
        requirement.schema_version,
        CATALOG_FINALIZATION_SCRATCH_SCHEMA_V2
    );
    assert_eq!(requirement.database_page_size, 4096);
    assert!(requirement.database_page_count > 0);
    assert_eq!(
        requirement.database_bytes,
        requirement.database_page_size * requirement.database_page_count
    );
    assert_eq!(requirement.compacted_copy_bytes, requirement.database_bytes);
    assert_eq!(
        requirement.required_additional_bytes,
        requirement.database_bytes
    );
    assert_eq!(lease_drops.load(Ordering::SeqCst), 1);
    let entries = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![std::ffi::OsString::from("catalog.sqlite3")]);
}

#[test]
fn typed_refusal_precedes_vacuum_and_removes_private_candidate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.sqlite3");
    let admission = Arc::new(RecordingAdmission {
        requirement: Mutex::new(None),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse: true,
    });
    let writer =
        CatalogCandidateWriter::create_with_scratch_admission(&path, scope(), admission).unwrap();
    let error = writer.finish(evidence()).unwrap_err();

    assert!(matches!(error, Error::CatalogScratchCapacity(_)));
    assert!(!path.exists());
}
