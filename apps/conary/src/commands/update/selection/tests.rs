// apps/conary/src/commands/update/selection/tests.rs

use super::*;
use crate::commands::test_helpers::create_test_db;
use conary_core::db::models::{
    CanonicalPackage, InstallSource, RepologyCacheEntry, Repository, RepositoryPackage,
    SecurityAdvisorySupport, Trove, TroveType,
};
use conary_core::repository::resolution_policy::{DependencyMixingPolicy, ResolutionPolicy};
use conary_core::repository::versioning::VersionScheme;

fn seed_cross_source_update_fixture(conn: &rusqlite::Connection) -> Trove {
    let mut fedora_repo = Repository::new(
        "fedora-main".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.priority = 50;
    fedora_repo.source_profile = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.priority = 10;
    arch_repo.source_profile = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(conn).unwrap();

    let mut canonical = CanonicalPackage::new("demo".to_string(), "package".to_string());
    let canonical_id = canonical.insert(conn).unwrap();
    let fresh = chrono::Utc::now().to_rfc3339();

    RepologyCacheEntry::insert_or_replace(
        conn,
        &RepologyCacheEntry {
            project_name: "demo".to_string(),
            distro: "fedora-44".to_string(),
            distro_name: "demo".to_string(),
            version: Some("1.1.0-1.fc44".to_string()),
            status: Some("outdated".to_string()),
            fetched_at: fresh.clone(),
        },
    )
    .unwrap();
    RepologyCacheEntry::insert_or_replace(
        conn,
        &RepologyCacheEntry {
            project_name: "demo".to_string(),
            distro: "arch".to_string(),
            distro_name: "demo".to_string(),
            version: Some("1.2.0-1".to_string()),
            status: Some("newest".to_string()),
            fetched_at: fresh,
        },
    )
    .unwrap();

    let mut installed = Trove::new_with_source(
        "demo".to_string(),
        "1.0.0-1.fc44".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    installed.architecture = Some("x86_64".to_string());
    installed.source_profile = Some("fedora-44".to_string());
    installed.installed_from_repository_id = Some(fedora_repo_id);
    installed.insert(conn).unwrap();

    let mut fedora_candidate = RepositoryPackage::new(
        fedora_repo_id,
        "demo".to_string(),
        "1.1.0-1.fc44".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:fedora-demo".to_string(),
        123,
        "https://example.test/fedora/demo-1.1.0-1.fc44.rpm".to_string(),
    );
    fedora_candidate.architecture = Some("x86_64".to_string());
    fedora_candidate.source_profile = Some("fedora-44".to_string());
    fedora_candidate.canonical_id = Some(canonical_id);
    fedora_candidate.insert(conn).unwrap();

    let mut arch_candidate = RepositoryPackage::new(
        arch_repo_id,
        "demo".to_string(),
        "1.2.0-1".to_string(),
        conary_core::repository::versioning::VersionScheme::Arch,
        "sha256:arch-demo".to_string(),
        123,
        "https://example.test/arch/demo-1.2.0-1.pkg.tar.zst".to_string(),
    );
    arch_candidate.architecture = Some("x86_64".to_string());
    arch_candidate.source_profile = Some("arch".to_string());
    arch_candidate.canonical_id = Some(canonical_id);
    arch_candidate.insert(conn).unwrap();

    installed
}

fn seed_security_update_fixture(
    conn: &rusqlite::Connection,
    support: SecurityAdvisorySupport,
    candidate_is_security_update: bool,
) -> Trove {
    let mut repo = Repository::new(
        "security-repo".to_string(),
        "https://example.test/security".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    repo.security_advisory_support = support;
    let repo_id = repo.insert(conn).unwrap();

    let mut installed = Trove::new_with_source(
        "openssl".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    installed.architecture = Some("x86_64".to_string());
    installed.source_profile = Some("fedora-44".to_string());
    installed.installed_from_repository_id = Some(repo_id);
    installed.insert(conn).unwrap();

    let mut candidate = RepositoryPackage::new(
        repo_id,
        "openssl".to_string(),
        "1.0.1".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:openssl".to_string(),
        123,
        "https://example.test/security/openssl-1.0.1.ccs".to_string(),
    );
    candidate.architecture = Some("x86_64".to_string());
    candidate.source_profile = Some("fedora-44".to_string());
    candidate.is_security_update = candidate_is_security_update;
    if candidate_is_security_update {
        candidate.severity = Some("important".to_string());
        candidate.advisory_id = Some("FEDORA-2026-0001".to_string());
    }
    candidate.insert(conn).unwrap();

    installed
}

fn seed_architecture_independent_update_fixture(
    conn: &rusqlite::Connection,
    name: &str,
    scheme: VersionScheme,
    profile: &str,
    architecture: &str,
) -> Trove {
    let mut repo = Repository::new(
        format!("{name}-repo"),
        format!("https://example.test/{name}"),
    );
    repo.source_profile = Some(profile.to_string());
    let repo_id = repo.insert(conn).unwrap();

    let mut installed = Trove::new_with_source(
        name.to_string(),
        "1.0-1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        scheme,
    );
    installed.architecture = Some(architecture.to_string());
    installed.debian_multi_arch = (scheme == VersionScheme::Debian)
        .then_some(conary_core::repository::dependency_model::DebianMultiArch::No);
    installed.source_profile = Some(profile.to_string());
    installed.installed_from_repository_id = Some(repo_id);
    installed.insert(conn).unwrap();

    let mut candidate = RepositoryPackage::new(
        repo_id,
        name.to_string(),
        "1.0-2".to_string(),
        scheme,
        format!("sha256:{name}"),
        123,
        format!("https://example.test/{name}/{name}-1.0-2"),
    );
    candidate.architecture = Some(architecture.to_string());
    candidate.debian_multi_arch = (scheme == VersionScheme::Debian)
        .then_some(conary_core::repository::dependency_model::DebianMultiArch::No);
    candidate.source_profile = Some(profile.to_string());
    candidate.insert(conn).unwrap();

    installed
}

#[test]
fn test_is_repo_version_newer_uses_debian_scheme() {
    let trove = Trove::new_with_source(
        "demo".to_string(),
        "1.0~beta1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Debian,
    );

    let candidate = RepositoryPackage::new(
        1,
        "demo".to_string(),
        "1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Debian,
        "sha256:demo".to_string(),
        1,
        "https://deb.example.test/demo_1.0_amd64.deb".to_string(),
    );

    assert!(is_repo_version_newer(&trove, &candidate).unwrap());
}

#[test]
fn test_is_repo_version_newer_uses_arch_scheme() {
    let trove = Trove::new_with_source(
        "demo".to_string(),
        "1.0-1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Arch,
    );

    let candidate = RepositoryPackage::new(
        1,
        "demo".to_string(),
        "1.0-2".to_string(),
        conary_core::repository::versioning::VersionScheme::Arch,
        "sha256:demo".to_string(),
        1,
        "https://arch.example.test/demo-1.0-2.pkg.tar.zst".to_string(),
    );

    assert!(is_repo_version_newer(&trove, &candidate).unwrap());
}

#[test]
fn architecture_independent_installed_variants_find_updates() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();

    for (name, scheme, profile, architecture) in [
        ("rpm-independent", VersionScheme::Rpm, "fedora-44", "noarch"),
        (
            "debian-independent",
            VersionScheme::Debian,
            "ubuntu-26.04",
            "all",
        ),
        ("alpm-independent", VersionScheme::Arch, "arch", "any"),
    ] {
        let installed = seed_architecture_independent_update_fixture(
            &conn,
            name,
            scheme,
            profile,
            architecture,
        );
        let selected = select_update_candidate(
            &conn,
            &installed,
            false,
            &ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict),
        )
        .unwrap()
        .expect("architecture-independent package should find its update");

        assert_eq!(selected.package.version, "1.0-2");
        assert_eq!(selected.package.architecture.as_deref(), Some(architecture));
    }
}

#[test]
fn selects_debian_update_for_explicitly_sourced_local_artifact() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut repo = Repository::new(
        "slice-d-local-update".to_string(),
        "http://127.0.0.1:18087".to_string(),
    );
    repo.priority = 500;
    repo.source_profile = Some("ubuntu-26.04".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut package = RepositoryPackage::new(
        repo_id,
        "phase4-runtime-fixture".to_string(),
        "1.0.1".to_string(),
        conary_core::repository::versioning::VersionScheme::Debian,
        "sha256:fixture".to_string(),
        1110,
        "http://127.0.0.1:18087/phase4-runtime-fixture_1.0.1_amd64.deb".to_string(),
    );
    package.architecture = Some("amd64".to_string());
    package.source_profile = Some("ubuntu-26.04".to_string());
    package.insert(&conn).unwrap();

    let mut installed = Trove::new(
        "phase4-runtime-fixture".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    installed.architecture = Some("amd64".to_string());
    installed.source_profile = Some("ubuntu-26.04".to_string());
    assert_eq!(installed.install_source, InstallSource::File);
    assert_eq!(installed.installed_from_repository_id, None);

    let selected = select_update_candidate(
        &conn,
        &installed,
        false,
        &ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict),
    )
    .unwrap()
    .expect("expected generic metadata-driven Debian update");

    assert_eq!(selected.package.version, "1.0.1");
    assert_eq!(selected.repository.name, "slice-d-local-update");
    assert_eq!(selected.package.version_scheme, VersionScheme::Debian);
}

#[test]
fn repology_latest_signal_cannot_switch_update_source() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let trove = seed_cross_source_update_fixture(&conn);
    let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);

    let selected = select_update_candidate(&conn, &trove, false, &policy)
        .unwrap()
        .expect("expected update candidate");

    assert_eq!(selected.repository.name, "fedora-main");
    assert_eq!(selected.package.version, "1.1.0-1.fc44");
}

