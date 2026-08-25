// conary-core/src/resolver/sat/tests.rs

use super::*;
use crate::db;
use crate::db::models::{
    Changeset, ChangesetStatus, InstalledRequirementGroup, Repository, RepositoryPackage,
    RepositoryRequirement, Trove, TroveType,
};

fn setup_test_db() -> (tempfile::TempDir, Connection) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    (temp_dir, conn)
}

/// SAT algebra tests deliberately remove source-selection constraints. Strict
/// transaction-profile behavior is covered by the policy and app transaction
/// tests; these fixtures exercise version, relation, and Boolean semantics.
fn solve_install(
    conn: &Connection,
    requests: &[(String, VersionConstraint)],
) -> Result<SatResolution> {
    solve_install_with_policy(
        conn,
        requests,
        &ResolutionPolicy::new()
            .with_mixing(crate::repository::resolution_policy::DependencyMixingPolicy::Permissive),
    )
}

fn insert_repo_requirement_group(
    conn: &Connection,
    repository_package_id: i64,
    capability: &str,
    constraint: Option<&str>,
    native_text: Option<&str>,
) -> i64 {
    let clause = match constraint {
        Some(constraint) => {
            RepositoryRequirementClause::versioned(capability.to_string(), constraint.to_string())
        }
        None => RepositoryRequirementClause::name_only(capability.to_string()),
    };
    let expression = RepositoryRequirementExpression::Atom(clause);
    let mut group = RepositoryRequirementGroup::new(
        repository_package_id,
        "depends".to_string(),
        "hard".to_string(),
        serde_json::to_string(&expression).unwrap(),
    );
    group.native_text = native_text.map(str::to_string);
    let group_id = group.insert(conn).unwrap();

    RepositoryRequirement::new(
        repository_package_id,
        group.id.unwrap(),
        capability.to_string(),
        constraint.map(str::to_string),
        "package".to_string(),
        "runtime".to_string(),
        native_text.map(str::to_string),
    )
    .insert(conn)
    .unwrap();
    group_id
}

fn insert_rpm_repo_package(
    conn: &Connection,
    repository_id: i64,
    name: &str,
    version: &str,
) -> i64 {
    let mut package = RepositoryPackage::new(
        repository_id,
        name.to_string(),
        version.to_string(),
        VersionScheme::Rpm,
        format!("sha256:{name}-{version}"),
        1,
        format!("https://example.invalid/{name}-{version}.rpm"),
    );
    package.architecture = Some("x86_64".to_string());
    package.source_profile = Some("fedora-44".to_string());
    package.insert(conn).unwrap()
}

fn insert_typed_repo_requirement_group(
    conn: &Connection,
    repository_package_id: i64,
    requirement: &crate::repository::dependency_model::RepositoryRequirementGroup,
) {
    let mut group = RepositoryRequirementGroup::new(
        repository_package_id,
        requirement.kind.as_str().to_string(),
        "hard".to_string(),
        serde_json::to_string(&requirement.expression).unwrap(),
    );
    group.native_text = requirement.native_text.clone();
    let group_id = group.insert(conn).unwrap();
    for clause in &requirement.alternatives {
        RepositoryRequirement::new(
            repository_package_id,
            group_id,
            clause.name.clone(),
            clause.version_constraint.clone(),
            "package".to_string(),
            "runtime".to_string(),
            clause.native_text.clone(),
        )
        .insert(conn)
        .unwrap();
    }
}

fn insert_debian_repo_package(
    conn: &Connection,
    repository_id: i64,
    name: &str,
    version: &str,
    architecture: &str,
    multi_arch: crate::repository::dependency_model::DebianMultiArch,
) -> i64 {
    let mut package = RepositoryPackage::new(
        repository_id,
        name.to_string(),
        version.to_string(),
        VersionScheme::Debian,
        format!("sha256:{name}-{version}-{architecture}"),
        1,
        format!("https://debian.invalid/{name}_{version}_{architecture}.deb"),
    );
    package.architecture = Some(architecture.to_string());
    package.debian_multi_arch = Some(multi_arch);
    package.insert(conn).unwrap()
}

