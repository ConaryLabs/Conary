// conary-core/src/model/replatform/execution_tests.rs

use super::*;

fn insert_repository_requirement(
    conn: &rusqlite::Connection,
    repository_package_id: i64,
    capability: &str,
    capability_kind: crate::repository::dependency_model::RepositoryCapabilityKind,
    version_constraint: Option<&str>,
    native_text: &str,
) {
    use crate::db::models::RepositoryRequirementGroup as DbRequirementGroup;
    use crate::repository::dependency_model::{
        RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementExpression,
    };

    let clause = RepositoryRequirementClause {
        name: capability.to_string(),
        capability_kind: Some(capability_kind),
        version_constraint: version_constraint.map(str::to_string),
        native_text: Some(native_text.to_string()),
    };
    let expression = RepositoryRequirementExpression::Atom(clause);
    let mut group = DbRequirementGroup::new(
        repository_package_id,
        "depends".to_string(),
        "hard".to_string(),
        serde_json::to_string(&expression).unwrap(),
    );
    group.native_text = Some(native_text.to_string());
    let group_id = group.insert(conn).unwrap();
    let atom_kind = match capability_kind {
        RepositoryCapabilityKind::PackageName => "package",
        RepositoryCapabilityKind::Virtual => "virtual",
        RepositoryCapabilityKind::Soname => "soname",
        RepositoryCapabilityKind::File => "file",
        RepositoryCapabilityKind::Generic => "generic",
    };
    let mut requirement = RepositoryRequirement::new(
        repository_package_id,
        group_id,
        capability.to_string(),
        version_constraint.map(str::to_string),
        atom_kind.to_string(),
        "runtime".to_string(),
        Some(native_text.to_string()),
    );
    requirement.insert(conn).unwrap();
}

#[test]
fn test_planned_replatform_actions_use_installed_versions() {
    let snapshot = SourcePolicyReplatformSnapshot {
        target_distro: "arch".to_string(),
        estimate: Some(ReplatformEstimate {
            target_distro: "arch".to_string(),
            aligned_packages: 1,
            packages_to_realign: 1,
            total_packages: 2,
        }),
        visible_realignment_candidates: 1,
        visible_realignment_proposals: vec![VisibleRealignmentProposal {
            package: "vim".to_string(),
            current_distro: Some("fedora-44".to_string()),
            target_distro: "arch".to_string(),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(22),
        }],
    };
    let mut state = SystemState::new();
    state.add_package(
        "vim".to_string(),
        InstalledPackage {
            name: "vim".to_string(),
            version: "9.0.1".to_string(),
            architecture: Some("x86_64".to_string()),
            explicit: true,
            pinned: false,
            label: Some("fedora@f43:stable".to_string()),
        },
    );

    let actions = planned_replatform_actions(&snapshot, &state);

    assert_eq!(actions.len(), 1);
    assert!(matches!(
        &actions[0],
        crate::model::DiffAction::ReplatformReplace {
            package,
            current_distro,
            target_distro,
            current_version,
            current_architecture,
            target_version,
            architecture,
            target_repository,
            target_repository_package_id,
        } if package == "vim"
            && current_distro.as_deref() == Some("fedora-44")
            && target_distro == "arch"
            && current_version == "9.0.1"
            && current_architecture.as_deref() == Some("x86_64")
            && target_version == "9.1.0"
            && architecture.as_deref() == Some("x86_64")
            && target_repository.as_deref() == Some("arch-core")
            && *target_repository_package_id == Some(22)
    ));
}

