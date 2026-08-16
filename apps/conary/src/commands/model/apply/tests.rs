// apps/conary/src/commands/model/apply/tests.rs

use super::super::test_support::{
    ReplatformMetadataFailpointReset, build_test_ccs_package,
    build_test_ccs_package_with_payloads_and_relations, insert_test_repository_package_resolution,
    insert_test_static_ccs_repository, serve_test_file, typed_rpm_model_post_helper_bundle,
    typed_rpm_replatform_upgrade_bundle,
};
use super::*;
use crate::commands::test_helpers::create_test_db;
use conary_core::db::models::InstalledNativeLifecycleBundle;
use tempfile::tempdir;

fn published_or_pending_generation_has_path(db_path: &std::path::Path, path: &str) -> bool {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    if let Some(generation) =
        conary_core::generation::mount::current_generation(runtime_root.root()).unwrap()
    {
        let artifact = conary_core::generation::artifact::load_generation_artifact(
            &runtime_root.generation_path(generation),
        )
        .unwrap();
        return artifact
            .generation_root
            .entries
            .iter()
            .chain(&artifact.mutable_state.entries)
            .any(|entry| entry.path == path);
    }

    let conn = conary_core::db::open(db_path).unwrap();
    let debt = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)
        .unwrap()
        .pop()
        .expect("model package-set install should retain exact publication debt");
    let captured =
        crate::commands::generation::selected_root::load_publication_selected_root(&conn, &debt)
            .unwrap();
    captured
        .generation
        .entries
        .iter()
        .chain(&captured.state.entries)
        .any(|entry| entry.path == path)
}

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
async fn strict_model_apply_demotes_an_omitted_explicit_package() {
    use conary_core::db::models::{InstallReason, Trove, TroveType};
    use conary_core::repository::versioning::VersionScheme;

    let (_temp_file, db_path) = create_test_db();
    let temp_dir = tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1
install = ["retained-package"]
"#,
    )
    .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    for package in ["retained-package", "orphaned-package"] {
        Trove::new(
            package.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            VersionScheme::Rpm,
        )
        .insert(&conn)
        .unwrap();
    }
    drop(conn);

    cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: temp_dir.path().to_str().unwrap(),
        dry_run: false,
        skip_optional: false,
        strict: true,
        autoremove: false,
        offline: true,
    })
    .await
    .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let retained = Trove::find_one_by_name(&conn, "retained-package")
        .unwrap()
        .unwrap();
    let orphaned = Trove::find_one_by_name(&conn, "orphaned-package")
        .unwrap()
        .unwrap();
    assert_eq!(retained.install_reason, InstallReason::Explicit);
    assert_eq!(orphaned.install_reason, InstallReason::Dependency);
}