#[test]
fn test_transitive_loading_limit_rejects_excessive_name_count() {
    let err =
        check_transitive_loading_limits(std::time::Duration::from_secs(0), MAX_LOADED_NAMES + 1)
            .unwrap_err();
    assert!(err.to_string().contains("too many dependency names"));
}

#[test]
fn test_transitive_loading_limit_rejects_timeout() {
    let err = check_transitive_loading_limits(
        TRANSITIVE_LOAD_TIMEOUT + std::time::Duration::from_secs(1),
        1,
    )
    .unwrap_err();
    assert!(err.to_string().contains("timed out"));
}

/// Helper: insert an explicitly RPM-versioned trove and its dependencies.
fn insert_rpm_trove(
    conn: &Connection,
    name: &str,
    version: &str,
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
        VersionScheme::Rpm,
    );
    trove.architecture = Some("x86_64".to_string());
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
                VersionScheme::Rpm,
                &native,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    InstalledRequirementGroup::insert_groups(conn, trove_id, VersionScheme::Rpm, &requirements)
        .unwrap();

    trove_id
}

#[test]
fn test_simple_install() {
    // A depends on B, both available as installed
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "B", "1.0.0", &[]);
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", None)]);

    let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(result.conflict_message.is_none());
    assert!(!result.install_order.is_empty());

    // Both A and B should be in the result
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"A"));
    assert!(names.contains(&"B"));
}

#[test]
fn test_missing_dependency() {
    // A depends on B, but B is not installed or in any repo
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", Some(">= 2.0.0"))]);

    let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

    // Should have a conflict message since B can't be found
    assert!(result.conflict_message.is_some());
}