#[test]
fn test_replatform_execution_plan_collects_replace_actions() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    arch_repo.insert(&conn).unwrap();

    let actions = vec![
        DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        },
        DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-44".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(22),
        },
        DiffAction::ReplatformReplace {
            package: "bash".to_string(),
            current_distro: Some("fedora-44".to_string()),
            target_distro: "arch".to_string(),
            current_version: "5.1.0".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "5.2.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(11),
        },
    ];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert_eq!(plan.transactions.len(), 2);
    assert_eq!(plan.transactions[0].package, "bash");
    assert_eq!(plan.transactions[1].package, "vim");
    assert_eq!(plan.transactions[0].current_version, "5.1.0");
    assert_eq!(
        plan.transactions[0].current_architecture.as_deref(),
        Some("x86_64")
    );
    assert_eq!(plan.transactions[0].target_version, "5.2.0");
    assert!(!plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].install_repository.as_deref(),
        Some("arch-core")
    );
    assert_eq!(plan.transactions[0].install_repository_package_id, Some(11));
    assert_eq!(
        plan.transactions[0].install_route.as_deref(),
        Some("default:binary")
    );
    assert_eq!(
        plan.transactions[0].blocked_reason,
        Some(ReplatformBlockedReason::MissingVersionedInstallRoute)
    );
}

#[test]
fn test_replatform_execution_plan_reports_block_reason_when_repo_metadata_missing() {
    let (_temp, conn) = create_test_db();
    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: None,
        target_repository_package_id: None,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(!plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].blocked_reason,
        Some(ReplatformBlockedReason::MissingRepositoryMetadata)
    );
}

#[test]
fn test_replatform_execution_plan_reports_missing_versioned_install_route() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    arch_repo.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(22),
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(!plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].blocked_reason,
        Some(ReplatformBlockedReason::MissingInstallRoute)
    );
}

#[test]
fn test_replatform_execution_plan_reports_any_version_route_only() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-latest.ccs".to_string(),
            checksum: "sha256:any-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(22),
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(!plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].install_route.as_deref(),
        Some("resolution:binary")
    );
    assert_eq!(
        plan.transactions[0].blocked_reason,
        Some(ReplatformBlockedReason::AnyVersionRouteOnly)
    );
}

#[test]
fn test_replatform_execution_plan_marks_transaction_executable_only_when_all_legs_are_ready() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
            checksum: "sha256:exact".to_string(),
            delta_base: None,
        }],
    );
    resolution.version = Some("9.1.0".to_string());
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(22),
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    let transaction = &plan.transactions[0];
    assert!(transaction.executable);
    assert!(transaction.remove_leg.ready);
    assert!(transaction.install_leg.ready);
    assert!(transaction.metadata_leg.ready);
    assert!(transaction.blocked_reasons.is_empty());
}

#[test]
fn test_replatform_execution_plan_marks_transaction_blocked_when_route_is_missing() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy_distro = Some("arch".to_string());
    arch_repo.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(22),
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    let transaction = &plan.transactions[0];
    assert!(!transaction.executable);
    assert!(transaction.remove_leg.ready);
    assert!(!transaction.install_leg.ready);
    assert!(transaction.metadata_leg.ready);
    assert!(
        transaction
            .blocked_reasons
            .contains(&ReplatformBlockedReason::MissingInstallRoute)
    );
}

#[test]
fn test_replatform_execution_plan_marks_exact_version_resolution_executable() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("9.1.0".to_string());
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(22),
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].install_route.as_deref(),
        Some("resolution:binary")
    );
    assert_eq!(plan.transactions[0].blocked_reason, None);
}

#[test]
fn test_replatform_execution_plan_blocks_when_target_dependencies_are_missing() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut target_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:vim".to_string(),
        123,
        "https://example.test/arch/vim.pkg.tar.zst".to_string(),
    );
    target_pkg.architecture = Some("x86_64".to_string());
    target_pkg.insert(&conn).unwrap();
    insert_repository_requirement(
        &conn,
        target_pkg.id.unwrap(),
        "libmagic",
        crate::repository::dependency_model::RepositoryCapabilityKind::PackageName,
        Some(">= 1.0"),
        "libmagic >= 1.0",
    );

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("9.1.0".to_string());
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: target_pkg.id,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(!plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].blocked_reason,
        Some(ReplatformBlockedReason::UnsatisfiedTargetDependencies)
    );
    assert_eq!(
        plan.transactions[0].unresolved_dependencies,
        vec!["libmagic (>= 1.0)".to_string()]
    );
}

