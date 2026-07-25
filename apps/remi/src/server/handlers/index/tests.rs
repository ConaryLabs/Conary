// apps/remi/src/server/handlers/index/tests.rs
use super::*;
use crate::server::handlers::find_repository_for_distro;
use crate::server::native_publish::test_support::seed_native_publication;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{
    ConvertedPackage, Repository, RepositoryPackage, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup, Trove, TroveType,
};
use conary_core::db::schema;
use conary_core::repository::dependency_model::{
    RepositoryRequirementExpression, RepositoryRequirementKind,
};
use conary_core::repository::package_relation::parse_native_relation;
use conary_core::repository::versioning::VersionScheme;
use tempfile::NamedTempFile;

fn create_test_db() -> (NamedTempFile, Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

fn insert_converted_with_summary(
    conn: &Connection,
    distro: &str,
    package: &str,
    version: &str,
    architecture: Option<&str>,
    original_format: &str,
    summary: ScriptletBundleSummary,
) {
    let mut converted = ConvertedPackage::new_repository(
        distro.to_string(),
        package.to_string(),
        version.to_string(),
        original_format.to_string(),
        format!("sha256:{package}-{version}-source"),
        &[format!("sha256:{package}-{version}-chunk")],
        42,
        format!("sha256:{package}-{version}-content"),
        format!("/tmp/{package}-{version}.ccs"),
    );
    converted.package_architecture = architecture.map(str::to_string);
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(conn).unwrap();
}

fn insert_stale_conversion(
    conn: &Connection,
    distro: &str,
    package: &str,
    version: &str,
    architecture: Option<&str>,
    original_format: &str,
) {
    insert_converted_with_summary(
        conn,
        distro,
        package,
        version,
        architecture,
        original_format,
        ScriptletBundleSummary::default(),
    );
    conn.execute(
        "UPDATE converted_packages SET conversion_version = ?1
         WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4",
        rusqlite::params![
            conary_core::db::models::CONVERSION_VERSION - 1,
            distro,
            package,
            version
        ],
    )
    .unwrap();
}

#[test]
fn test_build_metadata_no_repository() {
    let (temp_file, _conn) = create_test_db();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert_eq!(metadata.id, "conary-fedora");
    assert_eq!(metadata.distro, "fedora");
    assert!(metadata.last_sync.is_none());
    assert_eq!(metadata.package_count, 0);
    assert_eq!(metadata.converted_count, 0);
    assert!(metadata.packages.is_empty());
}

#[test]
fn test_build_metadata_empty_repository() {
    let (temp_file, conn) = create_test_db();

    // Create a repository with default_strategy_distro
    let mut repo = Repository::new("fedora-base".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    repo.insert(&conn).unwrap();

    // Update with last_sync (not set during insert)
    repo.last_sync = Some("2026-01-21T12:00:00Z".to_string());
    repo.update(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert_eq!(metadata.distro, "fedora");
    assert_eq!(metadata.last_sync.as_deref(), Some("2026-01-21T12:00:00Z"));
    assert_eq!(metadata.package_count, 0);
    assert_eq!(metadata.converted_count, 0);
}

#[test]
fn metadata_wire_preserves_exact_typed_package_relation() {
    let (temp_file, conn) = create_test_db();
    let mut repository =
        Repository::new("fedora-base".to_string(), "https://example.com".to_string());
    repository.default_strategy_distro = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();
    let mut package = RepositoryPackage::new(
        repository_id,
        "newpkg".to_string(),
        "2".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:newpkg".to_string(),
        42,
        "https://example.com/newpkg.rpm".to_string(),
    );
    let package_id = package.insert(&conn).unwrap();
    let relation = parse_native_relation(
        RepositoryRequirementKind::Obsolete,
        VersionScheme::Rpm,
        "oldpkg < 2",
    )
    .unwrap();
    let mut stored = DbRequirementGroup::new(
        package_id,
        relation.kind.as_str().to_string(),
        "hard".to_string(),
        serde_json::to_string(&relation.expression).unwrap(),
    );
    stored.native_text = relation.native_text.clone();
    let group_id = stored.insert(&conn).unwrap();
    let clause = &relation.alternatives[0];
    RepositoryRequirement::new(
        package_id,
        group_id,
        clause.name.clone(),
        clause.version_constraint.clone(),
        "package".to_string(),
        "runtime".to_string(),
        clause.native_text.clone(),
    )
    .insert(&conn)
    .unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    let package = metadata
        .packages
        .iter()
        .find(|package| package.name == "newpkg")
        .unwrap();
    assert_eq!(package.requirement_groups.len(), 1);
    assert_eq!(package.requirement_groups[0].kind, "obsolete");
    assert_eq!(
        package.requirement_groups[0].native_text.as_deref(),
        Some("oldpkg < 2")
    );
    assert_eq!(
        serde_json::from_str::<RepositoryRequirementExpression>(
            &package.requirement_groups[0].expression_json,
        )
        .unwrap(),
        relation.expression
    );
}

#[test]
fn metadata_includes_native_only_package_as_native_not_converted() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "1",
        "noarch",
        "/tmp/hello.ccs",
    );

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let hello = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == "hello")
        .unwrap();

    assert_eq!(hello.version, "1.0.0");
    assert_eq!(hello.release.as_deref(), Some("1"));
    assert!(!hello.converted);
    assert_eq!(
        hello.metadata.as_ref().unwrap()["source_kind"],
        "native-ccs"
    );
}