#[test]
fn exact_repository_root_preserves_variant_identity() {
    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "fedora-44".to_string(),
        "https://example.invalid/fedora".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();
    let old_root = insert_rpm_repo_package(&conn, repository_id, "root", "1-1");
    let new_root = insert_rpm_repo_package(&conn, repository_id, "root", "2-1");
    let dependency = insert_rpm_repo_package(&conn, repository_id, "dependency", "1-1");
    insert_repo_requirement_group(&conn, old_root, "dependency", None, Some("dependency"));

    let result = solve_exact_repository_package_with_policy(
        &conn,
        old_root,
        "x86_64",
        &ResolutionPolicy::new().with_primary_source_identity("fedora-44"),
    )
    .unwrap();
    let SatExactResolution::Resolved { install_order } = result else {
        panic!("exact root unexpectedly unresolved");
    };
    let package_ids = install_order
        .iter()
        .map(|package| package.repo_package_id.unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(package_ids, BTreeSet::from([old_root, dependency]));
    assert!(!package_ids.contains(&new_root));
}

#[test]
fn exact_repository_root_returns_typed_missing_group() {
    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "fedora-44".to_string(),
        "https://example.invalid/fedora".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();
    let root = insert_rpm_repo_package(&conn, repository_id, "root", "1-1");
    let group = insert_repo_requirement_group(
        &conn,
        root,
        "missing",
        Some(">= 2-1"),
        Some("missing >= 2-1"),
    );

    let result = solve_exact_repository_package_with_policy(
        &conn,
        root,
        "x86_64",
        &ResolutionPolicy::new().with_primary_source_identity("fedora-44"),
    )
    .unwrap();
    assert_eq!(
        result,
        SatExactResolution::Unresolved {
            dependencies: vec![SatUnresolvedDependency {
                repository_package_id: root,
                repository_requirement_group_id: group,
            }],
        }
    );
}

#[test]
fn exact_repository_root_preserves_all_groups_sharing_one_constraint() {
    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "fedora-44".to_string(),
        "https://example.invalid/fedora".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();
    let root = insert_rpm_repo_package(&conn, repository_id, "root", "1-1");
    let first = insert_repo_requirement_group(
        &conn,
        root,
        "missing",
        None,
        Some("first missing declaration"),
    );
    let second = insert_repo_requirement_group(
        &conn,
        root,
        "missing",
        None,
        Some("second missing declaration"),
    );

    let result = solve_exact_repository_package_with_policy(
        &conn,
        root,
        "x86_64",
        &ResolutionPolicy::new().with_primary_source_identity("fedora-44"),
    )
    .unwrap();
    assert_eq!(
        result,
        SatExactResolution::Unresolved {
            dependencies: vec![
                SatUnresolvedDependency {
                    repository_package_id: root,
                    repository_requirement_group_id: first,
                },
                SatUnresolvedDependency {
                    repository_package_id: root,
                    repository_requirement_group_id: second,
                },
            ],
        }
    );
}

#[test]
fn adopted_malformed_self_provides_return_conflict_without_diagnostic_panic() {
    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "fedora-44".to_string(),
        "https://example.invalid/fedora".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();

    let kernel_core_id = formal_dependencies::insert_repo_pkg_with_reqs(
        &conn,
        repository_id,
        "kernel-core",
        "6.19.10-300.fc44",
        "https://example.invalid/kernel-core.rpm",
        "rpm",
        &[("dracut", Some(">= 027"))],
    );
    let kernel_capability = "kernel-core-uname-r";
    let kernel_capability_version = "6.19.10-300.fc44.x86_64";
    let mut kernel_provide = RepositoryProvide::new(
        kernel_core_id,
        kernel_capability.to_string(),
        Some(kernel_capability_version.to_string()),
        "generic".to_string(),
        Some(format!("{kernel_capability} = {kernel_capability_version}")),
        VersionScheme::Rpm,
    );
    kernel_provide.insert(&conn).unwrap();

    for (name, version, requirements, config_capability) in [
        (
            "dracut",
            "108-6.fc44",
            vec![("util-linux", Some(">= 2.21"))],
            "config(dracut)",
        ),
        (
            "util-linux",
            "2.41.3-12.fc44",
            vec![("condition-runtime", None)],
            "config(util-linux)",
        ),
    ] {
        let package_id = formal_dependencies::insert_repo_pkg_with_reqs(
            &conn,
            repository_id,
            name,
            version,
            &format!("https://example.invalid/{name}.rpm"),
            "rpm",
            &requirements,
        );
        insert_repo_requirement_group(
            &conn,
            package_id,
            config_capability,
            Some(&format!("= {version}")),
            Some(&format!("{config_capability} = {version}")),
        );
        let mut provide = RepositoryProvide::new(
            package_id,
            config_capability.to_string(),
            Some(version.to_string()),
            "generic".to_string(),
            Some(format!("{config_capability} = {version}")),
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();
        if name == "util-linux" {
            let conditional = crate::repository::requirement::parse_native_requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Rpm,
                "(missing-runtime if condition-runtime)",
            )
            .unwrap();
            insert_typed_repo_requirement_group(&conn, package_id, &conditional);
            let mut i686 = RepositoryPackage::new(
                repository_id,
                name.to_string(),
                version.to_string(),
                VersionScheme::Rpm,
                "sha256:util-linux-i686".to_string(),
                1,
                "https://example.invalid/util-linux.i686.rpm".to_string(),
            );
            i686.architecture = Some("i686".to_string());
            let i686_id = i686.insert(&conn).unwrap();
            insert_repo_requirement_group(&conn, i686_id, "/bin/sh", None, Some("/bin/sh"));
            insert_repo_requirement_group(
                &conn,
                i686_id,
                config_capability,
                Some(&format!("= {version}")),
                Some(&format!("{config_capability} = {version}")),
            );
            let mut i686_provide = RepositoryProvide::new(
                i686_id,
                config_capability.to_string(),
                Some(version.to_string()),
                "generic".to_string(),
                Some(format!("{config_capability} = {version}")),
                VersionScheme::Rpm,
            );
            i686_provide.insert(&conn).unwrap();
        }

        let exact_config_version = format!("= {version}");
        let installed_id = if name == "dracut" {
            insert_rpm_trove(
                &conn,
                name,
                version,
                &[
                    ("util-linux", Some(">= 2.21")),
                    (config_capability, Some(exact_config_version.as_str())),
                ],
            )
        } else {
            insert_rpm_trove(
                &conn,
                name,
                version,
                &[(config_capability, Some(exact_config_version.as_str()))],
            )
        };
        let mut malformed = crate::db::models::ProvideEntry::new(
            installed_id,
            format!("{config_capability} = {version}"),
            None,
            VersionScheme::Rpm,
        );
        malformed.insert(&conn).unwrap();
    }
    insert_rpm_trove(&conn, "condition-runtime", "1", &[]);

    let requirement = crate::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Rpm,
        &format!("{kernel_capability} = {kernel_capability_version}"),
    )
    .unwrap();
    let result = solve_requirement_groups_with_policy(
        &conn,
        &[requirement],
        VersionScheme::Rpm,
        &ResolutionPolicy::new()
            .with_mixing(crate::repository::resolution_policy::DependencyMixingPolicy::Permissive),
    )
    .expect("an unsatisfiable request must return a typed conflict");

    assert!(result.conflict_message.is_some(), "{result:?}");
}