#[test]
fn test_replatform_execution_plan_accepts_tracked_capability_provider_for_target_dependency() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut target_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "9.1.0".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:vim".to_string(),
        123,
        "https://example.test/arch/vim.pkg.tar.zst".to_string(),
    );
    target_pkg.architecture = Some("x86_64".to_string());
    target_pkg.insert(&conn).unwrap();
    insert_repository_requirement(
        &conn,
        target_pkg.id.unwrap(),
        "libmagic.so.1",
        crate::repository::dependency_model::RepositoryCapabilityKind::Soname,
        None,
        "libmagic.so.1",
    );

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("9.1.0".to_string());
    resolution.insert(&conn).unwrap();

    let mut provider_trove = Trove::new_with_source(
        "file-libs".to_string(),
        "5.45".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        crate::repository::versioning::VersionScheme::Conary,
    );
    provider_trove.architecture = Some("x86_64".to_string());
    let provider_trove_id = provider_trove.insert(&conn).unwrap();

    let mut provide = ProvideEntry::new_typed(
        provider_trove_id,
        "soname",
        "libmagic.so.1".to_string(),
        None,
    );
    provide.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: target_pkg.id,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(plan.transactions[0].executable);
    assert_eq!(plan.transactions[0].blocked_reason, None);
    assert!(plan.transactions[0].unresolved_dependencies.is_empty());
}

#[test]
fn test_replatform_execution_plan_accepts_repo_metadata_provider_for_target_dependency() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut target_pkg = RepositoryPackage::new(
        arch_repo_id,
        "kernel".to_string(),
        "6.19.6-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:kernel".to_string(),
        123,
        "https://example.test/arch/kernel.pkg.tar.zst".to_string(),
    );
    target_pkg.architecture = Some("x86_64".to_string());
    target_pkg.insert(&conn).unwrap();
    insert_repository_requirement(
        &conn,
        target_pkg.id.unwrap(),
        "kernel-core-uname-r",
        crate::repository::dependency_model::RepositoryCapabilityKind::Virtual,
        Some("= 6.19.6-200.fc44.x86_64"),
        "kernel-core-uname-r = 6.19.6-200.fc44.x86_64",
    );

    let mut provider_pkg = RepositoryPackage::new(
        arch_repo_id,
        "kernel-core".to_string(),
        "6.19.6-200.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:kernel-core".to_string(),
        123,
        "https://example.test/arch/kernel-core.pkg.tar.zst".to_string(),
    );
    provider_pkg.architecture = Some("x86_64".to_string());
    provider_pkg.insert(&conn).unwrap();
    let mut provide = RepositoryProvide::new(
        provider_pkg.id.unwrap(),
        "kernel-core-uname-r".to_string(),
        Some("6.19.6-200.fc44.x86_64".to_string()),
        "virtual".to_string(),
        Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
        crate::repository::versioning::VersionScheme::Arch,
    );
    provide.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "kernel".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/kernel-6.19.6-1.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("6.19.6-1".to_string());
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "kernel".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "6.19.5-1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "6.19.6-1".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: target_pkg.id,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(plan.transactions[0].executable);
    assert_eq!(plan.transactions[0].blocked_reason, None);
    assert!(plan.transactions[0].unresolved_dependencies.is_empty());
}

