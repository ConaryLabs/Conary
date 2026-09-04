// crates/conary-core/src/db/models/repository_requirement/tests.rs

use super::*;
use crate::db::schema;
use rusqlite::Connection;

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    conn
}

fn seed_repo_and_package(conn: &Connection) {
    conn.execute(
        "INSERT INTO repositories (name, url) VALUES ('repo', 'https://example.test')",
        [],
    )
    .unwrap();
    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (1, 'pkg', '1.0', 'sha256:test', 1, 'https://example.test/pkg', 'rpm')",
            [],
        )
        .unwrap();
}

fn expression_json(name: &str) -> String {
    serde_json::to_string(
        &crate::repository::dependency_model::RepositoryRequirementExpression::Atom(
            crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                name.to_string(),
            ),
        ),
    )
    .unwrap()
}

fn seed_group(conn: &Connection, name: &str) -> i64 {
    let mut group = RepositoryRequirementGroup::new(
        1,
        "depends".to_string(),
        "hard".to_string(),
        expression_json(name),
    );
    group.insert(conn).unwrap()
}

#[test]
fn repository_requirement_round_trip() {
    let conn = test_db();
    seed_repo_and_package(&conn);
    let group_id = seed_group(&conn, "libmagic");

    let mut requirement = RepositoryRequirement::new(
        1,
        group_id,
        "libmagic".to_string(),
        Some(">= 1.0".to_string()),
        "package".to_string(),
        "runtime".to_string(),
        Some("libmagic >= 1.0".to_string()),
    );
    requirement.insert(&conn).unwrap();

    let found = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].capability, "libmagic");
    assert_eq!(found[0].version_constraint.as_deref(), Some(">= 1.0"));
}

#[test]
fn multi_package_requirement_lookup_chunks_at_the_runtime_variable_limit() {
    let conn = test_db();
    seed_repo_and_package(&conn);
    for package_id in 2..=5 {
        conn.execute(
            "INSERT INTO repository_packages
                 (id, repository_id, name, version, checksum, size, download_url, version_scheme)
                 VALUES (?1, 1, ?2, '1.0', 'sha256:test', 1, ?3, 'rpm')",
            params![
                package_id,
                format!("pkg-{package_id}"),
                format!("https://example.test/pkg-{package_id}")
            ],
        )
        .unwrap();
    }
    for package_id in 1..=5 {
        let mut group = RepositoryRequirementGroup::new(
            package_id,
            "depends".to_string(),
            "hard".to_string(),
            expression_json(&format!("dep-{package_id}")),
        );
        let group_id = group.insert(&conn).unwrap();
        let mut requirement = RepositoryRequirement::new(
            package_id,
            group_id,
            format!("dep-{package_id}"),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        );
        requirement.insert(&conn).unwrap();
    }
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 2)
        .unwrap();

    let ids = [5, 4, 3, 2, 1, 1];
    let groups = RepositoryRequirementGroup::find_by_repository_packages(&conn, &ids).unwrap();
    let requirements = RepositoryRequirement::find_by_repository_packages(&conn, &ids).unwrap();

    assert_eq!(groups.len(), 5);
    assert_eq!(requirements.len(), 5);
    assert_eq!(
        groups
            .iter()
            .map(|group| group.repository_package_id)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        requirements
            .iter()
            .map(|requirement| requirement.repository_package_id)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
}

