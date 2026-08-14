// src/commands/ccs/install/command_reinstall_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;

#[tokio::test]
async fn ccs_install_reinstall_dry_run_does_not_mutate_db() {
    use conary_core::ccs::{BuildResult, CcsManifest};

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("reinstall-dry-run.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    let conn = conary_core::db::open(db_path_str).unwrap();
    let mut existing = conary_core::db::models::Trove::new(
        "reinstall-dry-run".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    existing.architecture = Some(conary_core::repository::registry::detect_system_arch());
    let existing_id = existing.insert(&conn).unwrap();
    drop(conn);

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("reinstall-dry-run", "1.0.0"),
        components: HashMap::new(),
        files: Vec::new(),
        payloads: Vec::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        true,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        true,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let (trove_count, retained_id): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(id), -1) FROM troves WHERE name = 'reinstall-dry-run'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
    assert_eq!(
        retained_id, existing_id,
        "dry-run reinstall must not delete the existing installed trove"
    );
}

#[tokio::test]
async fn ccs_noarch_replacement_uses_transaction_trove_for_dependent_validation() {
    use conary_core::ccs::{BuildResult, CcsManifest, TrustPolicy};
    use conary_core::db::models::{InstalledRequirementGroup, ProvideEntry, Trove, TroveType};
    use conary_core::repository::dependency_model::{
        ProvideArchitectureQualifier, RepositoryCapabilityKind, RepositoryRequirementKind,
    };
    use conary_core::repository::versioning::VersionScheme;

    fn upgrade_trove_id(upgrade: &crate::commands::install::UpgradeCheck) -> i64 {
        match upgrade {
            crate::commands::install::UpgradeCheck::Upgrade(trove)
            | crate::commands::install::UpgradeCheck::Downgrade(trove)
            | crate::commands::install::UpgradeCheck::Replatform(trove) => {
                trove.id.expect("upgrade trove must have a database id")
            }
            crate::commands::install::UpgradeCheck::FreshInstall
            | crate::commands::install::UpgradeCheck::AlreadyInstalled(_) => {
                panic!("expected a replacing upgrade trove")
            }
        }
    }

    fn insert_provider(
        conn: &rusqlite::Connection,
        name: &str,
        version: &str,
        architecture: &str,
        capability: Option<&str>,
    ) -> i64 {
        let mut trove = Trove::new(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            VersionScheme::Conary,
        );
        trove.package_release = Some("1".to_string());
        trove.architecture = Some(architecture.to_string());
        let trove_id = trove.insert(conn).unwrap();

        let mut identity = ProvideEntry::new(
            trove_id,
            name.to_string(),
            Some(version.to_string()),
            VersionScheme::Conary,
        );
        identity.insert(conn).unwrap();
        if let Some(capability) = capability {
            let mut declared = ProvideEntry::new_typed(
                trove_id,
                RepositoryCapabilityKind::Generic,
                capability.to_string(),
                None,
                VersionScheme::Conary,
                ProvideArchitectureQualifier::Implicit,
            );
            declared.insert(conn).unwrap();
        }
        trove_id
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("same-name-noarch.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();

    let native_architecture = conary_core::repository::registry::detect_system_arch();
    let conn = conary_core::db::open(db_path_str).unwrap();
    // Insert the native slot first so the old first-compatible-row selection
    // chooses the wrong trove; the exact noarch rule must choose the second.
    let native_id = insert_provider(
        &conn,
        "same-name-provider",
        "1.0.0",
        &native_architecture,
        None,
    );
    let noarch_id = insert_provider(
        &conn,
        "same-name-provider",
        "2.0.0",
        conary_core::ccs::manifest::DEFAULT_CONARY_ARCHITECTURE,
        Some("review-capability"),
    );
    let existing = Trove::find_by_name(&conn, "same-name-provider").unwrap();
    assert_eq!(
        existing.iter().map(|trove| trove.id).collect::<Vec<_>>(),
        vec![Some(native_id), Some(noarch_id)],
        "fixture must preserve the DB order that made the old selector wrong"
    );

    let mut dependent = Trove::new(
        "same-name-dependent".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    dependent.architecture = Some(native_architecture.clone());
    let dependent_id = dependent.insert(&conn).unwrap();
    let requirement = conary_core::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Conary,
        "review-capability",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(
        &conn,
        dependent_id,
        VersionScheme::Conary,
        &[requirement],
    )
    .unwrap();

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("same-name-provider", "2.0.0"),
        components: HashMap::new(),
        files: Vec::new(),
        payloads: Vec::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);
    let trust_policy = TrustPolicy::from_file(&trust_policy_path).unwrap();
    let verification =
        conary_core::ccs::verify::verify_package(&package_path, &trust_policy).unwrap();
    let ccs_package = conary_core::ccs::CcsPackage::from_verified_archive(
        package_path.to_str().unwrap(),
        &verification,
    )
    .unwrap();

    let command_upgrade =
        super::command::check_ccs_install_upgrade_status(&conn, &ccs_package, true).unwrap();
    let semantics =
        crate::commands::install::install_semantics_for_ccs_manifest(ccs_package.manifest())
            .unwrap();
    let transaction_upgrade = crate::commands::install::check_ccs_upgrade_status(
        &conn,
        &ccs_package,
        &semantics,
        false,
        crate::commands::install::InstallIntent::PackageChange,
        true,
    )
    .unwrap();
    assert_eq!(upgrade_trove_id(&command_upgrade), noarch_id);
    assert_eq!(
        upgrade_trove_id(&command_upgrade),
        upgrade_trove_id(&transaction_upgrade),
        "command validation and transaction must replace the same trove"
    );
    drop(conn);

    let error = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        true,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        true,
    )
    .await
    .expect_err("dropping the exact noarch provider must fail dependent validation");
    assert!(
        error
            .to_string()
            .contains("same-name-dependent requires review-capability"),
        "unexpected validation error: {error:#}"
    );
}

#[tokio::test]
async fn default_authored_package_upgrades_with_explicit_architecture_authority() {
    use conary_core::ccs::{BuildResult, CcsManifest};
    use conary_core::db::models::InstalledRequirementGroup;
    use conary_core::repository::dependency_model::RepositoryRequirementKind;
    use conary_core::repository::versioning::VersionScheme;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    super::test_support::stage_test_boot_assets(temp_dir.path());
    super::test_support::seed_test_init_trove(db_path_str, temp_dir.path());

    let package_result = |version: &str| {
        let mut manifest = CcsManifest::new_minimal("authored-upgrade", version);
        manifest.requirements.push(
            conary_core::repository::requirement::parse_native_requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Conary,
                "test-init",
            )
            .unwrap(),
        );
        BuildResult {
            manifest,
            components: HashMap::new(),
            files: Vec::new(),
            payloads: Vec::new(),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        }
    };

    let first_package = temp_dir.path().join("authored-upgrade-1.ccs");
    let first_policy =
        super::test_support::write_signed_test_package(&package_result("1.0.0"), &first_package);
    cmd_ccs_install(
        first_package.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(first_policy.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let installed =
        conary_core::db::models::Trove::find_by_name(&conn, "authored-upgrade").unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(
        installed[0].architecture.as_deref(),
        Some(conary_core::ccs::manifest::DEFAULT_CONARY_ARCHITECTURE)
    );
    assert_eq!(
        InstalledRequirementGroup::find_by_trove(&conn, installed[0].id.unwrap())
            .unwrap()
            .len(),
        1,
        "the replacement preflight must evaluate a persisted requirement"
    );
    drop(conn);

    let second_package = temp_dir.path().join("authored-upgrade-2.ccs");
    let second_policy =
        super::test_support::write_signed_test_package(&package_result("2.0.0"), &second_package);
    cmd_ccs_install(
        second_package.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(second_policy.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let installed =
        conary_core::db::models::Trove::find_by_name(&conn, "authored-upgrade").unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version, "2.0.0");
    assert_eq!(
        installed[0].architecture.as_deref(),
        Some(conary_core::ccs::manifest::DEFAULT_CONARY_ARCHITECTURE)
    );
}
