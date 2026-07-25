// conary-core/src/resolver/sat/tests/relations.rs

use super::*;
use crate::repository::dependency_model::RepositoryRequirementKind;
use crate::repository::package_relation::parse_native_relation;

fn insert_repo_package(
    conn: &Connection,
    name: &str,
    version: &str,
    scheme: VersionScheme,
) -> RepositoryPackage {
    let repository_id = match Repository::find_by_name(conn, "relations").unwrap() {
        Some(repository) => repository.id.unwrap(),
        None => {
            let mut repository = Repository::new(
                "relations".to_string(),
                "https://relations.invalid".to_string(),
            );
            repository.insert(conn).unwrap()
        }
    };
    let mut package = RepositoryPackage::new(
        repository_id,
        name.to_string(),
        version.to_string(),
        scheme,
        format!("sha256:{name}"),
        1,
        format!("https://relations.invalid/{name}"),
    );
    package.insert(conn).unwrap();
    package
}

fn insert_relation(
    conn: &Connection,
    package_id: i64,
    kind: RepositoryRequirementKind,
    scheme: VersionScheme,
    native_text: &str,
) {
    let relation = parse_native_relation(kind, scheme, native_text).unwrap();
    let mut group = RepositoryRequirementGroup::new(
        package_id,
        kind.as_str().to_string(),
        if relation.expression.is_conditional() {
            "conditional".to_string()
        } else {
            "hard".to_string()
        },
        serde_json::to_string(&relation.expression).unwrap(),
    );
    group.native_text = relation.native_text;
    group.insert(conn).unwrap();
}

#[test]
fn repository_replace_produces_typed_ownership_transfer_removal() {
    let (_temp, conn) = setup_test_db();
    let old_id = insert_rpm_trove(&conn, "oldpkg", "1.0-1", &[]);
    let incoming = insert_repo_package(&conn, "newpkg", "2.0-1", VersionScheme::Rpm);
    insert_relation(
        &conn,
        incoming.id.unwrap(),
        RepositoryRequirementKind::Obsolete,
        VersionScheme::Rpm,
        "oldpkg < 2",
    );

    let resolution =
        solve_install(&conn, &[("newpkg".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert_eq!(resolution.remove_order.len(), 1);
    assert_eq!(resolution.remove_order[0].trove_id, old_id);
    assert_eq!(
        resolution.remove_order[0].mode,
        crate::repository::dependency_model::PackageRelationRemovalMode::OwnershipTransfer
    );
}

#[test]
fn repository_conflict_produces_constraint_removal() {
    let (_temp, conn) = setup_test_db();
    let old_id = insert_rpm_trove(&conn, "oldpkg", "1.0-1", &[]);
    let incoming = insert_repo_package(&conn, "newpkg", "2.0-1", VersionScheme::Rpm);
    insert_relation(
        &conn,
        incoming.id.unwrap(),
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "oldpkg",
    );

    let resolution =
        solve_install(&conn, &[("newpkg".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert_eq!(resolution.remove_order.len(), 1);
    assert_eq!(resolution.remove_order[0].trove_id, old_id);
    assert_eq!(
        resolution.remove_order[0].mode,
        crate::repository::dependency_model::PackageRelationRemovalMode::Constraint
    );
}

#[test]
fn rich_conflict_chooses_one_exact_installed_conjunct() {
    let (_temp, conn) = setup_test_db();
    let first = insert_rpm_trove(&conn, "old-a", "1", &[]);
    insert_rpm_trove(&conn, "old-b", "1", &[]);
    let incoming = insert_repo_package(&conn, "newpkg", "2", VersionScheme::Rpm);
    insert_relation(
        &conn,
        incoming.id.unwrap(),
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "(old-a and old-b)",
    );

    let resolution =
        solve_install(&conn, &[("newpkg".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert_eq!(resolution.remove_order.len(), 1);
    assert_eq!(resolution.remove_order[0].trove_id, first);
}

#[test]
fn co_selected_conflict_is_unsatisfiable_instead_of_silent_removal() {
    let (_temp, conn) = setup_test_db();
    insert_rpm_trove(&conn, "oldpkg", "1", &[]);
    let incoming = insert_repo_package(&conn, "newpkg", "2", VersionScheme::Rpm);
    insert_relation(
        &conn,
        incoming.id.unwrap(),
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "oldpkg",
    );

    let resolution = solve_install(
        &conn,
        &[
            ("newpkg".to_string(), VersionConstraint::Any),
            ("oldpkg".to_string(), VersionConstraint::Any),
        ],
    )
    .unwrap();

    assert!(resolution.conflict_message.is_some(), "{resolution:?}");
    assert!(resolution.remove_order.is_empty());
}

#[test]
fn versioned_relation_never_compares_different_native_schemes() {
    let (_temp, conn) = setup_test_db();
    insert_rpm_trove(&conn, "oldpkg", "1.0-1", &[]);
    let incoming = insert_repo_package(&conn, "newpkg", "2.0-1", VersionScheme::Debian);
    insert_relation(
        &conn,
        incoming.id.unwrap(),
        RepositoryRequirementKind::Replace,
        VersionScheme::Debian,
        "oldpkg (<< 9)",
    );

    let resolution =
        solve_install(&conn, &[("newpkg".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert!(resolution.remove_order.is_empty());
}

#[test]
fn installed_rich_conflict_attributes_exact_combined_selected_cause() {
    let (_temp, conn) = setup_test_db();
    let declaring = insert_rpm_trove(&conn, "oldpkg", "1", &[]);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "(new-a and new-b)",
    )
    .unwrap();
    crate::db::models::InstalledRequirementGroup::insert_groups(
        &conn,
        declaring,
        VersionScheme::Rpm,
        &[relation],
    )
    .unwrap();
    insert_repo_package(&conn, "new-a", "1", VersionScheme::Rpm);
    insert_repo_package(&conn, "new-b", "1", VersionScheme::Rpm);

    let resolution = solve_install(
        &conn,
        &[
            ("new-a".to_string(), VersionConstraint::Any),
            ("new-b".to_string(), VersionConstraint::Any),
        ],
    )
    .unwrap();

    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert_eq!(resolution.remove_order.len(), 1);
    assert_eq!(resolution.remove_order[0].trove_id, declaring);
    assert_eq!(
        resolution.remove_order[0]
            .authorized_by
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["new-a", "new-b"]
    );
}