#[test]
fn test_version_conflict() {
    // A needs B >= 2.0, only B 1.0 is installed
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "B", "1.0.0", &[]);
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", Some(">= 2.0.0"))]);

    let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

    // B 1.0 doesn't satisfy >= 2.0, so this should report a conflict
    // (unless a repo has B >= 2.0, which it doesn't here)
    assert!(result.conflict_message.is_some());
}

#[test]
fn test_diamond_dependency() {
    // A -> B, C; B -> D; C -> D
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "D", "1.0.0", &[]);
    insert_rpm_trove(&conn, "B", "1.0.0", &[("D", None)]);
    insert_rpm_trove(&conn, "C", "1.0.0", &[("D", None)]);
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", None), ("C", None)]);

    let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(result.conflict_message.is_none());

    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"A"));
    assert!(names.contains(&"B"));
    assert!(names.contains(&"C"));
    assert!(names.contains(&"D"));
}

#[test]
fn test_removal_check() {
    // A depends on B; removing B should list A as breaking
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "B", "1.0.0", &[]);
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", None)]);

    let breaking = solve_removal(&conn, &["B".to_string()]).unwrap();
    assert!(breaking.contains(&"A".to_string()));
}

#[test]
fn exact_trove_removal_preserves_another_installed_instance_provider() {
    let (_dir, conn) = setup_test_db();
    let old = insert_rpm_trove(&conn, "B", "1.0.0", &[]);
    let current = insert_rpm_trove(&conn, "B", "2.0.0", &[]);
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", None)]);

    let one = solve_removal_troves(&conn, &[old]).unwrap();
    let both = solve_removal_troves(&conn, &[old, current]).unwrap();

    assert!(one.is_empty(), "the current B instance still satisfies A");
    assert!(both.contains(&"A".to_string()));
}

#[test]
fn test_removal_transitive() {
    // C depends on B, B depends on A; removing A should break both B and C
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "A", "1.0.0", &[]);
    insert_rpm_trove(&conn, "B", "1.0.0", &[("A", None)]);
    insert_rpm_trove(&conn, "C", "1.0.0", &[("B", None)]);

    let breaking = solve_removal(&conn, &["A".to_string()]).unwrap();
    assert!(breaking.contains(&"B".to_string()));
    assert!(breaking.contains(&"C".to_string()));
}

