// crates/conary-core/src/db/models/trove/tests.rs

use super::*;

fn setup_test_db() -> (tempfile::TempDir, Connection) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();
    (temp_dir, conn)
}

#[test]
fn trove_round_trips_source_identity() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new_with_source(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        TroveType::Package,
        InstallSource::AdoptedTrack,
        VersionScheme::Arch,
    );
    trove.source_profile = Some("arch".to_string());
    trove.architecture = Some("x86_64".to_string());
    trove.native_package_identity =
        Some(InstalledPackageIdentity::pacman("bash", "bash", "5.2.37-1", "x86_64").unwrap());

    let trove_id = trove.insert(&conn).unwrap();
    let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();

    assert_eq!(loaded.source_profile.as_deref(), Some("arch"));
    assert_eq!(loaded.version_scheme, VersionScheme::Arch);
    assert_eq!(
        loaded
            .native_package_identity
            .as_ref()
            .map(InstalledPackageIdentity::selector),
        Some("bash")
    );
}

#[test]
fn native_owned_trove_requires_exact_package_identity() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new_with_source(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        TroveType::Package,
        InstallSource::AdoptedFull,
        VersionScheme::Arch,
    );

    let error = trove.insert(&conn).unwrap_err().to_string();
    assert!(error.contains("without an exact native package identity"));
}

#[test]
fn native_owned_trove_identity_must_match_version_scheme() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new_with_source(
        "curl".to_string(),
        "8.8.0-1".to_string(),
        TroveType::Package,
        InstallSource::AdoptedFull,
        VersionScheme::Debian,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.debian_multi_arch = Some(crate::repository::dependency_model::DebianMultiArch::No);
    trove.native_package_identity = Some(
        InstalledPackageIdentity::rpm("curl-8.8.0-1.x86_64", "curl", None, "8.8.0", "1", "x86_64")
            .unwrap(),
    );

    let error = trove.insert(&conn).unwrap_err().to_string();
    assert!(error.contains("identity uses rpm versioning"), "{error}");
    assert!(error.contains("trove's debian scheme"), "{error}");
}

#[test]
fn native_owned_trove_identity_must_match_name_version_and_architecture() {
    let (_dir, conn) = setup_test_db();
    let identity = InstalledPackageIdentity::dpkg(
        "libc6:i386",
        "libc6",
        "2.41-12",
        "i386",
        DebianMultiArch::Same,
    )
    .unwrap();

    for (name, version, architecture, expected) in [
        (
            "wrong-name",
            "2.41-12",
            Some("i386"),
            "identity names libc6",
        ),
        (
            "libc6",
            "2.41-11",
            Some("i386"),
            "canonical version 2.41-12",
        ),
        ("libc6", "2.41-12", Some("amd64"), "architecture i386"),
        ("libc6", "2.41-12", None, "architecture i386"),
    ] {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
            VersionScheme::Debian,
        );
        trove.architecture = architecture.map(str::to_string);
        trove.debian_multi_arch = Some(crate::repository::dependency_model::DebianMultiArch::Same);
        trove.native_package_identity = Some(identity.clone());

        let error = trove.insert(&conn).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn native_owned_trove_multi_arch_projection_must_match_identity() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new_with_source(
        "libc6".to_string(),
        "2.41-12".to_string(),
        TroveType::Package,
        InstallSource::AdoptedTrack,
        VersionScheme::Debian,
    );
    trove.architecture = Some("i386".to_string());
    trove.debian_multi_arch = Some(DebianMultiArch::No);
    trove.native_package_identity = Some(
        InstalledPackageIdentity::dpkg(
            "libc6:i386",
            "libc6",
            "2.41-12",
            "i386",
            DebianMultiArch::Same,
        )
        .unwrap(),
    );

    let error = trove.insert(&conn).unwrap_err().to_string();
    assert!(error.contains("Multi-Arch authority"), "{error}");
}

#[test]
fn new_trove_has_required_conary_version_scheme() {
    let trove = Trove::new(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    assert_eq!(trove.version_scheme, VersionScheme::Conary);
}

#[test]
fn insert_rejects_version_outside_declared_scheme() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new(
        "captured-root".to_string(),
        "snapshot".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );

    let error = trove.insert(&conn).unwrap_err().to_string();

    assert!(
        error.contains("invalid conary version 'snapshot'"),
        "{error}"
    );
    assert!(Trove::list_all(&conn).unwrap().is_empty());
}

#[test]
fn database_decode_rejects_unknown_installed_version_scheme() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new(
        "bash".to_string(),
        "5.2.37".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    conn.pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    conn.execute(
        "UPDATE troves SET version_scheme = 'unknown' WHERE id = ?1",
        [trove_id],
    )
    .unwrap();

    let error = Trove::find_by_id(&conn, trove_id).unwrap_err().to_string();
    assert!(
        error.contains("unsupported persisted native version scheme 'unknown'"),
        "{error}"
    );
}

#[test]
fn trove_update_replatform_metadata_sets_all_provenance_fields() {
    let (_dir, conn) = setup_test_db();
    let mut repo = crate::db::models::Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();
    let mut trove = Trove::new(
        "vim".to_string(),
        "9.1.0".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();

    Trove::update_replatform_metadata(
        &conn,
        trove_id,
        Some("arch"),
        VersionScheme::Arch,
        repo_id,
        "Replatformed from fedora-44 to arch by model apply",
    )
    .unwrap();

    let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
    assert_eq!(loaded.source_profile.as_deref(), Some("arch"));
    assert_eq!(loaded.version_scheme, VersionScheme::Arch);
    assert_eq!(loaded.installed_from_repository_id, Some(repo_id));
    assert_eq!(
        loaded.selection_reason.as_deref(),
        Some("Replatformed from fedora-44 to arch by model apply")
    );
}

#[test]
fn trove_update_selection_reason_overwrites_existing_reason() {
    let (_dir, conn) = setup_test_db();
    let mut trove = Trove::new(
        "vim".to_string(),
        "9.1.0".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();

    Trove::update_selection_reason(&conn, trove_id, "Replatform partial failure").unwrap();

    let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
    assert_eq!(
        loaded.selection_reason.as_deref(),
        Some("Replatform partial failure")
    );
}

#[test]
fn test_taken_variant_roundtrip() {
    let taken = InstallSource::Taken;
    let s = taken.as_str();
    assert_eq!(s, "taken");
    let parsed: InstallSource = s.parse().unwrap();
    assert_eq!(parsed, InstallSource::Taken);
}

#[test]
fn test_taken_is_conary_owned() {
    assert!(InstallSource::Taken.is_conary_owned());
    assert!(InstallSource::CapturedRoot.is_conary_owned());
    assert!(InstallSource::File.is_conary_owned());
    assert!(InstallSource::Repository.is_conary_owned());
    assert!(!InstallSource::AdoptedTrack.is_conary_owned());
    assert!(!InstallSource::AdoptedFull.is_conary_owned());
}

#[test]
fn test_taken_is_not_adopted() {
    assert!(!InstallSource::Taken.is_adopted());
    assert!(!InstallSource::CapturedRoot.is_adopted());
}

#[test]
fn complete_selected_root_sources_are_generation_inputs() {
    for source in [
        InstallSource::AdoptedFull,
        InstallSource::Taken,
        InstallSource::File,
        InstallSource::Repository,
        InstallSource::CapturedRoot,
    ] {
        assert!(source.is_generation_input(), "{source}");
    }
    assert!(!InstallSource::AdoptedTrack.is_generation_input());
}
