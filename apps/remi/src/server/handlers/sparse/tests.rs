// apps/remi/src/server/handlers/sparse/tests.rs

//! Sparse serving proofs for immutable profile catalogs.

use super::*;
use crate::server::catalog_authority::test_support::{
    ActiveCatalogFixture, package as catalog_package,
};
use crate::server::native_publish::test_support::seed_native_publication;
use axum::extract::{Path, State};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::repository::catalog::{
    CatalogPackageRecordV1, CatalogProvideRecordV1, CatalogRequirementAtomV1,
    CatalogRequirementGroupV1,
};
use conary_core::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryRequirementClause, RepositoryRequirementExpression,
};
use conary_core::repository::versioning::VersionScheme;
use std::sync::Arc;
use tokio::sync::RwLock;

fn remi_empty_db_state() -> (tempfile::TempDir, Arc<RwLock<ServerState>>) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("metadata/conary.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let config = crate::server::ServerConfig {
        db_path,
        chunk_dir: temp.path().join("chunks"),
        cache_dir: temp.path().join("cache"),
        catalog_dir: temp.path().join("catalogs"),
        catalog_candidate_dir: temp.path().join("catalog-candidates"),
        ..Default::default()
    };
    std::fs::create_dir_all(&config.chunk_dir).unwrap();
    std::fs::create_dir_all(&config.cache_dir).unwrap();
    std::fs::create_dir_all(&config.catalog_dir).unwrap();
    std::fs::create_dir_all(&config.catalog_candidate_dir).unwrap();
    let state = Arc::new(RwLock::new(
        crate::server::ServerState::new(config).unwrap(),
    ));
    (temp, state)
}

fn package(
    name: &str,
    version: &str,
    release: &str,
    architecture: Option<&str>,
    size: u64,
    marker: &str,
) -> CatalogPackageRecordV1 {
    catalog_package(
        "fedora-44",
        name,
        version,
        release,
        architecture,
        size,
        marker,
    )
}

fn activate(fixture: &ActiveCatalogFixture, packages: Vec<CatalogPackageRecordV1>) {
    fixture.activate("fedora-44", 1, packages);
}

fn sparse_entry(fixture: &ActiveCatalogFixture, name: &str) -> SparseIndexEntry {
    build_sparse_entry(fixture.authority(), fixture.db_path(), "fedora", name)
        .unwrap()
        .expect("catalog package should be visible")
}