#[test]
fn test_empty_install() {
    let (_dir, conn) = setup_test_db();
    let result = solve_install(&conn, &[]).unwrap();
    assert!(result.install_order.is_empty());
    assert!(result.conflict_message.is_none());
}

#[test]
fn test_deep_transitive_chain() {
    // A -> B -> C -> D -> E (5-level chain, all installed)
    // Verifies fixed-point loading resolves arbitrarily deep chains
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "E", "1.0.0", &[]);
    insert_rpm_trove(&conn, "D", "1.0.0", &[("E", None)]);
    insert_rpm_trove(&conn, "C", "1.0.0", &[("D", None)]);
    insert_rpm_trove(&conn, "B", "1.0.0", &[("C", None)]);
    insert_rpm_trove(&conn, "A", "1.0.0", &[("B", None)]);

    let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(result.conflict_message.is_none());

    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"A"));
    assert!(names.contains(&"B"));
    assert!(names.contains(&"C"));
    assert!(names.contains(&"D"));
    assert!(names.contains(&"E"));
}

#[test]
fn test_sat_install_uses_repo_native_debian_constraints_via_provider() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "ubuntu-main".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    let mut app = RepositoryPackage::new(
        repo_id,
        "myapp".to_string(),
        "2.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:app".to_string(),
        1,
        "https://archive.ubuntu.com/ubuntu/pool/main/m/myapp.deb".to_string(),
    );
    app.architecture = Some("amd64".to_string());
    app.insert(&conn).unwrap();
    let app_id = app.id.unwrap();

    insert_repo_requirement_group(
        &conn,
        app_id,
        "libfoo",
        Some(">= 1.0~beta1"),
        Some("libfoo (>= 1.0~beta1)"),
    );

    let mut libfoo = RepositoryPackage::new(
        repo_id,
        "libfoo".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:libfoo".to_string(),
        1,
        "https://archive.ubuntu.com/ubuntu/pool/main/libf/libfoo.deb".to_string(),
    );
    libfoo.architecture = Some("amd64".to_string());
    libfoo.insert(&conn).unwrap();

    let result = solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(result.conflict_message.is_none(), "{result:?}");
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|pkg| pkg.name.as_str())
        .collect();
    assert!(names.contains(&"myapp"));
    assert!(names.contains(&"libfoo"));
}

#[test]
fn test_sat_install_uses_repo_native_arch_constraints_via_provider() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "arch-core".to_string(),
        "https://geo.mirror.pkgbuild.com".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    let mut app = RepositoryPackage::new(
        repo_id,
        "ripgrep".to_string(),
        "14.1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:ripgrep".to_string(),
        1,
        "https://geo.mirror.pkgbuild.com/core/os/x86_64/ripgrep.pkg.tar.zst".to_string(),
    );
    app.architecture = Some("x86_64".to_string());
    app.insert(&conn).unwrap();
    let app_id = app.id.unwrap();

    insert_repo_requirement_group(
        &conn,
        app_id,
        "glibc",
        Some(">= 2.39"),
        Some("glibc >= 2.39"),
    );

    let mut glibc = RepositoryPackage::new(
        repo_id,
        "glibc".to_string(),
        "2.39-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:glibc".to_string(),
        1,
        "https://geo.mirror.pkgbuild.com/core/os/x86_64/glibc.pkg.tar.zst".to_string(),
    );
    glibc.architecture = Some("x86_64".to_string());
    glibc.insert(&conn).unwrap();

    let result = solve_install(&conn, &[("ripgrep".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(result.conflict_message.is_none(), "{result:?}");
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|pkg| pkg.name.as_str())
        .collect();
    assert!(names.contains(&"ripgrep"));
    assert!(names.contains(&"glibc"));
}

#[test]
fn rpm_path_requirement_resolves_through_generator_selected_file_provider() {
    use crate::repository::dependency_model::RepositoryRequirementKind;

    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "fedora-44".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();

    let mut consumer = RepositoryPackage::new(
        repository_id,
        "glibc-common".to_string(),
        "2.43.9000-6.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:glibc-common".to_string(),
        1,
        "https://example.com/fedora/glibc-common.rpm".to_string(),
    );
    consumer.architecture = Some("x86_64".to_string());
    let consumer_id = consumer.insert(&conn).unwrap();
    let requirement = crate::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Rpm,
        "/usr/bin/bash",
    )
    .unwrap();
    insert_typed_repo_requirement_group(&conn, consumer_id, &requirement);

    let mut provider = RepositoryPackage::new(
        repository_id,
        "bash".to_string(),
        "5.3.3-2.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:bash".to_string(),
        1,
        "https://example.com/fedora/bash.rpm".to_string(),
    );
    provider.architecture = Some("x86_64".to_string());
    let provider_id = provider.insert(&conn).unwrap();
    let mut file_provide = RepositoryProvide::new(
        provider_id,
        "/usr/bin/bash".to_string(),
        None,
        "file".to_string(),
        None,
        VersionScheme::Rpm,
    );
    file_provide.insert(&conn).unwrap();

    let result = solve_install(
        &conn,
        &[("glibc-common".to_string(), VersionConstraint::Any)],
    )
    .unwrap();

    assert!(result.conflict_message.is_none(), "{result:?}");
    let names = result
        .install_order
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"glibc-common"));
    assert!(names.contains(&"bash"));
}

