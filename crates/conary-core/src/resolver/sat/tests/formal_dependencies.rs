// conary-core/src/resolver/sat/tests/formal_dependencies.rs

use super::*;
fn insert_native_trove(
    conn: &Connection,
    name: &str,
    version: &str,
    version_scheme: VersionScheme,
    deps: &[(&str, Option<&str>)],
) -> i64 {
    let mut changeset = Changeset::new(format!("Install {name}"));
    let _cs_id = changeset.insert(conn).unwrap();
    changeset
        .update_status(conn, ChangesetStatus::Applied)
        .unwrap();

    let mut trove = Trove::new(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        version_scheme,
    );
    trove.architecture = Some(
        if version_scheme == VersionScheme::Debian {
            "amd64"
        } else {
            "x86_64"
        }
        .to_string(),
    );
    trove.debian_multi_arch = (version_scheme == VersionScheme::Debian)
        .then_some(crate::repository::dependency_model::DebianMultiArch::No);
    let trove_id = trove.insert(conn).unwrap();

    let requirements = deps
        .iter()
        .map(|(dep_name, constraint)| {
            let native = constraint.map_or_else(
                || (*dep_name).to_string(),
                |constraint| format!("{dep_name} {constraint}"),
            );
            crate::repository::requirement::parse_native_requirement(
                crate::repository::dependency_model::RepositoryRequirementKind::Depends,
                version_scheme,
                &native,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    InstalledRequirementGroup::insert_groups(conn, trove_id, version_scheme, &requirements)
        .unwrap();

    trove_id
}

/// Helper: insert a repo package with normalized requirements
pub(super) fn insert_repo_pkg_with_reqs(
    conn: &Connection,
    repo_id: i64,
    name: &str,
    version: &str,
    url: &str,
    version_scheme: &str,
    reqs: &[(&str, Option<&str>)],
) -> i64 {
    let version_scheme =
        crate::repository::distro::require_version_scheme_from_db(Some(version_scheme), name)
            .unwrap();
    let mut pkg = RepositoryPackage::new(
        repo_id,
        name.to_string(),
        version.to_string(),
        version_scheme,
        format!("sha256:{name}"),
        1,
        url.to_string(),
    );
    pkg.architecture = Some(
        if version_scheme == VersionScheme::Debian {
            "amd64"
        } else {
            "x86_64"
        }
        .to_string(),
    );
    pkg.insert(conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    for (cap, constraint) in reqs {
        insert_repo_requirement_group(conn, pkg_id, cap, *constraint, None);
    }

    pkg_id
}

#[test]
fn rpm_transitive_capability_chain() {
    // kernel -> kernel-core-uname-r = X (capability provided by kernel-core)
    // kernel-core -> glibc >= 2.39
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    // kernel-core provides kernel-core-uname-r
    let kc_id = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "kernel-core",
        "6.19.6-200.fc44",
        "https://mirror.fedora.invalid/kernel-core.rpm",
        "rpm",
        &[("glibc", Some(">= 2.39"))],
    );
    let mut provide = RepositoryProvide::new(
        kc_id,
        "kernel-core-uname-r".to_string(),
        Some("6.19.6-200.fc44.x86_64".to_string()),
        "package".to_string(),
        Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    // kernel depends on kernel-core-uname-r
    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "kernel",
        "6.19.6-200.fc44",
        "https://mirror.fedora.invalid/kernel.rpm",
        "rpm",
        &[("kernel-core-uname-r", None)],
    );

    // glibc (leaf)
    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "glibc",
        "2.39-22.fc44",
        "https://mirror.fedora.invalid/glibc.rpm",
        "rpm",
        &[],
    );

    let result = solve_install(&conn, &[("kernel".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(
        result.conflict_message.is_none(),
        "RPM capability chain should resolve: {:?}",
        result.conflict_message
    );
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"kernel"));
    assert!(names.contains(&"kernel-core"));
    assert!(names.contains(&"glibc"));
}

#[test]
fn debian_or_plus_versioned_virtual_dep() {
    // postfix depends on "default-mta | mail-transport-agent"
    // exim4 provides mail-transport-agent
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "ubuntu-main".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    // exim4 provides "mail-transport-agent"
    let exim_id = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "exim4",
        "4.97-1",
        "https://archive.ubuntu.com/ubuntu/pool/exim4.deb",
        "debian",
        &[],
    );
    let mut provide = RepositoryProvide::new(
        exim_id,
        "mail-transport-agent".to_string(),
        None,
        "package".to_string(),
        None,
        VersionScheme::Debian,
    );
    provide.insert(&conn).unwrap();

    // bsd-mailx has OR dep: default-mta | mail-transport-agent (via groups)
    let mut mailx = RepositoryPackage::new(
        repo_id,
        "bsd-mailx".to_string(),
        "8.1.2-0.20220412cvs-1build2".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:mailx".to_string(),
        1,
        "https://archive.ubuntu.com/ubuntu/pool/bsd-mailx.deb".to_string(),
    );
    mailx.architecture = Some("amd64".to_string());
    mailx.insert(&conn).unwrap();
    let mailx_id = mailx.id.unwrap();

    let expression = RepositoryRequirementExpression::Or(vec![
        RepositoryRequirementExpression::Atom(RepositoryRequirementClause::name_only(
            "default-mta".to_string(),
        )),
        RepositoryRequirementExpression::Atom(RepositoryRequirementClause::name_only(
            "mail-transport-agent".to_string(),
        )),
    ]);
    let mut group = RepositoryRequirementGroup::new(
        mailx_id,
        "depends".to_string(),
        "hard".to_string(),
        serde_json::to_string(&expression).unwrap(),
    );
    group.native_text = Some("default-mta | mail-transport-agent".to_string());
    group.insert(&conn).unwrap();
    let group_id = group.id.unwrap();

    let mut clause_a = RepositoryRequirement::new(
        mailx_id,
        group_id,
        "default-mta".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    clause_a.insert(&conn).unwrap();

    let mut clause_b = RepositoryRequirement::new(
        mailx_id,
        group_id,
        "mail-transport-agent".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    clause_b.insert(&conn).unwrap();

    let result =
        solve_install(&conn, &[("bsd-mailx".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(
        result.conflict_message.is_none(),
        "Debian OR dep should resolve via mail-transport-agent provider: {:?}",
        result.conflict_message
    );
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"bsd-mailx"));
    assert!(
        names.contains(&"exim4"),
        "exim4 should be pulled in via OR dep"
    );
}

#[test]
fn rpm_with_requires_one_package_to_provide_both_capabilities() {
    let (_dir, conn) = setup_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let split_a = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "split-a",
        "1-1",
        "https://example.invalid/split-a.rpm",
        "rpm",
        &[],
    );
    insert_virtual_provide(&conn, split_a, "cap-a", VersionScheme::Rpm);
    let split_b = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "split-b",
        "1-1",
        "https://example.invalid/split-b.rpm",
        "rpm",
        &[],
    );
    insert_virtual_provide(&conn, split_b, "cap-b", VersionScheme::Rpm);
    let combined = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "combined",
        "1-1",
        "https://example.invalid/combined.rpm",
        "rpm",
        &[],
    );
    insert_virtual_provide(&conn, combined, "cap-a", VersionScheme::Rpm);
    insert_virtual_provide(&conn, combined, "cap-b", VersionScheme::Rpm);

    let app = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "with-app",
        "1-1",
        "https://example.invalid/with-app.rpm",
        "rpm",
        &[],
    );
    insert_requirement_expression(
        &conn,
        app,
        &RepositoryRequirementExpression::With {
            left: Box::new(requirement_atom("cap-a")),
            right: Box::new(requirement_atom("cap-b")),
        },
    );

    let result = solve_install(&conn, &[("with-app".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    let names = result
        .install_order
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"combined"), "{names:?}");
    assert!(!names.contains(&"split-a"), "{names:?}");
    assert!(!names.contains(&"split-b"), "{names:?}");
}

#[test]
fn rpm_without_excludes_packages_that_provide_the_right_operand() {
    let (_dir, conn) = setup_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let plain = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "plain",
        "1-1",
        "https://example.invalid/plain.rpm",
        "rpm",
        &[],
    );
    insert_virtual_provide(&conn, plain, "cap-a", VersionScheme::Rpm);
    let combined = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "combined",
        "2-1",
        "https://example.invalid/combined.rpm",
        "rpm",
        &[],
    );
    insert_virtual_provide(&conn, combined, "cap-a", VersionScheme::Rpm);
    insert_virtual_provide(&conn, combined, "cap-b", VersionScheme::Rpm);

    let app = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "without-app",
        "1-1",
        "https://example.invalid/without-app.rpm",
        "rpm",
        &[],
    );
    insert_requirement_expression(
        &conn,
        app,
        &RepositoryRequirementExpression::Without {
            left: Box::new(requirement_atom("cap-a")),
            right: Box::new(requirement_atom("cap-b")),
        },
    );

    let result = solve_install(
        &conn,
        &[("without-app".to_string(), VersionConstraint::Any)],
    )
    .unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    let names = result
        .install_order
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"plain"), "{names:?}");
    assert!(!names.contains(&"combined"), "{names:?}");
}

