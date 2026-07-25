// src/commands/ccs/install/dependency.rs

//! Exact installed-dependent validation for CCS replacement installs.

use anyhow::Result;
use conary_core::db::models::InstalledRequirementGroup;
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use conary_core::repository::versioning::VersionScheme;

pub(super) fn validate_incoming_version_against_dependents(
    conn: &rusqlite::Connection,
    package_name: &str,
    incoming_version: &str,
    incoming_version_scheme: VersionScheme,
) -> Result<()> {
    let before = conary_core::resolver::load_installed_package_identities(conn)?;
    let mut after = before.clone();
    let mut replaced = false;
    for package in &mut after {
        if package.name == package_name {
            package.version = incoming_version.to_string();
            package.version_scheme = incoming_version_scheme;
            replaced = true;
        }
    }
    if !replaced {
        return Ok(());
    }

    let mut violations = Vec::new();
    for dependent in &before {
        let Some(trove_id) = dependent.installed_trove_id else {
            continue;
        };
        for stored in InstalledRequirementGroup::find_by_trove(conn, trove_id)? {
            if !matches!(
                stored.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            ) {
                continue;
            }
            let was_satisfied = conary_core::resolver::requirement_expression_satisfied(
                &stored.requirement.expression,
                stored.version_scheme,
                &before,
            )?;
            let remains_satisfied = conary_core::resolver::requirement_expression_satisfied(
                &stored.requirement.expression,
                stored.version_scheme,
                &after,
            )?;
            if was_satisfied && !remains_satisfied {
                let requirement = stored
                    .requirement
                    .native_text
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", stored.requirement.expression));
                violations.push(format!("{} requires {}", dependent.name, requirement));
            }
        }
    }

    if violations.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "dependency version mismatch: {} {} would break {}",
        package_name,
        incoming_version,
        violations.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Trove, TroveType};
    use conary_core::repository::dependency_model::{
        RepositoryRequirementClause, RepositoryRequirementExpression, RepositoryRequirementGroup,
    };

    fn database() -> (tempfile::TempDir, rusqlite::Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("conary.db");
        conary_core::db::init(&path).unwrap();
        (temp, conary_core::db::open(&path).unwrap())
    }

    fn package(
        conn: &rusqlite::Connection,
        name: &str,
        version: &str,
        scheme: VersionScheme,
    ) -> i64 {
        let mut trove = Trove::new(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            scheme,
        );
        trove.insert(conn).unwrap()
    }

    #[test]
    fn incoming_version_is_checked_against_typed_installed_groups() {
        let (_temp, conn) = database();
        package(&conn, "dep-liba", "1.5", VersionScheme::Conary);
        let app = package(&conn, "dep-app", "1", VersionScheme::Conary);
        let requirement = conary_core::repository::requirement::parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Conary,
            "dep-liba < 2",
        )
        .unwrap();
        InstalledRequirementGroup::insert_groups(&conn, app, VersionScheme::Conary, &[requirement])
            .unwrap();

        let error = validate_incoming_version_against_dependents(
            &conn,
            "dep-liba",
            "2.0",
            VersionScheme::Conary,
        )
        .unwrap_err();
        assert!(error.to_string().contains("dep-app requires dep-liba < 2"));
        validate_incoming_version_against_dependents(
            &conn,
            "dep-liba",
            "1.9",
            VersionScheme::Conary,
        )
        .unwrap();
    }

    #[test]
    fn exact_alternative_prevents_false_upgrade_breakage() {
        let (_temp, conn) = database();
        package(&conn, "dep-liba", "1.5", VersionScheme::Rpm);
        package(&conn, "dep-libb", "3", VersionScheme::Rpm);
        let app = package(&conn, "dep-app", "1", VersionScheme::Rpm);
        let left =
            RepositoryRequirementClause::versioned("dep-liba".to_string(), "< 2".to_string());
        let right =
            RepositoryRequirementClause::versioned("dep-libb".to_string(), ">= 3".to_string());
        let mut requirement =
            RepositoryRequirementGroup::simple(RepositoryRequirementKind::Depends, left.clone())
                .with_expression(RepositoryRequirementExpression::Or(vec![
                    RepositoryRequirementExpression::Atom(left.clone()),
                    RepositoryRequirementExpression::Atom(right.clone()),
                ]));
        requirement.alternatives = vec![left, right];
        InstalledRequirementGroup::insert_groups(&conn, app, VersionScheme::Rpm, &[requirement])
            .unwrap();

        validate_incoming_version_against_dependents(&conn, "dep-liba", "2.0", VersionScheme::Rpm)
            .unwrap();
    }

    #[test]
    fn installed_schema_has_no_flat_dependency_rows() {
        let (_temp, conn) = database();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'dependencies'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