#[test]
fn debian_multi_arch_qualifiers_reach_sat_without_name_suffix_matching() {
    use crate::repository::dependency_model::{DebianMultiArch, RepositoryRequirementKind};

    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "debian-multiarch".to_string(),
        "https://debian.invalid".to_string(),
    );
    let repository_id = repository.insert(&conn).unwrap();

    let any_consumer = insert_debian_repo_package(
        &conn,
        repository_id,
        "any-consumer",
        "1",
        "amd64",
        DebianMultiArch::No,
    );
    let any_requirement = crate::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Debian,
        "liballowed:any",
    )
    .unwrap();
    insert_typed_repo_requirement_group(&conn, any_consumer, &any_requirement);
    insert_debian_repo_package(
        &conn,
        repository_id,
        "liballowed",
        "1",
        "arm64",
        DebianMultiArch::Allowed,
    );

    let any_result = solve_install(
        &conn,
        &[("any-consumer".to_string(), VersionConstraint::Any)],
    )
    .unwrap();
    assert!(any_result.conflict_message.is_none(), "{any_result:?}");
    assert!(
        any_result
            .install_order
            .iter()
            .any(|package| package.name == "liballowed")
    );

    let native_consumer = insert_debian_repo_package(
        &conn,
        repository_id,
        "native-consumer",
        "1",
        "all",
        DebianMultiArch::No,
    );
    let native_requirement = crate::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Debian,
        "libnative:native",
    )
    .unwrap();
    insert_typed_repo_requirement_group(&conn, native_consumer, &native_requirement);
    insert_debian_repo_package(
        &conn,
        repository_id,
        "libnative",
        "1",
        "amd64",
        DebianMultiArch::No,
    );
    insert_debian_repo_package(
        &conn,
        repository_id,
        "libnative",
        "2",
        "arm64",
        DebianMultiArch::No,
    );

    let native_result = solve_install(
        &conn,
        &[("native-consumer".to_string(), VersionConstraint::Any)],
    )
    .unwrap();
    assert!(
        native_result.conflict_message.is_none(),
        "{native_result:?}"
    );
    assert!(
        native_result
            .install_order
            .iter()
            .any(|package| { package.name == "libnative" && package.version == "1" })
    );
    assert!(
        !native_result
            .install_order
            .iter()
            .any(|package| { package.name == "libnative" && package.version == "2" })
    );
}