#[test]
fn delete_by_package_removes_requirements() {
    let conn = test_db();
    seed_repo_and_package(&conn);
    let group_id = seed_group(&conn, "glibc");

    let mut req = RepositoryRequirement::new(
        1,
        group_id,
        "glibc".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    req.insert(&conn).unwrap();

    RepositoryRequirement::delete_by_package(&conn, 1).unwrap();
    let found = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
    assert!(found.is_empty());
}

#[test]
fn delete_by_repository_removes_requirements() {
    let conn = test_db();
    seed_repo_and_package(&conn);
    let group_id = seed_group(&conn, "glibc");

    let mut req = RepositoryRequirement::new(
        1,
        group_id,
        "glibc".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    req.insert(&conn).unwrap();

    RepositoryRequirement::delete_by_repository(&conn, 1).unwrap();
    let found = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
    assert!(found.is_empty());
}

#[test]
fn requirement_group_round_trip() {
    let conn = test_db();
    seed_repo_and_package(&conn);

    let mut group = RepositoryRequirementGroup::new(
        1,
        "depends".to_string(),
        "hard".to_string(),
        expression_json("default-mta"),
    );
    group.native_text = Some("default-mta | mail-transport-agent".to_string());
    group.insert(&conn).unwrap();
    assert!(group.id.is_some());

    let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, "depends");
    assert_eq!(found[0].behavior, "hard");
    assert_eq!(found[0].expression_json, expression_json("default-mta"));
    assert_eq!(
        found[0].native_text.as_deref(),
        Some("default-mta | mail-transport-agent"),
    );
}

#[test]
fn delete_groups_by_package() {
    let conn = test_db();
    seed_repo_and_package(&conn);

    let mut group = RepositoryRequirementGroup::new(
        1,
        "depends".to_string(),
        "hard".to_string(),
        expression_json("glibc"),
    );
    group.insert(&conn).unwrap();

    RepositoryRequirementGroup::delete_by_package(&conn, 1).unwrap();
    let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
    assert!(found.is_empty());
}

#[test]
fn delete_groups_by_repository() {
    let conn = test_db();
    seed_repo_and_package(&conn);

    let mut group = RepositoryRequirementGroup::new(
        1,
        "depends".to_string(),
        "hard".to_string(),
        expression_json("glibc"),
    );
    group.insert(&conn).unwrap();

    RepositoryRequirementGroup::delete_by_repository(&conn, 1).unwrap();
    let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
    assert!(found.is_empty());
}

#[test]
fn batch_insert_groups() {
    let conn = test_db();
    seed_repo_and_package(&conn);

    let groups = vec![
        RepositoryRequirementGroup::new(
            1,
            "depends".to_string(),
            "hard".to_string(),
            expression_json("glibc"),
        ),
        RepositoryRequirementGroup::new(
            1,
            "optional".to_string(),
            "hard".to_string(),
            expression_json("docs"),
        ),
    ];
    let count = RepositoryRequirementGroup::batch_insert(&conn, &groups).unwrap();
    assert_eq!(count, 2);

    let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
    assert_eq!(found.len(), 2);
}

#[test]
fn find_clauses_by_group() {
    let conn = test_db();
    seed_repo_and_package(&conn);

    // Create a group for an OR-dependency: default-mta | mail-transport-agent
    let mut group = RepositoryRequirementGroup::new(
        1,
        "depends".to_string(),
        "hard".to_string(),
        expression_json("default-mta"),
    );
    group.native_text = Some("default-mta | mail-transport-agent".to_string());
    group.insert(&conn).unwrap();
    let group_id = group.id.unwrap();

    // Insert two OR-alternative clauses linked to the group
    let mut clause_a = RepositoryRequirement::new(
        1,
        group_id,
        "default-mta".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    clause_a.insert(&conn).unwrap();

    let mut clause_b = RepositoryRequirement::new(
        1,
        group_id,
        "mail-transport-agent".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    clause_b.insert(&conn).unwrap();

    // Every searchable atom belongs to the authoritative exact group.
    let clauses = RepositoryRequirement::find_by_group(&conn, group_id).unwrap();
    assert_eq!(clauses.len(), 2);
    assert_eq!(clauses[0].capability, "default-mta");
    assert_eq!(clauses[1].capability, "mail-transport-agent");
    assert_eq!(clauses[0].group_id, group_id);

    // Package-level lookup returns the same group-linked atoms.
    let all = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
    assert_eq!(all.len(), 2);
}
