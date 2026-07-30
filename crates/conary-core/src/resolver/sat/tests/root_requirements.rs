// conary-core/src/resolver/sat/tests/root_requirements.rs

use super::formal_dependencies::insert_repo_pkg_with_reqs;
use super::*;
use crate::db::models::RepositoryProvide;
use crate::repository::dependency_model::RepositoryRequirementKind;
use crate::repository::requirement::parse_native_requirement;

fn repository(conn: &Connection) -> i64 {
    let mut repository = Repository::new(
        "rich-root".to_string(),
        "https://rich-root.invalid".to_string(),
    );
    repository.insert(conn).unwrap()
}

fn package(conn: &Connection, repository_id: i64, name: &str, provides: &[&str]) {
    let package_id = insert_repo_pkg_with_reqs(
        conn,
        repository_id,
        name,
        "1-1",
        &format!("https://rich-root.invalid/{name}.rpm"),
        "rpm",
        &[],
    );
    for capability in provides {
        RepositoryProvide::new(
            package_id,
            (*capability).to_string(),
            None,
            "virtual".to_string(),
            None,
            VersionScheme::Rpm,
        )
        .insert(conn)
        .unwrap();
    }
}

fn solve_rpm_root(conn: &Connection, native: &str) -> SatResolution {
    solve_rpm_roots(conn, &[native])
}

fn solve_rpm_roots(conn: &Connection, native: &[&str]) -> SatResolution {
    let groups = native
        .iter()
        .map(|native| {
            parse_native_requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Rpm,
                native,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    solve_requirement_groups_with_policy(
        conn,
        &groups,
        VersionScheme::Rpm,
        &ResolutionPolicy::new()
            .with_mixing(crate::repository::resolution_policy::DependencyMixingPolicy::Permissive),
    )
    .unwrap()
}

fn selected_names(result: &SatResolution) -> Vec<&str> {
    result
        .install_order
        .iter()
        .map(|package| package.name.as_str())
        .collect()
}

#[test]
fn root_and_or_and_version_constraints_reach_sat_exactly() {
    let (_temp, conn) = setup_test_db();
    let repository_id = repository(&conn);
    package(&conn, repository_id, "alpha", &[]);
    package(&conn, repository_id, "beta", &[]);
    package(&conn, repository_id, "gamma", &[]);

    let result = solve_rpm_root(&conn, "(alpha >= 1 and (beta or gamma))");
    assert!(result.conflict_message.is_none(), "{result:?}");
    let names = selected_names(&result);
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta") || names.contains(&"gamma"));
}

#[test]
fn root_if_else_preserves_boolean_semantics() {
    let (_temp, conn) = setup_test_db();
    let repository_id = repository(&conn);
    for name in ["condition", "required", "alternate"] {
        package(&conn, repository_id, name, &[]);
    }

    let if_result = solve_rpm_roots(&conn, &["(required if condition)", "condition"]);
    assert!(
        if_result.conflict_message.is_none(),
        "{:?}",
        if_result.conflict_message
    );
    let if_names = selected_names(&if_result);
    assert!(if_names.contains(&"condition"));
    assert!(if_names.contains(&"required"));
}

#[test]
fn root_if_missing_required_returns_conflict_without_panic() {
    let (_temp, conn) = setup_test_db();
    let repository_id = repository(&conn);
    package(&conn, repository_id, "condition", &[]);

    let result = solve_rpm_roots(&conn, &["(required if condition)", "condition"]);
    let conflict = result
        .conflict_message
        .as_ref()
        .expect("triggered conditional with no required provider must be unsatisfiable");

    assert!(!conflict.trim().is_empty());
    assert!(result.install_order.is_empty(), "{result:?}");
}

#[test]
fn root_with_and_without_require_same_provider_facts() {
    let (_temp, conn) = setup_test_db();
    let repository_id = repository(&conn);
    package(&conn, repository_id, "split-a", &["feature-a"]);
    package(&conn, repository_id, "split-b", &["feature-b"]);
    package(
        &conn,
        repository_id,
        "combined",
        &["feature-a", "feature-b"],
    );
    package(&conn, repository_id, "left-only", &["feature-a"]);

    let with_result = solve_rpm_root(&conn, "(feature-a with feature-b)");
    assert!(
        with_result.conflict_message.is_none(),
        "{:?}",
        with_result.conflict_message
    );
    let with_names = selected_names(&with_result);
    assert!(with_names.contains(&"combined"));
    assert!(!(with_names.contains(&"split-a") && with_names.contains(&"split-b")));

    let without_result = solve_rpm_root(&conn, "(feature-a without feature-b)");
    assert!(
        without_result.conflict_message.is_none(),
        "{:?}",
        without_result.conflict_message
    );
    let without_names = selected_names(&without_result);
    assert!(without_names.contains(&"left-only") || without_names.contains(&"split-a"));
    assert!(!without_names.contains(&"combined"));
}