#[test]
fn native_row_not_filtered_by_conversion_publication_gate() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "1",
        "noarch",
        "/tmp/hello.ccs",
    );

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let hello = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == "hello")
        .unwrap();

    assert!(!hello.converted);
    assert_eq!(
        hello.metadata.as_ref().unwrap()["source_kind"],
        "native-ccs"
    );
}

#[test]
fn test_build_metadata_with_packages() {
    let (temp_file, conn) = create_test_db();

    // Create repository
    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    // Add some packages
    let mut pkg1 = RepositoryPackage::new(
        repo_id,
        "nginx".to_string(),
        "1.24.0-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:abc".to_string(),
        1024,
        "https://example.com/nginx.rpm".to_string(),
    );
    pkg1.architecture = Some("x86_64".to_string());
    pkg1.insert(&conn).unwrap();

    let mut pkg2 = RepositoryPackage::new(
        repo_id,
        "curl".to_string(),
        "8.5.0-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:def".to_string(),
        512,
        "https://example.com/curl.rpm".to_string(),
    );
    pkg2.insert(&conn).unwrap();

    let mut pkg3 = RepositoryPackage::new(
        repo_id,
        "zlib".to_string(),
        "1.3.1-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:ghi".to_string(),
        256,
        "https://example.com/zlib.rpm".to_string(),
    );
    pkg3.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert_eq!(metadata.package_count, 3);
    assert_eq!(metadata.converted_count, 0);

    // Verify sorted by name
    assert_eq!(metadata.packages[0].name, "curl");
    assert_eq!(metadata.packages[1].name, "nginx");
    assert_eq!(metadata.packages[2].name, "zlib");
}

#[test]
fn test_build_metadata_preserves_repository_architecture() {
    let (temp_file, conn) = create_test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "qemu-img".to_string(),
        "2:10.1.0-7.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:qemu-img".to_string(),
        4096,
        "https://example.com/qemu-img.rpm".to_string(),
    );
    pkg.architecture = Some("x86_64".to_string());
    pkg.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let qemu_img = metadata
        .packages
        .iter()
        .find(|p| p.name == "qemu-img")
        .unwrap();
    let serialized = serde_json::to_value(qemu_img).unwrap();

    assert_eq!(serialized["architecture"], "x86_64");
}

#[test]
fn test_build_metadata_ignores_zero_sized_repository_rows() {
    let (temp_file, conn) = create_test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut placeholder = RepositoryPackage::new(
        repo_id,
        "qemu-img".to_string(),
        "10.1.0-7.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:placeholder".to_string(),
        0,
        "".to_string(),
    );
    placeholder.architecture = Some("x86_64".to_string());
    placeholder.insert(&conn).unwrap();

    let mut real_package = RepositoryPackage::new(
        repo_id,
        "qemu-img".to_string(),
        "2:10.1.0-7.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:qemu-img".to_string(),
        4096,
        "https://example.com/qemu-img.rpm".to_string(),
    );
    real_package.architecture = Some("x86_64".to_string());
    real_package.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert_eq!(metadata.package_count, 1);
    assert!(
        metadata
            .packages
            .iter()
            .any(|p| p.name == "qemu-img" && p.version == "2:10.1.0-7.fc44")
    );
    assert!(
        !metadata
            .packages
            .iter()
            .any(|p| p.name == "qemu-img" && p.version == "10.1.0-7.fc44")
    );
}

