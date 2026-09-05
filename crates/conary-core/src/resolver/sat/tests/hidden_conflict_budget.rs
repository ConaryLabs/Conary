// crates/conary-core/src/resolver/sat/tests/hidden_conflict_budget.rs

use super::*;
use resolvo::DependencyProvider;

fn missing_fixture(
    n: u32,
) -> (
    tempfile::TempDir,
    Connection,
    i64,
    Vec<SatUnresolvedDependency>,
) {
    let (directory, conn) = setup_test_db();
    let mut repo = Repository::new(
        "fedora-44".to_string(),
        "https://example.invalid/fedora".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    let repository_id = repo.insert(&conn).unwrap();
    let root = insert_rpm_repo_package(&conn, repository_id, "many-missing", "1-1");
    let dependencies = (0..n)
        .map(|i| SatUnresolvedDependency {
            repository_package_id: root,
            repository_requirement_group_id: insert_repo_requirement_group(
                &conn,
                root,
                &format!("missing-{i:03}"),
                None,
                None,
            ),
        })
        .collect();
    (directory, conn, root, dependencies)
}

#[test]
fn independent_missing_groups_are_complete_below_probe_budget() {
    let (_directory, conn, root, dependencies) = missing_fixture(16);
    let result = solve_exact_repository_package_with_policy(
        &conn,
        root,
        "x86_64",
        &ResolutionPolicy::new().with_primary_source_identity("fedora-44"),
    )
    .unwrap();
    assert_eq!(result, SatExactResolution::Unresolved { dependencies });
    assert_eq!(hidden_conflict::counts(), (16, 2));
    println!("16 missing groups: Unresolved, 16 re-solves, 2 total provider loads");
}

#[test]
fn independent_missing_groups_above_budget_fail_without_classification() {
    let n = hidden_conflict::MAX_HIDDEN_CONFLICT_RESOLVES + 1;
    let (_directory, conn, root, _) = missing_fixture(n);
    let result = solve_exact_repository_package_with_policy(
        &conn,
        root,
        "x86_64",
        &ResolutionPolicy::new().with_primary_source_identity("fedora-44"),
    );
    assert!(
        matches!(result, Err(Error::HiddenConflictProbeBudgetExceeded {
        root, resolves: 64, elapsed,
    }) if root == "many-missing" && elapsed < hidden_conflict::HIDDEN_CONFLICT_PROBE_TIMEOUT)
    );
    assert_eq!(hidden_conflict::counts(), (64, 2));
    println!("65 missing groups: typed failure, 64 re-solves, 2 total provider loads, no outcome");
}

#[test]
fn native_solver_cooperatively_observes_the_probe_deadline() {
    let (_directory, conn, root, _) = missing_fixture(1);
    let policy = ResolutionPolicy::new().with_primary_source_identity("fedora-44");
    let mut provider = install::build_provider_for_install(
        &conn,
        &[("many-missing".to_string(), VersionConstraint::Any)],
        &policy,
    )
    .unwrap();
    provider.probe_deadline = Some(std::time::Instant::now());
    let exact = provider
        .intern_exact_repository_package("many-missing", root)
        .unwrap();
    assert!(provider.should_cancel_with_value().is_some());
    let mut solver = Solver::new(&provider);
    assert!(matches!(
        solver.solve(Problem::new().requirements(vec![exact.into()])),
        Err(UnsolvableOrCancelled::Cancelled(_))
    ));
}
