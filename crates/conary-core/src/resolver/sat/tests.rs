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

fn insert_repo_requirement_group(
    conn: &Connection,
    repository_package_id: i64,
    capability: &str,
    constraint: Option<&str>,
    native_text: Option<&str>,
) {
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
    group.insert(conn).unwrap();

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
    let mut provide =
        ProvideEntry::new(trove_id, capability.to_string(), version.map(String::from));
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
