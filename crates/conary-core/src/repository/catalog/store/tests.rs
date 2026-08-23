// crates/conary-core/src/repository/catalog/store/tests.rs

use std::io::{Read, Seek, SeekFrom, Write};

use super::*;
use crate::repository::catalog::SourceMetadataObjectRoleV1;
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::dependency_source::CapabilityProvenance;

fn source_scope() -> CatalogScopeV1 {
    CatalogScopeV1::Source {
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: "fedora-everything-x86_64".to_string(),
    }
}

fn evidence() -> Vec<CatalogSourceEvidenceV1> {
    vec![CatalogSourceEvidenceV1::AuthenticatedObject {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: "a".repeat(64),
        size: 4096,
    }]
}

fn package(name: &str, checksum: &str) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Source {
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        },
        source_profile: "fedora-44".to_string(),
        name: name.to_string(),
        version: "1.0-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some(format!("{name} package")),
        checksum: checksum.to_string(),
        size: 128,
        download_url: format!("https://example.test/{name}.rpm"),
        metadata: Some("{}".to_string()),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Rpm,
        provides: vec![CatalogProvideRecordV1 {
            capability: name.to_string(),
            version: Some("1.0-1".to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            kind: "package".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }],
        requirement_groups: vec![CatalogRequirementGroupV1 {
            kind: "depends".to_string(),
            behavior: "hard".to_string(),
            description: None,
            native_text: Some("glibc >= 2.39".to_string()),
            expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(
                RepositoryRequirementClause::versioned("glibc".to_string(), ">= 2.39".to_string()),
            ))
            .unwrap(),
            atoms: vec![CatalogRequirementAtomV1 {
                capability: "glibc".to_string(),
                version_constraint: Some(">= 2.39".to_string()),
                kind: "package".to_string(),
                dependency_type: "runtime".to_string(),
                raw: Some("glibc >= 2.39".to_string()),
            }],
        }],
    }
}

fn package_with_version_and_size(
    name: &str,
    version: &str,
    size: u64,
    checksum: &str,
) -> CatalogPackageRecordV1 {
    let mut package = package(name, checksum);
    package.version = version.to_string();
    package.size = size;
    package
}

#[test]
fn catalog_artifact_is_independent_of_input_order() {
    let directory = tempfile::tempdir().unwrap();
    let left_content = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![package("zlib", "b"), package("bash", "a")],
    )
    .unwrap();
    let right_content = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![package("bash", "a"), package("zlib", "b")],
    )
    .unwrap();
    assert_eq!(left_content, right_content);
    let left_path = directory.path().join("left.sqlite");
    let right_path = directory.path().join("right.sqlite");
    let left = write_catalog_candidate(&left_path, &left_content).unwrap();
    let right = write_catalog_candidate(&right_path, &right_content).unwrap();
    assert_eq!(left.artifact, right.artifact);
    assert_eq!(left.logical_digest_sha256, right.logical_digest_sha256);
    let reader = CatalogReader::open_verified(&left_path, &left).unwrap();
    assert_eq!(reader.packages().unwrap(), left_content.packages);
    assert_eq!(reader.source_evidence().unwrap(), evidence());
    assert_eq!(reader.find_packages_by_name("bash").unwrap().len(), 1);
    assert!(reader.contains_package_name("bash").unwrap());
    assert!(!reader.contains_package_name("Bash").unwrap());
    assert!(!reader.contains_package_name("missing").unwrap());
}

#[test]
fn duplicate_package_identity_is_rejected_even_when_checksum_changes() {
    let error = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![package("bash", "a"), package("bash", "b")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("repeats exact package key"));
}

#[test]
fn verified_reader_rejects_tamper_and_manifest_count_drift() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite");
    let content =
        CatalogContentV1::new(source_scope(), evidence(), vec![package("bash", "a")]).unwrap();
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let mut wrong_counts = binding.clone();
    wrong_counts.counts.packages += 1;
    assert!(CatalogReader::open_verified(&path, &wrong_counts).is_err());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(128)).unwrap();
    file.write_all(&[0xff]).unwrap();
    file.sync_all().unwrap();
    let error = CatalogReader::open_verified(&path, &binding)
        .err()
        .expect("tampered catalog must fail");
    assert!(error.to_string().contains("Checksum mismatch"));
}

#[test]
fn candidate_build_does_not_touch_adjacent_operational_database() {
    let directory = tempfile::tempdir().unwrap();
    let operational = directory.path().join("conary.db");
    fs::write(&operational, b"operational sentinel").unwrap();
    let before = hash_file(&operational).unwrap();
    let content =
        CatalogContentV1::new(source_scope(), evidence(), vec![package("bash", "a")]).unwrap();
    write_catalog_candidate(directory.path().join("catalog.sqlite"), &content).unwrap();
    assert_eq!(hash_file(&operational).unwrap(), before);
}

