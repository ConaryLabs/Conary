// apps/conary/src/commands/model/apply/tests.rs

use super::super::context::compute_model_diff;
use super::super::test_support::{
    ReplatformMetadataFailpointReset, build_test_ccs_package, build_test_ccs_package_with_bundle,
    serve_test_file, typed_rpm_replatform_upgrade_bundle,
};
use super::*;
use crate::commands::test_helpers::create_test_db;
use conary_core::db::models::{DistroPin, InstalledNativeLifecycleBundle, settings};
use conary_core::model::capture_current_state;
use conary_core::model::parser::SystemModel;
use conary_core::repository::{SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE};
use tempfile::tempdir;

#[tokio::test]
async fn test_model_apply_updates_source_policy_without_package_changes() {
    let (_temp_file, db_path) = create_test_db();
    let model_dir = tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
    )
    .unwrap();

    cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: "/",
        dry_run: false,
        skip_optional: false,
        strict: false,
        autoremove: false,
        offline: true,
    })
    .await
    .unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let pin = DistroPin::get_current(&conn).unwrap().unwrap();
    assert_eq!(pin.distro, "arch");
    assert_eq!(pin.mixing_policy, "strict");
}

#[tokio::test]
async fn test_model_apply_updates_selection_mode_without_package_changes() {
    let (_temp_file, db_path) = create_test_db();
    let model_dir = tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1

[system]
selection_mode = "latest"
"#,
    )
    .unwrap();

    cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: "/",
        dry_run: false,
        skip_optional: false,
        strict: false,
        autoremove: false,
        offline: true,
    })
    .await
    .unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        settings::get(&conn, SETTINGS_KEY_SELECTION_MODE).unwrap(),
        Some("latest".to_string())
    );
    assert!(DistroPin::get_current(&conn).unwrap().is_none());
}

#[tokio::test]
async fn test_model_apply_updates_allowed_distros_without_package_changes() {
    let (_temp_file, db_path) = create_test_db();
    let model_dir = tempdir().unwrap();
    let model_path = model_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1

[system]
allowed_distros = ["arch"]
"#,
    )
    .unwrap();

    cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: "/",
        dry_run: false,
        skip_optional: false,
        strict: false,
        autoremove: false,
        offline: true,
    })
    .await
    .unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        settings::get(&conn, SETTINGS_KEY_ALLOWED_DISTROS).unwrap(),
        Some("[\"arch\"]".to_string())
    );
    assert!(DistroPin::get_current(&conn).unwrap().is_none());
}

#[tokio::test]
async fn test_model_apply_executes_replatform_replacement_when_route_is_executable() {
    use conary_core::db::models::{
        DistroPin, InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
        RepositoryPackage, ResolutionStrategy, Trove, TroveType,
    };

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let package_path = build_test_ccs_package(temp_dir.path(), "vim", "9.1.0");
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    DistroPin::set(&conn, "fedora-44", "strict").unwrap();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.repository_id = Some(fedora_repo_id);
    let fedora_label_id = fedora_label.insert(&conn).unwrap();

    let mut installed = Trove::new_with_source(
        "vim".to_string(),
        "9.0.1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    installed.label_id = Some(fedora_label_id);
    installed.architecture = Some("x86_64".to_string());
    installed.source_distro = Some("fedora-44".to_string());
    installed.installed_from_repository_id = Some(fedora_repo_id);
    installed.insert(&conn).unwrap();

    let mut arch_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Arch,
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    arch_pkg.architecture = Some("x86_64".to_string());
    arch_pkg.distro = Some("arch".to_string());
    arch_pkg.insert(&conn).unwrap();

    let mut exact_resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    exact_resolution.version = Some("9.1.0".to_string());
    exact_resolution.primary_strategy = PrimaryStrategy::Binary;
    exact_resolution.insert(&conn).unwrap();
    drop(conn);

    let model_path = temp_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
    )
    .unwrap();

    let result = cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        skip_optional: false,
        strict: false,
        autoremove: false,
        offline: true,
    });

    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let result = result.await;

    result.unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
    assert_eq!(installed_troves.len(), 1);
    let installed = &installed_troves[0];
    assert_eq!(installed.version, "9.1.0");
    assert_eq!(installed.source_distro.as_deref(), Some("arch"));
    assert_eq!(
        installed.version_scheme,
        conary_core::repository::versioning::VersionScheme::Arch
    );
    assert_eq!(installed.installed_from_repository_id, Some(arch_repo_id));
    assert_eq!(
        installed.selection_reason.as_deref(),
        Some("Replatformed from fedora-44 to arch by model apply")
    );
    assert_eq!(
        DistroPin::get_current(&conn).unwrap().unwrap().distro,
        "arch"
    );
}