#[test]
fn test_build_metadata_with_converted_packages() {
    let (temp_file, conn) = create_test_db();

    // Create repository
    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    // Add packages
    let mut pkg1 = RepositoryPackage::new(
        repo_id,
        "nginx".to_string(),
        "1.24.0-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:abc".to_string(),
        1024,
        "https://example.com/nginx.rpm".to_string(),
    );
    pkg1.architecture = Some("x86_64".to_string());
    pkg1.insert(&conn).unwrap();

    let mut pkg2 = RepositoryPackage::new(
        repo_id,
        "curl".to_string(),
        "8.5.0-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:def".to_string(),
        512,
        "https://example.com/curl.rpm".to_string(),
    );
    pkg2.insert(&conn).unwrap();

    // Mark nginx as converted
    let mut converted = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "nginx".to_string(),
        "1.24.0-1.fc44".to_string(),
        "rpm".to_string(),
        "sha256:abc".to_string(),
        &["chunk1".to_string(), "chunk2".to_string()],
        2048,
        "sha256:content".to_string(),
        "/path/to/nginx.ccs".to_string(),
    );
    converted.package_architecture = Some("x86_64".to_string());
    converted.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert_eq!(metadata.package_count, 2);
    assert_eq!(metadata.converted_count, 1);

    // curl is not converted
    let curl = metadata.packages.iter().find(|p| p.name == "curl").unwrap();
    assert!(!curl.converted);

    // nginx is converted
    let nginx = metadata
        .packages
        .iter()
        .find(|p| p.name == "nginx")
        .unwrap();
    assert!(nginx.converted);
}

#[test]
fn test_build_metadata_with_converted_only_package() {
    let (temp_file, conn) = create_test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    repo.insert(&conn).unwrap();

    let mut converted = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "conary-test-fixture".to_string(),
        "1.0.0".to_string(),
        "ccs".to_string(),
        "upload:fedora:fixture".to_string(),
        &["fixture".to_string()],
        1277,
        "fixture-hash".to_string(),
        "/conary/cache/packages/conary-test-fixture-1.0.0.ccs".to_string(),
    );
    converted.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert_eq!(metadata.package_count, 1);
    assert_eq!(metadata.converted_count, 1);

    let fixture = metadata
        .packages
        .iter()
        .find(|p| p.name == "conary-test-fixture")
        .unwrap();
    assert_eq!(fixture.version, "1.0.0");
    assert!(fixture.converted);
    assert!(fixture.provides.is_empty());
    assert!(fixture.requirement_groups.is_empty());
}

#[test]
fn metadata_merges_public_scriptlet_metadata_for_repo_backed_and_converted_only_rows() {
    let (temp_file, conn) = create_test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut repo_backed_pkg = RepositoryPackage::new(
        repo_id,
        "repo-backed".to_string(),
        "1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:repo-backed".to_string(),
        2048,
        "https://example.com/repo-backed.rpm".to_string(),
    );
    repo_backed_pkg.architecture = Some("x86_64".to_string());
    repo_backed_pkg.insert(&conn).unwrap();

    let mut unconverted_pkg = RepositoryPackage::new(
        repo_id,
        "unconverted".to_string(),
        "1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:unconverted".to_string(),
        1024,
        "https://example.com/unconverted.rpm".to_string(),
    );
    unconverted_pkg.insert(&conn).unwrap();

    let mut repo_backed = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "repo-backed".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:repo-backed".to_string(),
        &["sha256:repo-backed-chunk".to_string()],
        2048,
        "sha256:repo-backed-content".to_string(),
        "/cache/repo-backed.ccs".to_string(),
    );
    repo_backed.package_architecture = Some("x86_64".to_string());
    repo_backed
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            scriptlet_fidelity: "native-lifecycle".to_string(),
            ..ScriptletBundleSummary::default()
        })
        .unwrap();
    repo_backed.insert(&conn).unwrap();

    let mut converted_only = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "converted-only".to_string(),
        "2.0".to_string(),
        "ccs".to_string(),
        "upload:fedora:converted-only".to_string(),
        &["sha256:converted-only-chunk".to_string()],
        4096,
        "sha256:converted-only-content".to_string(),
        "/cache/converted-only.ccs".to_string(),
    );
    converted_only
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            scriptlet_fidelity: "native-lifecycle".to_string(),
            ..ScriptletBundleSummary::default()
        })
        .unwrap();
    converted_only.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    let repo_backed = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == "repo-backed")
        .unwrap();
    assert!(repo_backed.converted);
    let repo_backed_scriptlets = repo_backed
        .metadata
        .as_ref()
        .unwrap()
        .get("scriptlets")
        .unwrap();
    assert_eq!(
        repo_backed_scriptlets
            .get("scriptlet_fidelity")
            .and_then(serde_json::Value::as_str),
        Some("native-lifecycle")
    );

    let converted_only = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == "converted-only")
        .unwrap();
    assert!(converted_only.converted);
    let converted_only_scriptlets = converted_only
        .metadata
        .as_ref()
        .unwrap()
        .get("scriptlets")
        .unwrap();
    assert_eq!(
        converted_only_scriptlets
            .get("scriptlet_fidelity")
            .and_then(serde_json::Value::as_str),
        Some("native-lifecycle")
    );

    let unconverted = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == "unconverted")
        .unwrap();
    assert!(!unconverted.converted);
    assert!(unconverted.metadata.is_none());
}