#[tokio::test]
async fn sparse_entry_rejects_unsupported_distro_before_db_lookup() {
    let (_temp, state) = remi_empty_db_state();
    let response = get_sparse_entry(
        State(state),
        Path(("debian".to_string(), "bash".to_string())),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn catalog_only_entry_does_not_need_operational_package_rows() {
    let fixture = ActiveCatalogFixture::new();
    activate(
        &fixture,
        vec![
            package("curl", "8.5.0", "1", Some("x86_64"), 512, "curl"),
            package("nginx", "1.24.0", "1", Some("x86_64"), 1024, "nginx"),
        ],
    );

    let entry = sparse_entry(&fixture, "nginx");
    assert_eq!(entry.name, "nginx");
    assert_eq!(entry.versions.len(), 1);
    assert_eq!(entry.versions[0].version, "1.24.0");
}

#[test]
fn zero_sized_catalog_packages_are_excluded_from_entry() {
    let fixture = ActiveCatalogFixture::new();
    activate(
        &fixture,
        vec![
            package("placeholder", "0", "", Some("x86_64"), 0, "placeholder"),
            package("visible", "1.0", "1", Some("x86_64"), 1, "visible"),
        ],
    );

    assert!(
        build_sparse_entry(
            fixture.authority(),
            fixture.db_path(),
            "fedora",
            "placeholder",
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(sparse_entry(&fixture, "visible").versions.len(), 1);
}

fn rich_package(version: &str, marker: &str, advisory: &str) -> CatalogPackageRecordV1 {
    let mut package = package("openssl", version, "1", Some("x86_64"), 4096, marker);
    let requirement = RepositoryRequirementClause::name_only("libc".to_string());
    package.metadata = Some(
        serde_json::json!({
            "security_advisory": {
                "id": advisory,
                "source": "immutable-fixture",
                "fixed_version": version,
            }
        })
        .to_string(),
    );
    package.provides = vec![CatalogProvideRecordV1 {
        capability: format!("openssl = {version}"),
        version: Some(version.to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        kind: "package".to_string(),
        raw: Some(format!("openssl = {version}")),
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    package.requirement_groups = vec![CatalogRequirementGroupV1 {
        kind: "depends".to_string(),
        behavior: "hard".to_string(),
        description: Some("runtime".to_string()),
        native_text: Some("libc >= 2".to_string()),
        expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(requirement))
            .unwrap(),
        atoms: vec![CatalogRequirementAtomV1 {
            capability: "libc".to_string(),
            version_constraint: Some(">= 2".to_string()),
            kind: "package".to_string(),
            dependency_type: "runtime".to_string(),
            raw: Some("libc >= 2".to_string()),
        }],
    }];
    package
}

#[test]
fn sparse_projection_is_deterministic_for_versions_metadata_provides_and_requirements() {
    let fixture = ActiveCatalogFixture::new();
    activate(
        &fixture,
        vec![
            rich_package("2.0", "openssl-2", "FEDORA-2026-0002"),
            rich_package("1.0", "openssl-1", "FEDORA-2026-0001"),
        ],
    );

    let entry = sparse_entry(&fixture, "openssl");
    assert_eq!(
        entry
            .versions
            .iter()
            .map(|version| version.version.as_str())
            .collect::<Vec<_>>(),
        vec!["1.0", "2.0"]
    );
    assert_eq!(entry.versions[0].provides[0].capability, "openssl = 1.0");
    assert_eq!(entry.versions[0].requirement_groups.len(), 1);
    assert_eq!(
        entry.versions[0].requirement_groups[0].clauses[0].capability,
        "libc"
    );
    assert_eq!(entry, sparse_entry(&fixture, "openssl"));
}

#[test]
fn operational_repository_row_mutation_and_deletion_do_not_change_catalog_output() {
    let fixture = ActiveCatalogFixture::new();
    activate(
        &fixture,
        vec![package(
            "stable",
            "1.0",
            "1",
            Some("x86_64"),
            4096,
            "stable",
        )],
    );
    let before = sparse_entry(&fixture, "stable");

    let conn = fixture.connection();
    let mut repository = Repository::new(
        "mutable-operational-shadow".to_string(),
        "https://example.invalid/shadow".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();
    let mut shadow = RepositoryPackage::new(
        repository_id,
        "stable".to_string(),
        "9.9".to_string(),
        VersionScheme::Rpm,
        "deadbeef".to_string(),
        9999,
        "https://example.invalid/shadow.rpm".to_string(),
    );
    shadow.package_release = "9".to_string();
    shadow.architecture = Some("aarch64".to_string());
    shadow.source_profile = Some("fedora-44".to_string());
    let shadow_id = shadow.insert(&conn).unwrap();

    conn.execute(
        "UPDATE repository_packages SET name = 'mutated', version = '99.0', size = 0 WHERE id = ?1",
        [shadow_id],
    )
    .unwrap();
    assert_eq!(before, sparse_entry(&fixture, "stable"));

    conn.execute("DELETE FROM repository_packages WHERE id = ?1", [shadow_id])
        .unwrap();
    assert_eq!(before, sparse_entry(&fixture, "stable"));
}

#[test]
fn sparse_readers_fail_closed_without_an_active_catalog() {
    let fixture = ActiveCatalogFixture::new();

    assert!(
        build_sparse_entry(fixture.authority(), fixture.db_path(), "fedora", "missing",).is_err()
    );
}

#[test]
fn native_sibling_releases_do_not_dereference_repository_package_ids() {
    let fixture = ActiveCatalogFixture::new();
    activate(&fixture, Vec::new());
    let checksum_one = "sha256:native-sibling-1.0-1-noarch".to_string();
    let checksum_two = "sha256:native-sibling-1.0-2-noarch".to_string();

    let conn = fixture.connection();
    seed_native_publication(
        &conn,
        "fedora",
        "native-sibling",
        "1.0",
        "1",
        "noarch",
        "/tmp/native-sibling-1.ccs",
    );
    seed_native_publication(
        &conn,
        "fedora",
        "native-sibling",
        "1.0",
        "2",
        "noarch",
        "/tmp/native-sibling-2.ccs",
    );
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF; DELETE FROM repository_packages; PRAGMA foreign_keys = ON;",
    )
    .unwrap();

    let entry = sparse_entry(&fixture, "native-sibling");
    assert_eq!(entry.versions.len(), 2);
    assert_eq!(entry.versions[0].release.as_deref(), Some("1"));
    assert_eq!(entry.versions[1].release.as_deref(), Some("2"));
    assert_eq!(
        entry.versions[0].content_hash.as_deref(),
        Some(checksum_one.as_str())
    );
    assert_eq!(
        entry.versions[1].content_hash.as_deref(),
        Some(checksum_two.as_str())
    );
}

#[test]
fn native_publication_architecture_aliases_fail_as_duplicates() {
    let fixture = ActiveCatalogFixture::new();
    activate(&fixture, Vec::new());
    let conn = fixture.connection();
    seed_native_publication(
        &conn,
        "fedora",
        "native-alias",
        "1.0",
        "1",
        "x86_64",
        "/tmp/native-alias-1.ccs",
    );
    seed_native_publication(
        &conn,
        "fedora",
        "native-alias",
        "1.0",
        "1",
        " x86_64 ",
        "/tmp/native-alias-2.ccs",
    );

    let error = build_sparse_entry(
        fixture.authority(),
        fixture.db_path(),
        "fedora",
        "native-alias",
    )
    .expect_err("normalized duplicate publication identity must fail closed");
    assert!(
        error
            .to_string()
            .contains("multiple public native publications")
    );
}