#[test]
fn security_update_refuses_unknown_source_metadata_before_mutation() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Unknown, false);
    let policy = ResolutionPolicy::new();

    let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

    assert!(matches!(
        result,
        UpdateCandidateSelection::SecurityMetadataUnavailable(_)
    ));
}

#[test]
fn security_update_refuses_unsupported_source_metadata_before_mutation() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Unsupported, false);
    let policy = ResolutionPolicy::new();

    let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

    assert!(matches!(
        result,
        UpdateCandidateSelection::SecurityMetadataUnavailable(_)
    ));
}

#[test]
fn security_update_selects_supported_security_candidate() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Supported, true);
    let policy = ResolutionPolicy::new();

    let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

    assert!(matches!(result, UpdateCandidateSelection::Selected(_)));
}

#[test]
fn security_update_marker_includes_trusted_advisory_details() {
    let mut package = RepositoryPackage::new(
        7,
        "openssl".to_string(),
        "3.2.1-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:openssl-fixed".to_string(),
        4096,
        "https://example.test/openssl-3.2.1-1.fc44.ccs".to_string(),
    );
    package.is_security_update = true;
    package.severity = Some("critical".to_string());
    package.cve_ids = Some("CVE-2026-0001,CVE-2026-0002".to_string());
    package.advisory_id = Some("FEDORA-2026-0001".to_string());
    package.metadata = Some(
        serde_json::json!({
            "security_advisory": {
                "source": "conary-json",
                "source_trust": "trusted",
                "fixed_version": "3.2.1-1.fc44"
            }
        })
        .to_string(),
    );

    let marker = render_security_update_marker(&package);

    assert!(marker.contains("critical"), "{marker}");
    assert!(marker.contains("FEDORA-2026-0001"), "{marker}");
    assert!(marker.contains("CVE-2026-0001,CVE-2026-0002"), "{marker}");
    assert!(marker.contains("fixed: 3.2.1-1.fc44"), "{marker}");
    assert!(
        marker.contains("source: conary-json (feed trust claim: trusted)"),
        "{marker}"
    );
}

#[test]
fn security_update_ignores_supported_non_security_candidate() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Supported, false);
    let policy = ResolutionPolicy::new();

    let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

    assert!(matches!(result, UpdateCandidateSelection::NoEligibleUpdate));
}

#[test]
fn strict_mixing_update_stays_on_current_source() {
    let (_temp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let trove = seed_cross_source_update_fixture(&conn);
    let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict);

    let selected = select_update_candidate(&conn, &trove, false, &policy)
        .unwrap()
        .expect("expected strict-mixing update candidate");

    assert_eq!(selected.repository.name, "fedora-main");
    assert_eq!(selected.package.version, "1.1.0-1.fc44");
}