#[test]
fn debian_explicit_any_provide_reaches_sat_as_architecture_authority() {
    use crate::repository::dependency_model::{
        DebianMultiArch, ProvideArchitectureQualifier, RepositoryRequirementKind,
    };

    let (_dir, conn) = setup_test_db();
    let mut repository = Repository::new(
        "debian-provides".to_string(),
        "https://debian.invalid".to_string(),
    );
    let repository_id = repository.insert(&conn).unwrap();

    let consumer = insert_debian_repo_package(
        &conn,
        repository_id,
        "mail-client",
        "1",
        "amd64",
        DebianMultiArch::No,
    );
    let requirement = crate::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Debian,
        "mail-api:any",
    )
    .unwrap();
    insert_typed_repo_requirement_group(&conn, consumer, &requirement);

    let provider = insert_debian_repo_package(
        &conn,
        repository_id,
        "mail-provider",
        "1",
        "arm64",
        DebianMultiArch::No,
    );
    let mut provide = RepositoryProvide::new(
        provider,
        "mail-api".to_string(),
        None,
        "virtual".to_string(),
        Some("mail-api:any".to_string()),
        VersionScheme::Debian,
    )
    .with_architecture_qualifier(ProvideArchitectureQualifier::Any);
    provide.insert(&conn).unwrap();

    let result = solve_install(
        &conn,
        &[("mail-client".to_string(), VersionConstraint::Any)],
    )
    .unwrap();
    assert!(result.conflict_message.is_none(), "{result:?}");
    assert!(
        result
            .install_order
            .iter()
            .any(|package| package.name == "mail-provider")
    );
}

// ==========================================================================
// Task 11: Cross-distro SAT and policy regression tests
// ==========================================================================