#[test]
fn rpm_if_else_selects_the_branch_from_transaction_state() {
    let (_dir, conn) = setup_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    for name in ["feature", "addon", "fallback"] {
        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            name,
            "1-1",
            &format!("https://example.invalid/{name}.rpm"),
            "rpm",
            &[],
        );
    }
    let app = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "conditional-app",
        "1-1",
        "https://example.invalid/conditional-app.rpm",
        "rpm",
        &[],
    );
    insert_requirement_expression(&conn, app, &requirement_atom("feature"));
    insert_requirement_expression(
        &conn,
        app,
        &RepositoryRequirementExpression::If {
            requirement: Box::new(requirement_atom("addon")),
            condition: Box::new(requirement_atom("feature")),
            otherwise: Some(Box::new(requirement_atom("fallback"))),
        },
    );

    let result = solve_install(
        &conn,
        &[("conditional-app".to_string(), VersionConstraint::Any)],
    )
    .unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    let names = result
        .install_order
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"feature"), "{names:?}");
    assert!(names.contains(&"addon"), "{names:?}");
    assert!(!names.contains(&"fallback"), "{names:?}");
}

#[test]
fn arch_versioned_provider_chain() {
    // ripgrep -> glibc >= 2.39 -> (leaf)
    // Uses Arch version semantics throughout
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "arch-core".to_string(),
        "https://geo.mirror.pkgbuild.com".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "glibc",
        "2.39-1",
        "https://geo.mirror.pkgbuild.com/core/glibc.pkg.tar.zst",
        "arch",
        &[],
    );
    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "ripgrep",
        "14.1.0-1",
        "https://geo.mirror.pkgbuild.com/extra/ripgrep.pkg.tar.zst",
        "arch",
        &[("glibc", Some(">= 2.39"))],
    );

    let result = solve_install(&conn, &[("ripgrep".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"ripgrep"));
    assert!(names.contains(&"glibc"));
}

