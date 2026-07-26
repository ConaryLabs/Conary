// apps/conary/src/commands/model/apply/tests.rs

use super::super::context::compute_model_diff;
use super::super::test_support::{
    ReplatformMetadataFailpointReset, build_test_ccs_package,
    insert_test_repository_package_resolution, insert_test_static_ccs_repository, serve_test_file,
    typed_rpm_replatform_upgrade_bundle,
};
use super::*;
use crate::commands::test_helpers::create_test_db;
use conary_core::db::models::{DistroPin, InstalledNativeLifecycleBundle, settings};
use conary_core::model::capture_current_state;
use conary_core::model::parser::SystemModel;
use conary_core::repository::SETTINGS_KEY_ALLOWED_DISTROS;
use conary_core::repository::resolution_policy::DependencyMixingPolicy;
use tempfile::tempdir;

fn replatform_variant(architecture: Option<&str>, marker: &str) -> Trove {
    let mut trove = Trove::new(
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    trove.architecture = architecture.map(str::to_string);
    trove.description = Some(marker.to_string());
    trove.source_profile = Some("fedora-44".to_string());
    trove
}

#[test]
fn replatform_variant_prefers_exact_architecture_over_independent_fallback() {
    let matches = vec![
        replatform_variant(None, "noarch"),
        replatform_variant(Some("x86_64"), "exact"),
    ];

    let selected = select_replatform_variant(&matches, Some("x86_64"), "vim")
        .unwrap()
        .unwrap();
    assert_eq!(selected.description.as_deref(), Some("exact"));
}

#[test]
fn replatform_variant_uses_one_architecture_independent_fallback() {
    let matches = vec![
        replatform_variant(Some("aarch64"), "incompatible"),
        replatform_variant(None, "noarch"),
    ];

    let selected = select_replatform_variant(&matches, Some("x86_64"), "vim")
        .unwrap()
        .unwrap();
    assert_eq!(selected.description.as_deref(), Some("noarch"));
}

#[test]
fn replatform_variant_fails_closed_on_ambiguous_compatible_fallback() {
    let matches = vec![
        replatform_variant(None, "noarch-a"),
        replatform_variant(None, "noarch-b"),
    ];

    let error = select_replatform_variant(&matches, Some("x86_64"), "vim")
        .expect_err("multiple compatible fallbacks must be rejected");
    assert!(
        error
            .to_string()
            .contains("multiple architecture-independent matches")
    );
}

#[test]
fn replatform_absent_architecture_selects_only_unambiguous_variant() {
    let matches = vec![replatform_variant(Some("x86_64"), "concrete")];

    let selected = select_replatform_variant(&matches, None, "vim")
        .unwrap()
        .unwrap();
    assert_eq!(selected.description.as_deref(), Some("concrete"));
}

#[test]
fn replatform_absent_architecture_fails_closed_on_multiple_variants() {
    let matches = vec![
        replatform_variant(Some("x86_64"), "x86_64"),
        replatform_variant(Some("aarch64"), "aarch64"),
    ];

    let error = select_replatform_variant(&matches, None, "vim")
        .expect_err("missing selector must not choose an arbitrary architecture");
    assert!(
        error
            .to_string()
            .contains("multiple installed variants but no architecture selector")
    );
}

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
    assert_eq!(pin.mixing_policy, DependencyMixingPolicy::Strict);
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
    const TEST_NAME: &str = "commands::model::apply::tests::test_model_apply_executes_replatform_replacement_when_route_is_executable";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    use conary_core::db::models::{
        DistroPin, InstallSource, LabelEntry, Repository, RepositoryPackage, Trove, TroveType,
    };

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let package_path = build_test_ccs_package(
        temp_dir.path(),
        "vim",
        "9.1.0",
        conary_core::repository::versioning::VersionScheme::Arch,
        None,
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Strict).unwrap();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.source_profile = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let arch_repo_id =
        insert_test_static_ccs_repository(&conn, "arch-core", "https://example.test/arch", "arch");

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
    installed.source_profile = Some("fedora-44".to_string());
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
    arch_pkg.source_profile = Some("arch".to_string());
    let arch_package_id = arch_pkg.insert(&conn).unwrap();
    insert_test_repository_package_resolution(&conn, arch_repo_id, arch_package_id, "vim", "9.1.0");
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
    assert_eq!(installed.source_profile.as_deref(), Some("arch"));
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
    const TEST_NAME: &str =
        "commands::model::apply::tests::test_model_apply_replatform_executes_typed_rpm_lifecycle";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    use conary_core::db::models::{
        InstallSource, LabelEntry, Repository, RepositoryPackage, Trove, TroveType,
    };

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let package_path = build_test_ccs_package(
        temp_dir.path(),
        "vim",
        "9.1.0",
        conary_core::repository::versioning::VersionScheme::Rpm,
        Some(typed_rpm_replatform_upgrade_bundle("vim", "9.1.0")),
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    DistroPin::set(&conn, "arch", DependencyMixingPolicy::Strict).unwrap();

    let mut arch_repo =
        Repository::new("arch".to_string(), "https://example.test/arch".to_string());
    arch_repo.source_profile = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let fedora_repo_id = insert_test_static_ccs_repository(
        &conn,
        "fedora",
        "https://example.test/fedora",
        "fedora-44",
    );

    let mut arch_label = LabelEntry::new(
        "arch".to_string(),
        "rolling".to_string(),
        "stable".to_string(),
    );
    arch_label.repository_id = Some(arch_repo_id);
    let arch_label_id = arch_label.insert(&conn).unwrap();

    let mut installed = Trove::new_with_source(
        "vim".to_string(),
        "9.0.1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Arch,
    );
    installed.label_id = Some(arch_label_id);
    installed.architecture = Some("x86_64".to_string());
    installed.source_profile = Some("arch".to_string());
    installed.installed_from_repository_id = Some(arch_repo_id);
    installed.insert(&conn).unwrap();

    let mut fedora_pkg = RepositoryPackage::new(
        fedora_repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        package_checksum.clone(),
        std::fs::metadata(&package_path)
            .unwrap()
            .len()
            .try_into()
            .unwrap(),
        package_url.clone(),
    );
    fedora_pkg.architecture = Some("x86_64".to_string());
    fedora_pkg.source_profile = Some("fedora-44".to_string());
    let fedora_package_id = fedora_pkg.insert(&conn).unwrap();
    insert_test_repository_package_resolution(
        &conn,
        fedora_repo_id,
        fedora_package_id,
        "vim",
        "9.1.0",
    );

    let model: SystemModel = toml::from_str(
        r#"
[model]
version = 1

[system.pin]
distro = "fedora-44"
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

    assert_eq!(
        executed, 1,
        "typed RPM replatform should execute: {errors:?}"
    );
    assert!(
        errors.is_empty(),
        "unexpected replatform errors: {errors:?}"
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
    assert_eq!(installed_troves.len(), 1);
    let installed = &installed_troves[0];
    assert_eq!(installed.version, "9.1.0");
    assert_eq!(installed.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(
        installed.version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
    assert_eq!(installed.installed_from_repository_id, Some(fedora_repo_id));
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
    const TEST_NAME: &str = "commands::model::apply::tests::test_model_apply_rolls_back_or_reports_partial_failure_during_replatform";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    use conary_core::db::models::{
        InstallSource, LabelEntry, Repository, RepositoryPackage, Trove, TroveType,
    };

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let package_path = build_test_ccs_package(
        temp_dir.path(),
        "vim",
        "9.1.0",
        conary_core::repository::versioning::VersionScheme::Arch,
        None,
    );
    let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
    let (package_url, _server_handle) = serve_test_file(package_path.clone());

    let conn = rusqlite::Connection::open(&db_path).unwrap();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.source_profile = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let arch_repo_id =
        insert_test_static_ccs_repository(&conn, "arch-core", "https://example.test/arch", "arch");

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
    installed.source_profile = Some("fedora-44".to_string());
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
    arch_pkg.source_profile = Some("arch".to_string());
    let arch_package_id = arch_pkg.insert(&conn).unwrap();
    insert_test_repository_package_resolution(&conn, arch_repo_id, arch_package_id, "vim", "9.1.0");

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
    assert_eq!(installed.source_profile.as_deref(), Some("arch"));
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
