// conary-core/src/resolver/provider/tests/installed_and_canonical.rs

use super::*;

#[test]
fn installed_capability_requirements_are_not_filtered_by_name_shape() {
    use crate::db::models::{Changeset, ChangesetStatus, TroveType};

    let (_dir, conn) = setup_test_db();

    let mut changeset = Changeset::new("Install consumer".to_string());
    changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let mut trove = Trove::new(
        "consumer".to_string(),
        "1.0-1".to_string(),
        TroveType::Package,
        VersionScheme::Rpm,
    );
    let trove_id = trove.insert(&conn).unwrap();

    for capability in ["/usr/bin/sh", "libcrypto.so.3()(64bit)"] {
        insert_installed_requirement(&conn, trove_id, VersionScheme::Rpm, capability);
    }

    let mut provider = ConaryProvider::new(&conn);
    provider.load_installed_packages().unwrap();

    let deps = provider.dependencies.values().next().unwrap();
    let names = deps
        .iter()
        .map(|dependency| dependency.as_single().unwrap().0)
        .collect::<Vec<_>>();
    assert!(names.contains(&"/usr/bin/sh"));
    assert!(names.contains(&"libcrypto.so.3()(64bit)"));
}

#[test]
fn installed_arch_package_in_provider_selection() {
    use crate::db::models::{Changeset, ChangesetStatus, TroveType};

    let (_dir, conn) = setup_test_db();

    let mut changeset = Changeset::new("Install glibc".to_string());
    changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let mut trove = Trove::new(
        "glibc".to_string(),
        "2.39-1".to_string(),
        TroveType::Package,
        VersionScheme::Arch,
    );
    trove.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider.load_installed_packages().unwrap();

    let solvable = provider
        .solvables
        .iter()
        .find(|solvable| solvable.name == "glibc")
        .unwrap();
    assert_eq!(solvable.version, "2.39-1");
    assert_eq!(solvable.version_scheme, VersionScheme::Arch);

    let constraint = ConaryConstraint::Repository {
        scheme: VersionScheme::Arch,
        constraint: RepoVersionConstraint::GreaterOrEqual("2.39".to_string()),
        raw: Some(">= 2.39".to_string()),
    };
    assert!(
        constraint_matches_package(&constraint, &solvable.version, solvable.version_scheme,)
            .expect("valid installed version")
    );
}

#[test]
fn native_ccs_package_uses_explicit_conary_version_scheme() {
    use crate::db::models::{Changeset, ChangesetStatus, TroveType};

    let (_dir, conn) = setup_test_db();

    let mut changeset = Changeset::new("Install bash".to_string());
    changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let mut trove = Trove::new(
        "bash".to_string(),
        "5.2.21-2.fc44".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider.load_installed_packages().unwrap();

    let solvable = provider
        .solvables
        .iter()
        .find(|solvable| solvable.name == "bash")
        .unwrap();
    assert_eq!(solvable.version_scheme, VersionScheme::Conary);
}

#[test]
fn canonical_index_loads_cross_distro_equivalents() {
    use crate::db::models::{CanonicalPackage, PackageImplementation};

    let (_dir, conn) = setup_test_db();

    let mut package = CanonicalPackage::new("openssl".to_string(), "package".to_string());
    let canonical_id = package.insert(&conn).unwrap();

    let mut fedora = PackageImplementation::new(
        canonical_id,
        "fedora".to_string(),
        "openssl".to_string(),
        "auto".to_string(),
    );
    fedora.insert(&conn).unwrap();

    let mut debian = PackageImplementation::new(
        canonical_id,
        "debian".to_string(),
        "libssl3".to_string(),
        "auto".to_string(),
    );
    debian.insert(&conn).unwrap();

    let mut arch = PackageImplementation::new(
        canonical_id,
        "arch".to_string(),
        "openssl".to_string(),
        "auto".to_string(),
    );
    arch.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider.load_canonical_index().unwrap();

    let equivalents = provider.canonical_equivalents("libssl3");
    assert!(
        equivalents.contains(&"openssl".to_string()),
        "libssl3 should have openssl as equivalent, got: {equivalents:?}"
    );

    let equivalents = provider.canonical_equivalents("openssl");
    assert!(
        equivalents.contains(&"libssl3".to_string()),
        "openssl should have libssl3 as equivalent, got: {equivalents:?}"
    );

    assert!(provider.canonical_equivalents("nonexistent").is_empty());
}

#[test]
fn canonical_index_empty_when_no_mappings() {
    let (_dir, conn) = setup_test_db();

    let mut provider = ConaryProvider::new(&conn);
    provider.load_canonical_index().unwrap();

    assert!(provider.canonical_equivalents("anything").is_empty());
}