#[test]
fn installed_debian_satisfies_debian_dep_in_sat() {
    // libc6 is already installed (Debian scheme).
    // Installing myapp which depends on libc6 >= 2.38 should find it installed.
    let (_dir, conn) = setup_test_db();

    insert_native_trove(&conn, "libc6", "2.39-0ubuntu2", VersionScheme::Debian, &[]);

    let mut repo = Repository::new(
        "ubuntu-main".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "myapp",
        "1.0-1",
        "https://archive.ubuntu.com/ubuntu/pool/myapp.deb",
        "debian",
        &[("libc6", Some(">= 2.38"))],
    );

    let result = solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(
        result.conflict_message.is_none(),
        "Installed Debian libc6 should satisfy dep: {:?}",
        result.conflict_message
    );

    // libc6 should be in the result as Installed
    let libc6 = result
        .install_order
        .iter()
        .find(|p| p.name == "libc6")
        .unwrap();
    assert_eq!(libc6.source, SatSource::Installed);
}

#[test]
fn installed_file_provider_wins_over_its_repository_copy() {
    let (_dir, conn) = setup_test_db();
    let installed =
        insert_native_trove(&conn, "filesystem", "3.18-52.fc44", VersionScheme::Rpm, &[]);
    let mut installed_provide = crate::db::models::ProvideEntry::new_typed(
        installed,
        crate::repository::dependency_model::RepositoryCapabilityKind::File,
        "/usr/bin".to_string(),
        None,
        VersionScheme::Rpm,
        crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
    );
    installed_provide.insert(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    repo.priority = 100;
    let repo_id = repo.insert(&conn).unwrap();
    let filesystem = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "filesystem",
        "3.18-52.fc44",
        "https://mirror.fedora.invalid/filesystem.rpm",
        "rpm",
        &[],
    );
    let mut repository_provide = RepositoryProvide::new(
        filesystem,
        "/usr/bin".to_string(),
        None,
        "file".to_string(),
        Some("/usr/bin".to_string()),
        VersionScheme::Rpm,
    );
    repository_provide.insert(&conn).unwrap();
    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "consumer",
        "1",
        "https://mirror.fedora.invalid/consumer.rpm",
        "rpm",
        &[("/usr/bin", None)],
    );

    let result = solve_install(&conn, &[("consumer".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    let selected = result
        .install_order
        .iter()
        .filter(|package| package.name == "filesystem")
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 1, "{result:?}");
    assert_eq!(selected[0].source, SatSource::Installed, "{result:?}");
}

#[test]
fn incompatible_installed_virtual_provider_does_not_block_repository_provider() {
    let (_dir, conn) = setup_test_db();
    let installed = insert_native_trove(&conn, "provider-old", "1", VersionScheme::Rpm, &[]);
    let mut installed_provide = crate::db::models::ProvideEntry::new_typed(
        installed,
        crate::repository::dependency_model::RepositoryCapabilityKind::Virtual,
        "virtual-api".to_string(),
        Some("1".to_string()),
        VersionScheme::Rpm,
        crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
    );
    installed_provide.insert(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();
    let provider = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "provider-new",
        "2",
        "https://mirror.fedora.invalid/provider-new.rpm",
        "rpm",
        &[],
    );
    let mut repository_provide = RepositoryProvide::new(
        provider,
        "virtual-api".to_string(),
        Some("2".to_string()),
        "virtual".to_string(),
        Some("virtual-api = 2".to_string()),
        VersionScheme::Rpm,
    );
    repository_provide.insert(&conn).unwrap();
    let consumer = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "consumer",
        "1",
        "https://mirror.fedora.invalid/consumer.rpm",
        "rpm",
        &[],
    );
    insert_repo_requirement_group(
        &conn,
        consumer,
        "virtual-api",
        Some(">= 2"),
        Some("virtual-api >= 2"),
    );

    let result = solve_install(&conn, &[("consumer".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    assert!(result.install_order.iter().any(|package| {
        package.name == "provider-new" && package.source == SatSource::Repository
    }));
    assert!(
        !result
            .install_order
            .iter()
            .any(|package| package.name == "provider-old"),
        "{result:?}"
    );
}

#[test]
fn provide_version_used_instead_of_package_version() {
    // Package kernel-modules-core version 6.19.6-200.fc44
    // Provides kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64
    // A dep on kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64
    // should match via the provide version, not the package version.
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    // kernel-modules-core with a provide
    let kmc_id = insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "kernel-modules-core",
        "6.19.6-200.fc44",
        "https://mirror.fedora.invalid/kernel-modules-core.rpm",
        "rpm",
        &[],
    );
    let mut provide = RepositoryProvide::new(
        kmc_id,
        "kernel-modules-core-uname-r".to_string(),
        Some("6.19.6-200.fc44.x86_64".to_string()),
        "package".to_string(),
        None,
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    // kernel depends on kernel-modules-core-uname-r (any version)
    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "kernel",
        "6.19.6-200.fc44",
        "https://mirror.fedora.invalid/kernel.rpm",
        "rpm",
        &[("kernel-modules-core-uname-r", None)],
    );

    let result = solve_install(&conn, &[("kernel".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(
        result.conflict_message.is_none(),
        "Should resolve via provide version: {:?}",
        result.conflict_message
    );
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"kernel"));
    assert!(
        names.contains(&"kernel-modules-core"),
        "kernel-modules-core should be pulled in via capability provide"
    );
}

#[test]
fn default_conary_installed_package_is_explicitly_typed_for_sat() {
    let (_dir, conn) = setup_test_db();

    let mut changeset = Changeset::new("Install untyped bash".to_string());
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
    trove.architecture = Some("x86_64".to_string());
    trove.insert(&conn).unwrap();

    let resolution = solve_install(&conn, &[("bash".to_string(), VersionConstraint::Any)]).unwrap();
    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert_eq!(resolution.install_order[0].name, "bash");
}