#[test]
fn metadata_hides_stale_scriptlet_rows() {
    let (temp_file, conn) = create_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "gtk3".to_string(),
        "3.24.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:repo".to_string(),
        1024,
        "https://example.com/gtk3.rpm".to_string(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.insert(&conn).unwrap();

    insert_stale_conversion(&conn, "fedora", "gtk3", "3.24.0", Some("x86_64"), "rpm");

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let pkg = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == "gtk3")
        .unwrap();

    assert!(!pkg.converted);
    assert_eq!(metadata.converted_count, 0);
    assert!(
        pkg.metadata
            .as_ref()
            .and_then(|value| value.get("scriptlets"))
            .is_none()
    );
}

#[test]
fn metadata_omits_converted_only_stale_rows() {
    let (temp_file, conn) = create_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    repo.insert(&conn).unwrap();

    insert_stale_conversion(&conn, "fedora", "stale-only", "1.0", Some("x86_64"), "ccs");

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert!(metadata.packages.iter().all(|pkg| pkg.name != "stale-only"));
    assert_eq!(metadata.converted_count, 0);
}

#[test]
fn test_build_metadata_omits_legacy_repo_converted_only_without_architecture() {
    let (temp_file, conn) = create_test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "qemu-img".to_string(),
        "2:10.1.0-7.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:qemu-img".to_string(),
        4096,
        "https://example.com/qemu-img.rpm".to_string(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.insert(&conn).unwrap();

    let mut stale_converted = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "qemu-img".to_string(),
        "10.1.0-7.fc44".to_string(),
        "rpm".to_string(),
        "sha256:old-qemu-img".to_string(),
        &["chunk".to_string()],
        2048,
        "sha256:content".to_string(),
        "/cache/qemu-img-10.1.0-7.fc44.ccs".to_string(),
    );
    stale_converted.insert(&conn).unwrap();

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert!(
        metadata
            .packages
            .iter()
            .any(|p| p.name == "qemu-img" && p.version == "2:10.1.0-7.fc44")
    );
    assert!(
        !metadata
            .packages
            .iter()
            .any(|p| p.name == "qemu-img" && p.version == "10.1.0-7.fc44")
    );
}

#[test]
fn test_find_repository_by_strategy_distro() {
    let (_temp_file, conn) = create_test_db();

    // Create repo with default_strategy_distro
    let mut repo = Repository::new(
        "my-fedora-repo".to_string(),
        "https://example.com".to_string(),
    );
    repo.default_strategy_distro = Some("fedora-44".to_string());
    repo.insert(&conn).unwrap();

    let found = find_repository_for_distro(&conn, "fedora").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "my-fedora-repo");
}

#[test]
fn repository_name_does_not_create_profile_authority() {
    let (_temp_file, conn) = create_test_db();

    // Create repo without default_strategy_distro but with matching name
    let mut repo = Repository::new("arch-linux".to_string(), "https://example.com".to_string());
    repo.insert(&conn).unwrap();

    let found = find_repository_for_distro(&conn, "arch").unwrap();
    assert!(found.is_none());
}

#[test]
fn exact_profile_identity_selects_repository() {
    let (_temp_file, conn) = create_test_db();

    // Create two repos - one with matching name, one with matching strategy_distro
    let mut repo1 = Repository::new(
        "debian-old".to_string(),
        "https://old.example.com".to_string(),
    );
    repo1.insert(&conn).unwrap();

    let mut repo2 = Repository::new(
        "my-deb-repo".to_string(),
        "https://new.example.com".to_string(),
    );
    repo2.default_strategy_distro = Some("ubuntu-26.04".to_string());
    repo2.insert(&conn).unwrap();

    // Should prefer the one with matching strategy_distro
    let found = find_repository_for_distro(&conn, "ubuntu").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "my-deb-repo");
}