#[test]
fn reader_pages_distinct_downloadable_names_by_total_and_lexical_order() {
    let directory = tempfile::tempdir().unwrap();
    let content = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![
            package_with_version_and_size("delta", "1.0-1", 512, "delta"),
            package_with_version_and_size("alpha", "2.0-1", 256, "alpha-v2"),
            package_with_version_and_size("bravo", "1.0-1", 128, "bravo"),
            package_with_version_and_size("alpha", "1.0-1", 64, "alpha-v1"),
            package_with_version_and_size("charlie", "1.0-1", 1, "charlie"),
        ],
    )
    .unwrap();
    let path = directory.path().join("catalog.sqlite");
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let reader = CatalogReader::open_verified(&path, &binding).unwrap();

    let first = reader
        .find_downloadable_package_name_page(0, 2, 128)
        .unwrap();
    assert_eq!(
        first,
        CatalogPackageNamePageV1 {
            total: 3,
            names: vec!["alpha".to_string(), "bravo".to_string()],
        }
    );
    let second = reader
        .find_downloadable_package_name_page(2, 2, 128)
        .unwrap();
    assert_eq!(second.total, 3);
    assert_eq!(second.names, vec!["delta"]);
    let empty = reader
        .find_downloadable_package_name_page(3, 2, 128)
        .unwrap();
    assert_eq!(empty.total, 3);
    assert!(empty.names.is_empty());
}

#[test]
fn reader_name_page_rejects_zero_limit_and_sqlite_range_overflow() {
    let directory = tempfile::tempdir().unwrap();
    let content =
        CatalogContentV1::new(source_scope(), evidence(), vec![package("bash", "a")]).unwrap();
    let path = directory.path().join("catalog.sqlite");
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let reader = CatalogReader::open_verified(&path, &binding).unwrap();

    let error = reader
        .find_downloadable_package_name_page(0, 0, 1)
        .unwrap_err();
    assert!(error.to_string().contains("limit must be positive"));

    if let Some(overflow) = usize::try_from(i64::MAX)
        .ok()
        .and_then(|maximum| maximum.checked_add(1))
    {
        let error = reader
            .find_downloadable_package_name_page(overflow, 1, 1)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("offset exceeds SQLite integer range")
        );

        let error = reader
            .find_downloadable_package_name_page(0, overflow, 1)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("limit exceeds SQLite integer range")
        );
    }
}

#[test]
fn reader_name_page_order_is_deterministic_for_repeated_reads() {
    let directory = tempfile::tempdir().unwrap();
    let content = CatalogContentV1::new(
        source_scope(),
        evidence(),
        vec![
            package_with_version_and_size("zulu", "1.0-1", 100, "zulu"),
            package_with_version_and_size("alpha", "1.0-1", 100, "alpha"),
            package_with_version_and_size("echo", "1.0-1", 100, "echo"),
            package_with_version_and_size("bravo", "1.0-1", 100, "bravo"),
        ],
    )
    .unwrap();
    let path = directory.path().join("catalog.sqlite");
    let binding = write_catalog_candidate(&path, &content).unwrap();
    let reader = CatalogReader::open_verified(&path, &binding).unwrap();

    let expected = [
        "alpha".to_string(),
        "bravo".to_string(),
        "echo".to_string(),
        "zulu".to_string(),
    ];
    for offset in 0..expected.len() {
        let page = reader
            .find_downloadable_package_name_page(offset, 1, 100)
            .unwrap();
        assert_eq!(page.total, expected.len());
        assert_eq!(page.names, vec![expected[offset].clone()]);
    }
}

#[test]
fn logical_digest_streams_high_cardinality_package_relations() {
    const CHILD_ENV: &str = "CONARY_CATALOG_HIGH_CARDINALITY_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repository::catalog::store::tests::logical_digest_streams_high_cardinality_package_relations",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        std::io::stderr().write_all(&output.stderr).unwrap();
        assert!(
            output.status.success(),
            "high-cardinality digest child failed"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("CATALOG_RELATION_VM_HWM_KIB="),
            "high-cardinality digest child did not report VmHWM"
        );
        return;
    }

    const PROVIDES: usize = 175_000;
    const RSS_LIMIT_KIB: u64 = 192 * 1024;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("high-cardinality.sqlite");
    create_private_file(&path).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(&format!(
            "PRAGMA foreign_keys = ON;
             PRAGMA cache_size = -8192;
             PRAGMA application_id = {CATALOG_APPLICATION_ID};
             PRAGMA user_version = {CATALOG_CONTENT_SCHEMA_V1};"
        ))
        .unwrap();
    connection.execute_batch(CATALOG_SCHEMA).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();

    let scope = source_scope();
    let mut base = package("relation-bomb", &"b".repeat(64));
    base.provides.clear();
    base.requirement_groups.clear();
    base.canonicalize_for_scope(&scope).unwrap();
    insert_package_base(&connection, &base).unwrap();
    for ordinal in 0..PROVIDES {
        insert_provide(
            &connection,
            &base.package_key_sha256,
            checked_ordinal(ordinal, "provide").unwrap(),
            &CatalogProvideRecordV1 {
                capability: format!("generated-capability-{ordinal:06}"),
                version: None,
                version_relation: None,
                kind: "package".to_string(),
                raw: None,
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                provenance: CapabilityProvenance::AuthorDeclared,
            },
        )
        .unwrap();
    }
    connection.execute_batch("COMMIT").unwrap();

    let (_, counts) = digest_catalog_connection(&connection, &scope, &evidence()).unwrap();
    assert_eq!(counts.packages, 1);
    assert_eq!(counts.provides, PROVIDES as u64);

    let high_water_kib = vm_hwm_kib().unwrap();
    println!("CATALOG_RELATION_VM_HWM_KIB={high_water_kib}");
    assert!(
        high_water_kib < RSS_LIMIT_KIB,
        "VmHWM {high_water_kib} KiB exceeded fixed {RSS_LIMIT_KIB} KiB bound"
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