use crate::db::models::{RepositoryProvide, RepositoryRequirementGroup};
use crate::repository::dependency_model::{
    RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::versioning::VersionScheme;

fn requirement_atom(name: &str) -> RepositoryRequirementExpression {
    RepositoryRequirementExpression::Atom(RepositoryRequirementClause::name_only(name.to_string()))
}

fn insert_requirement_expression(
    conn: &Connection,
    package_id: i64,
    expression: &RepositoryRequirementExpression,
) {
    let mut group = RepositoryRequirementGroup::new(
        package_id,
        "depends".to_string(),
        if expression.is_conditional() {
            "conditional".to_string()
        } else {
            "hard".to_string()
        },
        serde_json::to_string(expression).unwrap(),
    );
    group.insert(conn).unwrap();
}

fn insert_virtual_provide(
    conn: &Connection,
    package_id: i64,
    capability: &str,
    scheme: VersionScheme,
) {
    let mut provide = RepositoryProvide::new(
        package_id,
        capability.to_string(),
        None,
        "virtual".to_string(),
        None,
        scheme,
    );
    provide.insert(conn).unwrap();
}

fn insert_provide(conn: &Connection, trove_id: i64, capability: &str, version: Option<&str>) {
    use crate::db::models::ProvideEntry;
    let mut provide = ProvideEntry::new(
        trove_id,
        capability.to_string(),
        version.map(String::from),
        VersionScheme::Rpm,
    );
    provide.insert_or_ignore(conn).unwrap();
}

#[test]
fn test_removal_checks_provides_not_just_names() {
    let (_dir, conn) = setup_test_db();
    let id_a = insert_rpm_trove(&conn, "provider-a", "1.0.0", &[]);
    insert_provide(&conn, id_a, "virtual-cap", Some("1.0.0"));
    let id_b = insert_rpm_trove(&conn, "provider-b", "1.0.0", &[]);
    insert_provide(&conn, id_b, "virtual-cap", Some("1.0.0"));
    let _id_c = insert_rpm_trove(&conn, "consumer", "1.0.0", &[("virtual-cap", None)]);

    let breaking = solve_removal(&conn, &["provider-a".to_string()]).unwrap();
    assert!(
        breaking.is_empty(),
        "Should be safe (provider-b still provides virtual-cap), got: {breaking:?}"
    );
}

#[test]
fn test_removal_blocked_when_sole_provider() {
    let (_dir, conn) = setup_test_db();
    let id_a = insert_rpm_trove(&conn, "provider-a", "1.0.0", &[]);
    insert_provide(&conn, id_a, "virtual-cap", Some("1.0.0"));
    let _id_c = insert_rpm_trove(&conn, "consumer", "1.0.0", &[("virtual-cap", None)]);

    let breaking = solve_removal(&conn, &["provider-a".to_string()]).unwrap();
    assert!(
        breaking.contains(&"consumer".to_string()),
        "Removing sole provider should break consumer, got: {breaking:?}"
    );
}

#[test]
fn test_removal_with_soname_provides() {
    let (_dir, conn) = setup_test_db();
    let id_glibc = insert_rpm_trove(&conn, "glibc", "2.38", &[]);
    insert_provide(&conn, id_glibc, "libc.so.6", Some("2.38"));
    let _id_consumer = insert_rpm_trove(&conn, "curl", "8.0", &[("libc.so.6", None)]);
    let _id_other = insert_rpm_trove(&conn, "tree", "2.1", &[("libc.so.6", None)]);

    let breaking = solve_removal(&conn, &["tree".to_string()]).unwrap();
    assert!(
        breaking.is_empty(),
        "Removing tree should not break curl (glibc provides libc.so.6), got: {breaking:?}"
    );
}

#[test]
fn test_removal_uses_package_name_self_provide() {
    let (_dir, conn) = setup_test_db();
    insert_rpm_trove(&conn, "B", "1.0.0", &[]);
    let _id_a = insert_rpm_trove(&conn, "A", "1.0.0", &[("B", None)]);

    let breaking = solve_removal(&conn, &["B".to_string()]).unwrap();
    assert!(
        breaking.contains(&"A".to_string()),
        "Removing B should break A through B's exact package-name self-provide, got: {breaking:?}"
    );
}

#[test]
fn test_removal_both_providers_breaks_consumer() {
    let (_dir, conn) = setup_test_db();
    let id_a = insert_rpm_trove(&conn, "provider-a", "1.0.0", &[]);
    insert_provide(&conn, id_a, "virtual-cap", Some("1.0.0"));
    let id_b = insert_rpm_trove(&conn, "provider-b", "1.0.0", &[]);
    insert_provide(&conn, id_b, "virtual-cap", Some("1.0.0"));
    let _id_c = insert_rpm_trove(&conn, "consumer", "1.0.0", &[("virtual-cap", None)]);

    let breaking =
        solve_removal(&conn, &["provider-a".to_string(), "provider-b".to_string()]).unwrap();
    assert!(
        breaking.contains(&"consumer".to_string()),
        "Removing all providers should break consumer, got: {breaking:?}"
    );
}

#[test]
fn test_removal_does_not_attribute_preexisting_unsatisfied_capability_to_unrelated_package() {
    // A persisted dependency without a persisted provider is already
    // unsatisfied before this transaction. Removing an unrelated package must
    // not report that preexisting state as newly broken by the removal.
    let (_dir, conn) = setup_test_db();

    // "bash" has a soname dep that no conary package provides
    let _id_bash = insert_rpm_trove(
        &conn,
        "bash",
        "5.2",
        &[("libc.so.6(GLIBC_2.34)(64bit)", None)],
    );
    let _id_tree = insert_rpm_trove(&conn, "tree", "2.1", &[]);

    let breaking = solve_removal(&conn, &["tree".to_string()]).unwrap();
    assert!(
        breaking.is_empty(),
        "preexisting unsatisfied requirements are not caused by unrelated removal: {breaking:?}"
    );
}

#[path = "tests/canonical.rs"]
mod canonical;
#[path = "tests/formal_dependencies.rs"]
mod formal_dependencies;
#[path = "tests/relations.rs"]
mod relations;
#[path = "tests/root_requirements.rs"]
mod root_requirements;