#[test]
fn test_find_repository_not_found() {
    let (_temp_file, conn) = create_test_db();

    // Create unrelated repo
    let mut repo = Repository::new("centos".to_string(), "https://example.com".to_string());
    repo.insert(&conn).unwrap();

    let found = find_repository_for_distro(&conn, "fedora").unwrap();
    assert!(found.is_none());
}

#[test]
fn test_build_converted_packages() {
    let (_temp_file, conn) = create_test_db();

    // Add converted packages for different distros
    let mut fedora_pkg = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "nginx".to_string(),
        "1.24.0".to_string(),
        "rpm".to_string(),
        "sha256:fed1".to_string(),
        &[],
        1024,
        "sha256:c1".to_string(),
        "/path/1.ccs".to_string(),
    );
    fedora_pkg.package_architecture = Some("x86_64".to_string());
    fedora_pkg.insert(&conn).unwrap();

    let mut arch_pkg = ConvertedPackage::new_repository(
        "arch".to_string(),
        "nginx".to_string(),
        "1.24.0".to_string(),
        "arch".to_string(),
        "sha256:arch1".to_string(),
        &[],
        1024,
        "sha256:c2".to_string(),
        "/path/2.ccs".to_string(),
    );
    arch_pkg.package_architecture = Some("x86_64".to_string());
    arch_pkg.insert(&conn).unwrap();

    // Query for fedora - should only get fedora packages
    let fedora_set = build_converted_packages(&conn, "fedora").unwrap();
    assert_eq!(fedora_set.len(), 1);
    assert_eq!(fedora_set[0].name, "nginx");
    assert_eq!(fedora_set[0].version, "1.24.0");
    assert!(fedora_set[0].converted);

    // Query for arch - should only get arch packages
    let arch_set = build_converted_packages(&conn, "arch").unwrap();
    assert_eq!(arch_set.len(), 1);
    assert_eq!(arch_set[0].name, "nginx");
    assert_eq!(arch_set[0].version, "1.24.0");

    // Query for ubuntu - should be empty
    let ubuntu_set = build_converted_packages(&conn, "ubuntu").unwrap();
    assert!(ubuntu_set.is_empty());
}

#[test]
fn test_build_converted_packages_ignores_null_fields() {
    let (_temp_file, conn) = create_test_db();

    // Add a client-side converted package (no package_name/version/distro)
    let mut trove = Trove::new(
        "installed-client".to_string(),
        "1.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut client_pkg =
        ConvertedPackage::new_installed(trove_id, "rpm".to_string(), "sha256:client".to_string());
    client_pkg.insert(&conn).unwrap();

    // Add a server-side converted package
    let mut server_pkg = ConvertedPackage::new_repository(
        "fedora".to_string(),
        "curl".to_string(),
        "8.5.0".to_string(),
        "rpm".to_string(),
        "sha256:server".to_string(),
        &[],
        512,
        "sha256:c".to_string(),
        "/path/curl.ccs".to_string(),
    );
    server_pkg.package_architecture = Some("x86_64".to_string());
    server_pkg.insert(&conn).unwrap();

    let set = build_converted_packages(&conn, "fedora").unwrap();

    // Should only include the server-side package with non-null fields
    assert_eq!(set.len(), 1);
    assert_eq!(set[0].name, "curl");
    assert_eq!(set[0].version, "8.5.0");
}

#[test]
fn test_metadata_package_sorting() {
    let (temp_file, conn) = create_test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    // Add packages in non-alphabetical order
    for (name, version) in [
        ("zlib", "1.3.0"),
        ("acl", "2.3.2"),
        ("zlib", "1.2.0"), // older version
        ("bash", "5.2.0"),
    ] {
        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            version.to_string(),
            VersionScheme::Rpm,
            format!("sha256:{name}{version}"),
            100,
            format!("https://example.com/{name}.rpm"),
        );
        pkg.insert(&conn).unwrap();
    }

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    // Verify sorted by name, then version
    assert_eq!(metadata.packages[0].name, "acl");
    assert_eq!(metadata.packages[1].name, "bash");
    assert_eq!(metadata.packages[2].name, "zlib");
    assert_eq!(metadata.packages[2].version, "1.2.0"); // earlier version first (lexicographic)
    assert_eq!(metadata.packages[3].name, "zlib");
    assert_eq!(metadata.packages[3].version, "1.3.0");
}