#[test]
fn test_replatform_execution_plan_accepts_debian_repo_metadata_provider_for_target_dependency() {
    let (_temp, conn) = create_test_db();
    let mut deb_repo = Repository::new(
        "ubuntu-main".to_string(),
        "https://example.test/ubuntu".to_string(),
    );
    deb_repo.default_strategy = Some("binary".to_string());
    deb_repo.default_strategy_distro = Some("ubuntu-26.04".to_string());
    let deb_repo_id = deb_repo.insert(&conn).unwrap();

    let mut target_pkg = RepositoryPackage::new(
        deb_repo_id,
        "mailer".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:mailer".to_string(),
        123,
        "https://example.test/ubuntu/mailer.deb".to_string(),
    );
    target_pkg.architecture = Some("amd64".to_string());
    target_pkg.insert(&conn).unwrap();
    insert_repository_requirement(
        &conn,
        target_pkg.id.unwrap(),
        "mail-transport-agent",
        crate::repository::dependency_model::RepositoryCapabilityKind::Virtual,
        None,
        "mail-transport-agent",
    );

    let mut provider_pkg = RepositoryPackage::new(
        deb_repo_id,
        "postfix".to_string(),
        "3.8.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:postfix".to_string(),
        123,
        "https://example.test/ubuntu/postfix.deb".to_string(),
    );
    provider_pkg.architecture = Some("amd64".to_string());
    provider_pkg.insert(&conn).unwrap();
    let mut provide = RepositoryProvide::new(
        provider_pkg.id.unwrap(),
        "mail-transport-agent".to_string(),
        None,
        "virtual".to_string(),
        Some("mail-transport-agent".to_string()),
        crate::repository::versioning::VersionScheme::Debian,
    );
    provide.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        deb_repo_id,
        "mailer".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/ubuntu/mailer-1.0-1.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("1.0-1".to_string());
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "mailer".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "ubuntu-26.04".to_string(),
        current_version: "0.9-1".to_string(),
        current_architecture: Some("amd64".to_string()),
        target_version: "1.0-1".to_string(),
        architecture: Some("amd64".to_string()),
        target_repository: Some("ubuntu-main".to_string()),
        target_repository_package_id: target_pkg.id,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(plan.transactions[0].executable);
    assert_eq!(plan.transactions[0].blocked_reason, None);
    assert!(plan.transactions[0].unresolved_dependencies.is_empty());
}

#[test]
fn test_replatform_execution_plan_accepts_debian_normalized_provider_with_version_constraint() {
    let (_temp, conn) = create_test_db();
    let mut deb_repo = Repository::new(
        "ubuntu-main".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    deb_repo.default_strategy = Some("binary".to_string());
    deb_repo.default_strategy_distro = Some("ubuntu-26.04".to_string());
    let deb_repo_id = deb_repo.insert(&conn).unwrap();

    let mut target_pkg = RepositoryPackage::new(
        deb_repo_id,
        "mailer".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:mailer".to_string(),
        123,
        "https://archive.ubuntu.com/ubuntu/pool/mailer_1.0-1_amd64.deb".to_string(),
    );
    target_pkg.architecture = Some("amd64".to_string());
    target_pkg.insert(&conn).unwrap();
    insert_repository_requirement(
        &conn,
        target_pkg.id.unwrap(),
        "mail-transport-agent",
        crate::repository::dependency_model::RepositoryCapabilityKind::Virtual,
        Some(">= 1.0~beta1"),
        "mail-transport-agent (>= 1.0~beta1)",
    );

    let mut provider_pkg = RepositoryPackage::new(
        deb_repo_id,
        "postfix".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:postfix".to_string(),
        123,
        "https://archive.ubuntu.com/ubuntu/pool/postfix_1.0-1_amd64.deb".to_string(),
    );
    provider_pkg.architecture = Some("amd64".to_string());
    provider_pkg.insert(&conn).unwrap();

    let mut provide = RepositoryProvide::new(
        provider_pkg.id.unwrap(),
        "mail-transport-agent".to_string(),
        Some("1.0".to_string()),
        "virtual".to_string(),
        Some("mail-transport-agent (= 1.0)".to_string()),
        VersionScheme::Debian,
    );
    provide.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        deb_repo_id,
        "mailer".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://archive.ubuntu.com/ubuntu/pool/mailer_1.0-1.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("1.0-1".to_string());
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "mailer".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "ubuntu-26.04".to_string(),
        current_version: "0.9-1".to_string(),
        current_architecture: Some("amd64".to_string()),
        target_version: "1.0-1".to_string(),
        architecture: Some("amd64".to_string()),
        target_repository: Some("ubuntu-main".to_string()),
        target_repository_package_id: target_pkg.id,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(plan.transactions[0].executable);
    assert_eq!(plan.transactions[0].blocked_reason, None);
    assert!(plan.transactions[0].unresolved_dependencies.is_empty());
}

#[test]
fn test_replatform_execution_plan_accepts_arch_normalized_provider_for_target_dependency() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut target_pkg = RepositoryPackage::new(
        arch_repo_id,
        "mailer".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:mailer".to_string(),
        123,
        "https://example.test/arch/mailer.pkg.tar.zst".to_string(),
    );
    target_pkg.architecture = Some("x86_64".to_string());
    target_pkg.insert(&conn).unwrap();
    insert_repository_requirement(
        &conn,
        target_pkg.id.unwrap(),
        "mail-transport-agent",
        crate::repository::dependency_model::RepositoryCapabilityKind::Virtual,
        None,
        "mail-transport-agent",
    );

    let mut provider_pkg = RepositoryPackage::new(
        arch_repo_id,
        "postfix".to_string(),
        "3.8.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:postfix".to_string(),
        123,
        "https://example.test/arch/postfix.pkg.tar.zst".to_string(),
    );
    provider_pkg.architecture = Some("x86_64".to_string());
    provider_pkg.insert(&conn).unwrap();
    let mut provide = RepositoryProvide::new(
        provider_pkg.id.unwrap(),
        "mail-transport-agent".to_string(),
        None,
        "virtual".to_string(),
        Some("mail-transport-agent".to_string()),
        VersionScheme::Arch,
    );
    provide.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        arch_repo_id,
        "mailer".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/mailer-1.0-1.ccs".to_string(),
            checksum: "sha256:exact-version".to_string(),
            delta_base: None,
        }],
    );
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.version = Some("1.0-1".to_string());
    resolution.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "mailer".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "0.9-1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "1.0-1".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: target_pkg.id,
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(plan.transactions[0].executable);
    assert_eq!(plan.transactions[0].blocked_reason, None);
    assert!(plan.transactions[0].unresolved_dependencies.is_empty());
}