#[tokio::test]
async fn test_model_apply_replatform_executes_typed_rpm_lifecycle() {
    use conary_core::db::models::{
        InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
        RepositoryPackage, ResolutionStrategy, Trove, TroveType,
    };

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let package_path = build_test_ccs_package_with_bundle(
        temp_dir.path(),
        "vim",
        "9.1.0",
        Some(typed_rpm_replatform_upgrade_bundle("vim", "9.1.0")),
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    DistroPin::set(&conn, "fedora-44", "strict").unwrap();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.repository_id = Some(fedora_repo_id);
    let fedora_label_id = fedora_label.insert(&conn).unwrap();

    let mut installed = Trove::new_with_source(
        "vim".to_string(),
        "9.0.1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    installed.label_id = Some(fedora_label_id);
    installed.architecture = Some("x86_64".to_string());
    installed.source_distro = Some("fedora-44".to_string());
    installed.installed_from_repository_id = Some(fedora_repo_id);
    installed.insert(&conn).unwrap();

    let mut arch_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Arch,
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    arch_pkg.architecture = Some("x86_64".to_string());
    arch_pkg.distro = Some("arch".to_string());
    arch_pkg.insert(&conn).unwrap();

    let mut exact_resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    exact_resolution.version = Some("9.1.0".to_string());
    exact_resolution.primary_strategy = PrimaryStrategy::Binary;
    exact_resolution.insert(&conn).unwrap();

    let model: SystemModel = toml::from_str(
        r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
    )
    .unwrap();

    let state = capture_current_state(&conn).unwrap();
    let diff = compute_model_diff(&model, &state, &conn, true, false)
        .await
        .unwrap();
    let action_refs = diff.actions.iter().collect::<Vec<_>>();
    apply_source_policy_changes(&conn, &action_refs).unwrap();
    drop(conn);

    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let (executed, errors) =
        apply_replatform_changes(&db_path, install_root.to_str().unwrap(), &action_refs)
            .await
            .unwrap();

    assert_eq!(executed, 1);
    assert!(
        errors.is_empty(),
        "unexpected replatform errors: {errors:?}"
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
    assert_eq!(installed_troves.len(), 1);
    let installed = &installed_troves[0];
    assert_eq!(installed.version, "9.1.0");
    assert_eq!(installed.source_distro.as_deref(), Some("arch"));
    assert_eq!(
        installed.version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
    assert_eq!(installed.installed_from_repository_id, Some(arch_repo_id));
    assert_eq!(
        InstalledNativeLifecycleBundle::find_by_trove(
            &conn,
            installed.id.expect("installed trove id")
        )
        .unwrap()
        .expect("typed lifecycle bundle must persist")
        .source_format,
        "rpm"
    );
}

#[tokio::test]
async fn test_model_apply_rolls_back_or_reports_partial_failure_during_replatform() {
    use conary_core::db::models::{
        InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
        RepositoryPackage, ResolutionStrategy, Trove, TroveType,
    };

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let package_path = build_test_ccs_package(temp_dir.path(), "vim", "9.1.0");
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let conn = rusqlite::Connection::open(&db_path).unwrap();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.repository_id = Some(fedora_repo_id);
    let fedora_label_id = fedora_label.insert(&conn).unwrap();

    let mut installed = Trove::new_with_source(
        "vim".to_string(),
        "9.0.1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    installed.label_id = Some(fedora_label_id);
    installed.architecture = Some("x86_64".to_string());
    installed.source_distro = Some("fedora-44".to_string());
    installed.installed_from_repository_id = Some(fedora_repo_id);
    installed.insert(&conn).unwrap();

    let mut arch_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Arch,
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    arch_pkg.architecture = Some("x86_64".to_string());
    arch_pkg.distro = Some("arch".to_string());
    arch_pkg.insert(&conn).unwrap();

    let mut exact_resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    exact_resolution.version = Some("9.1.0".to_string());
    exact_resolution.primary_strategy = PrimaryStrategy::Binary;
    exact_resolution.insert(&conn).unwrap();

    let model: SystemModel = toml::from_str(
        r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
    )
    .unwrap();

    let state = capture_current_state(&conn).unwrap();
    let diff = compute_model_diff(&model, &state, &conn, true, false)
        .await
        .unwrap();
    drop(conn);

    set_replatform_metadata_failpoint_for_test(true);
    let _reset = ReplatformMetadataFailpointReset;

    let action_refs = diff.actions.iter().collect::<Vec<_>>();
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let (executed, errors) =
        apply_replatform_changes(&db_path, install_root.to_str().unwrap(), &action_refs)
            .await
            .unwrap();

    assert_eq!(executed, 0);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].contains("failed to finalize replatform metadata"),
        "expected explicit execution failure, got: {}",
        errors[0]
    );
    assert!(
        !errors[0].contains("blocked"),
        "execution failure should not be reported as blocked: {}",
        errors[0]
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
    assert_eq!(installed_troves.len(), 1);
    let installed = &installed_troves[0];
    assert_eq!(installed.version, "9.1.0");
    assert_eq!(installed.source_distro.as_deref(), Some("arch"));
    assert_eq!(
        installed.version_scheme,
        conary_core::repository::versioning::VersionScheme::Arch
    );
    assert_eq!(installed.installed_from_repository_id, Some(arch_repo_id));
    assert_eq!(
        installed.selection_reason.as_deref(),
        Some("Replatform partial failure after install: injected replatform metadata failure")
    );
}