#[tokio::test]
async fn test_model_apply_installs_explicit_roots_in_one_lifecycle_transaction() {
    const TEST_NAME: &str = "commands::model::apply::tests::test_model_apply_installs_explicit_roots_in_one_lifecycle_transaction";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    use conary_core::db::models::{
        InstallReason, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
        RepositoryRequirementGroup as DbRepositoryRequirementGroup, Trove,
    };
    use conary_core::repository::dependency_model::{
        RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementGroup,
        RepositoryRequirementKind,
    };

    let (_temp_file, db_path) = create_test_db();
    crate::commands::test_helpers::seed_test_bootable_runtime(std::path::Path::new(&db_path));
    let temp_dir = tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let consumer_name = "a-model-consumer";
    let provider_name = "z-model-provider";
    let version = "1-1";
    let libsystemd = "libmodel-systemd.so.0()(64bit)";
    let requirement = RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        RepositoryRequirementClause {
            name: libsystemd.to_string(),
            capability_kind: Some(RepositoryCapabilityKind::Soname),
            version_constraint: None,
            architecture_qualifier: Default::default(),
            native_text: Some(libsystemd.to_string()),
        },
    );
    let consumer_path = build_test_ccs_package_with_payloads_and_relations(
        temp_dir.path(),
        consumer_name,
        version,
        conary_core::repository::versioning::VersionScheme::Rpm,
        Some(typed_rpm_model_post_helper_bundle(consumer_name, version)),
        vec![(
            format!("/usr/bin/{consumer_name}"),
            b"#!/bin/sh\nexit 0\n".to_vec(),
            0o755,
        )],
        vec![requirement.clone()],
        Vec::new(),
    );
    let provider_path = build_test_ccs_package_with_payloads_and_relations(
        temp_dir.path(),
        provider_name,
        version,
        conary_core::repository::versioning::VersionScheme::Rpm,
        None,
        vec![(
            "/usr/lib/systemd/systemd-update-helper".to_string(),
            b"#!/bin/sh\nprintf observed > /model-post-observed\n".to_vec(),
            0o755,
        )],
        Vec::new(),
        vec![libsystemd.to_string()],
    );
    let (consumer_url, _consumer_server) = serve_test_file(consumer_path.clone());
    let (provider_url, _provider_server) = serve_test_file(provider_path.clone());

    let conn = conary_core::db::open(&db_path).unwrap();
    let repository_id = insert_test_static_ccs_repository(
        &conn,
        "fedora",
        "https://example.test/fedora",
        "fedora-44",
    );
    let mut package_ids = std::collections::HashMap::new();
    for (name, package_path, package_url) in [
        (consumer_name, &consumer_path, consumer_url),
        (provider_name, &provider_path, provider_url),
    ] {
        let checksum = conary_core::hash::sha256(&std::fs::read(package_path).unwrap());
        let mut package = RepositoryPackage::new(
            repository_id,
            name.to_string(),
            version.to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            checksum,
            std::fs::metadata(package_path)
                .unwrap()
                .len()
                .try_into()
                .unwrap(),
            package_url,
        );
        package.architecture = Some("x86_64".to_string());
        package.source_profile = Some("fedora-44".to_string());
        let package_id = package.insert(&conn).unwrap();
        package_ids.insert(name, package_id);
        insert_test_repository_package_resolution(&conn, repository_id, package_id, name, version);
    }
    let consumer_id = package_ids[consumer_name];
    let provider_id = package_ids[provider_name];
    let mut requirement_group = DbRepositoryRequirementGroup::new(
        consumer_id,
        requirement.kind.as_str().to_string(),
        "hard".to_string(),
        serde_json::to_string(&requirement.expression).unwrap(),
    );
    requirement_group.native_text = requirement.native_text.clone();
    let group_id = requirement_group.insert(&conn).unwrap();
    let clause = &requirement.alternatives[0];
    RepositoryRequirement::new(
        consumer_id,
        group_id,
        clause.name.clone(),
        clause.version_constraint.clone(),
        "soname".to_string(),
        "runtime".to_string(),
        clause.native_text.clone(),
    )
    .insert(&conn)
    .unwrap();
    RepositoryProvide::new(
        provider_id,
        libsystemd.to_string(),
        None,
        "soname".to_string(),
        Some(libsystemd.to_string()),
        conary_core::repository::versioning::VersionScheme::Rpm,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let model_path = temp_dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1
install = ["a-model-consumer", "z-model-provider"]
"#,
    )
    .unwrap();

    let result = cmd_model_apply(ApplyOptions {
        model_path: model_path.to_str().unwrap(),
        db_path: &db_path,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        skip_optional: false,
        strict: true,
        autoremove: false,
        offline: true,
    });
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    result.await.unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut changeset_ids = Vec::new();
    for name in [consumer_name, provider_name] {
        let installed = Trove::find_by_name(&conn, name).unwrap();
        assert_eq!(installed.len(), 1, "{name} must be installed exactly once");
        assert_eq!(installed[0].install_reason, InstallReason::Explicit);
        assert_eq!(
            installed[0].selection_reason.as_deref(),
            Some("Installed by model apply")
        );
        changeset_ids.push(installed[0].installed_by_changeset_id);
    }
    assert_eq!(changeset_ids[0], changeset_ids[1]);
    assert!(changeset_ids[0].is_some());
    assert!(published_or_pending_generation_has_path(
        std::path::Path::new(&db_path),
        "/model-post-observed"
    ));
}

#[tokio::test]
async fn test_replatform_executor_replaces_package_when_route_is_executable() {
    const TEST_NAME: &str = "commands::model::apply::tests::test_replatform_executor_replaces_package_when_route_is_executable";
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

    let conn = conary_core::db::open(&db_path).unwrap();
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
    let actions = [DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_source_identity: Some("fedora-44".to_string()),
        target_source_identity: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(arch_package_id),
    }];
    let action_refs = actions.iter().collect::<Vec<_>>();
    drop(conn);

    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let (executed, errors) =
        apply_replatform_changes(&db_path, install_root.to_str().unwrap(), &action_refs)
            .await
            .unwrap();
    assert_eq!(executed, 1, "replatform should execute: {errors:?}");
    assert!(
        errors.is_empty(),
        "unexpected replatform errors: {errors:?}"
    );

    let conn = conary_core::db::open(&db_path).unwrap();
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

    let conn = conary_core::db::open(&db_path).unwrap();
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

    let actions = [DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_source_identity: Some("arch".to_string()),
        target_source_identity: "fedora-44".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("fedora".to_string()),
        target_repository_package_id: Some(fedora_package_id),
    }];
    let action_refs = actions.iter().collect::<Vec<_>>();
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

    let conn = conary_core::db::open(&db_path).unwrap();
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

    let conn = conary_core::db::open(&db_path).unwrap();

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

    let actions = [DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_source_identity: Some("fedora-44".to_string()),
        target_source_identity: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(arch_package_id),
    }];
    drop(conn);

    set_replatform_metadata_failpoint_for_test(true);
    let _reset = ReplatformMetadataFailpointReset;

    let action_refs = actions.iter().collect::<Vec<_>>();
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

    let conn = conary_core::db::open(&db_path).unwrap();
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