#[test]
fn test_replatform_execution_plan_reports_architecture_mismatch() {
    let (_temp, conn) = create_test_db();
    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    arch_repo.insert(&conn).unwrap();

    let actions = vec![DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("aarch64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(22),
    }];

    let plan = replatform_execution_plan(&conn, &actions)
        .expect("plan query should succeed")
        .expect("expected plan");

    assert!(!plan.transactions[0].executable);
    assert_eq!(
        plan.transactions[0].blocked_reason,
        Some(ReplatformBlockedReason::ArchitectureMismatch)
    );
}

#[test]
fn test_convergence_plans_ownership_state_transition_adopted_track_to_taken() {
    use crate::model::parser::ConvergenceIntent;

    // Given: packages at various adoption states and FullOwnership convergence intent
    let convergence = ConvergenceIntent::FullOwnership;
    let target_source = convergence.target_install_source();
    // Verify the convergence target maps to "taken"
    assert_eq!(target_source, "taken");

    // Given: a package currently at AdoptedTrack
    let adopted_track = InstallSource::AdoptedTrack;
    let adopted_full = InstallSource::AdoptedFull;
    let taken = InstallSource::Taken;

    // AdoptedTrack is not at the convergence target
    assert_ne!(adopted_track.as_str(), target_source);
    // AdoptedFull is not at the convergence target either
    assert_ne!(adopted_full.as_str(), target_source);
    // Taken IS the convergence target
    assert_eq!(taken.as_str(), target_source);

    // Verify the state ordering: AdoptedTrack < AdoptedFull < Taken
    // Each convergence level maps to a progressively deeper ownership state
    assert_eq!(
        ConvergenceIntent::TrackOnly.target_install_source(),
        adopted_track.as_str()
    );
    assert_eq!(
        ConvergenceIntent::CasBacked.target_install_source(),
        adopted_full.as_str()
    );
    assert_eq!(
        ConvergenceIntent::FullOwnership.target_install_source(),
        taken.as_str()
    );

    // AdoptedTrack is adopted (not yet converged)
    assert!(adopted_track.is_adopted());
    // AdoptedFull is adopted (not yet converged for FullOwnership)
    assert!(adopted_full.is_adopted());
    // Taken is Conary-owned (fully converged)
    assert!(!taken.is_adopted());
    assert!(taken.is_conary_owned());
}
