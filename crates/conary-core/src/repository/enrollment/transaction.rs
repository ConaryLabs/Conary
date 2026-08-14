// conary-core/src/repository/enrollment/transaction.rs

//! SQLite planning and application for package-owned repository authority.

use super::{
    LastOwnerDisposition, PackageProjectionRole, PackageRepositoryEnrollmentIntent,
    RepositoryDefinition,
};
use crate::db::models::{Repository, RepositoryOwnership};
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::{BTreeMap, BTreeSet};

/// Validate one package transition against every owner that will remain after
/// the outgoing package identity is released. This performs no mutation.
pub fn preflight_transition(
    conn: &Connection,
    outgoing_trove_id: Option<i64>,
    desired: &[PackageRepositoryEnrollmentIntent],
) -> Result<()> {
    let desired = desired_authority(desired)?;
    for (name, (definition_hash, _)) in &desired.repositories {
        let Some(existing) = Repository::find_by_name(conn, name)? else {
            continue;
        };
        let repository_id = existing.id.ok_or_else(|| {
            Error::ParseError(format!("persisted repository '{name}' has no identity"))
        })?;
        if existing.managed_by != RepositoryOwnership::PackageProjection {
            return Err(Error::ConflictError(format!(
                "package repository enrollment cannot replace {name} owned by {}",
                existing.managed_by.as_str()
            )));
        }
        let remaining = remaining_definition_hashes(conn, repository_id, outgoing_trove_id)?;
        if remaining.is_empty() && !repository_has_any_owner(conn, repository_id)? {
            return Err(Error::ConflictError(format!(
                "package-projection repository '{name}' has no enrollment owner"
            )));
        }
        if remaining.iter().any(|existing| existing != definition_hash) {
            return Err(Error::ConflictError(format!(
                "package repository enrollment for '{name}' conflicts with an installed owner"
            )));
        }
    }
    for (path, (sha256, mode, role)) in &desired.projections {
        if conn
            .query_row(
                "SELECT 1 FROM native_repository_projections WHERE path = ?1 LIMIT 1",
                [path],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Err(Error::ConflictError(format!(
                "package repository projection '{path}' is already owned by native takeover authority"
            )));
        }
        let remaining = remaining_projection_authority(conn, path, outgoing_trove_id)?;
        if remaining
            .iter()
            .any(|existing| existing != &(sha256.clone(), *mode, *role))
        {
            return Err(Error::ConflictError(format!(
                "package repository projection '{path}' conflicts with an installed owner"
            )));
        }
    }
    Ok(())
}

/// Validate every incoming package in one atomic batch, including conflicts
/// between two packages that have not been persisted yet.
pub fn preflight_batch(
    conn: &Connection,
    transitions: &[(Option<i64>, &[PackageRepositoryEnrollmentIntent])],
) -> Result<()> {
    let mut repositories = BTreeMap::<String, String>::new();
    let mut projections = BTreeMap::<String, (String, u32, PackageProjectionRole)>::new();
    for (outgoing_trove_id, intents) in transitions {
        preflight_transition(conn, *outgoing_trove_id, intents)?;
        let desired = desired_authority(intents)?;
        for (name, (hash, _)) in desired.repositories {
            if let Some(existing) = repositories.insert(name.to_string(), hash.clone())
                && existing != hash
            {
                return Err(Error::ConflictError(format!(
                    "atomic package batch gives repository '{name}' conflicting definitions"
                )));
            }
        }
        for (path, authority) in desired.projections {
            if let Some(existing) = projections.insert(path.to_string(), authority.clone())
                && existing != authority
            {
                return Err(Error::ConflictError(format!(
                    "atomic package batch gives projection '{path}' conflicting authority"
                )));
            }
        }
    }
    Ok(())
}

/// Replace an outgoing package's enrollment set with the incoming package's
/// complete signed set inside the caller's package transaction.
pub fn apply_transition(
    tx: &Transaction<'_>,
    outgoing_trove_id: Option<i64>,
    incoming_trove_id: i64,
    package_name: &str,
    package_version: &str,
    desired: &[PackageRepositoryEnrollmentIntent],
) -> Result<()> {
    // Payload replacement may already have deleted the outgoing trove, whose
    // cascade removes its enrollment row. Reap only those transaction-local
    // package repositories before checking and installing the incoming set.
    remove_all_unowned_package_repositories(tx)?;
    preflight_transition(tx, outgoing_trove_id, desired)?;
    if let Some(trove_id) = outgoing_trove_id {
        tx.execute(
            "DELETE FROM package_repository_enrollments WHERE trove_id = ?1",
            [trove_id],
        )?;
    }
    remove_all_unowned_package_repositories(tx)?;
    persist_intents(
        tx,
        Some(incoming_trove_id),
        "package",
        package_name,
        package_version,
        desired,
    )
}

/// Release one removed package. Retained intents become explicit durable
/// owners; remove-when-unowned intents disappear and their repositories are
/// deleted only after the final owner is gone.
pub fn apply_removal(tx: &Transaction<'_>, trove_id: i64) -> Result<()> {
    let rows = enrollment_rows_for_trove(tx, trove_id)?;
    let affected = repository_ids_for_trove(tx, trove_id)?;
    for row in &rows {
        if row.intent.last_owner == LastOwnerDisposition::Retain {
            persist_intents(
                tx,
                None,
                "retained",
                &row.package_name,
                &row.package_version,
                std::slice::from_ref(&row.intent),
            )?;
        }
    }
    tx.execute(
        "DELETE FROM package_repository_enrollments WHERE trove_id = ?1",
        [trove_id],
    )?;
    remove_unowned_repositories(tx, &affected)?;
    remove_all_unowned_package_repositories(tx)
}

pub fn preflight_removal(conn: &Connection, trove_id: i64) -> Result<()> {
    for row in enrollment_rows_for_trove(conn, trove_id)? {
        row.intent.validate()?;
    }
    Ok(())
}

fn persist_intents(
    tx: &Transaction<'_>,
    trove_id: Option<i64>,
    owner_kind: &str,
    package_name: &str,
    package_version: &str,
    intents: &[PackageRepositoryEnrollmentIntent],
) -> Result<()> {
    let desired = desired_authority(intents)?;
    for intent in intents {
        let intent_sha256 = intent.sha256()?;
        let intent_json = serde_json::to_string(intent)?;
        tx.execute(
            "INSERT INTO package_repository_enrollments
             (trove_id, owner_kind, source_package, source_version, intent_id,
              intent_sha256, intent_json, last_owner)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                trove_id,
                owner_kind,
                package_name,
                package_version,
                &intent.intent_id,
                intent_sha256,
                intent_json,
                last_owner_value(intent.last_owner),
            ],
        )?;
        let enrollment_id = tx.last_insert_rowid();
        for definition in &intent.repositories {
            let definition_hash = definition_sha256(definition)?;
            let repository_id =
                if let Some(existing) = Repository::find_by_name(tx, &definition.name)? {
                    let id = existing.id.ok_or_else(|| {
                        Error::ParseError(format!(
                            "persisted repository '{}' has no identity",
                            definition.name
                        ))
                    })?;
                    if existing.managed_by != RepositoryOwnership::PackageProjection {
                        return Err(Error::ConflictError(format!(
                            "repository '{}' changed ownership after enrollment preflight",
                            definition.name
                        )));
                    }
                    let hashes = remaining_definition_hashes(tx, id, None)?;
                    if hashes.iter().any(|existing| existing != &definition_hash) {
                        return Err(Error::ConflictError(format!(
                            "repository '{}' changed definition after enrollment preflight",
                            definition.name
                        )));
                    }
                    id
                } else {
                    let mut repository =
                        definition.materialize_for(RepositoryOwnership::PackageProjection)?;
                    repository.insert(tx)?
                };
            tx.execute(
                "INSERT INTO package_repository_enrollment_members
                 (enrollment_id, repository_id, repository_name, definition_sha256)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    enrollment_id,
                    repository_id,
                    &definition.name,
                    definition_hash
                ],
            )?;
        }
        for projection in &intent.projections {
            let expected = desired
                .projections
                .get(projection.path.as_str())
                .expect("validated intent projection is in desired authority");
            let remaining = remaining_projection_authority(tx, &projection.path, None)?;
            if remaining.iter().any(|observed| observed != expected) {
                return Err(Error::ConflictError(format!(
                    "projection '{}' changed after enrollment preflight",
                    projection.path
                )));
            }
            tx.execute(
                "INSERT INTO package_repository_projection_owners
                 (enrollment_id, path, sha256, mode, role)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    enrollment_id,
                    &projection.path,
                    &projection.sha256,
                    projection.mode,
                    projection_role_value(projection.role),
                ],
            )?;
        }
    }
    Ok(())
}

struct DesiredAuthority<'a> {
    repositories: BTreeMap<&'a str, (String, &'a RepositoryDefinition)>,
    projections: BTreeMap<&'a str, (String, u32, PackageProjectionRole)>,
}

fn desired_authority(
    intents: &[PackageRepositoryEnrollmentIntent],
) -> Result<DesiredAuthority<'_>> {
    let mut intent_ids = BTreeSet::new();
    let mut repositories = BTreeMap::new();
    let mut projections = BTreeMap::new();
    for intent in intents {
        intent.validate()?;
        if !intent_ids.insert(intent.intent_id.as_str()) {
            return Err(Error::ConfigError(format!(
                "package repeats repository enrollment intent '{}'",
                intent.intent_id
            )));
        }
        for definition in &intent.repositories {
            let hash = definition_sha256(definition)?;
            if let Some((existing, _)) =
                repositories.insert(definition.name.as_str(), (hash.clone(), definition))
                && existing != hash
            {
                return Err(Error::ConfigError(format!(
                    "package gives repository '{}' conflicting definitions",
                    definition.name
                )));
            }
        }
        for projection in &intent.projections {
            let authority = (projection.sha256.clone(), projection.mode, projection.role);
            if let Some(existing) = projections.insert(projection.path.as_str(), authority.clone())
                && existing != authority
            {
                return Err(Error::ConfigError(format!(
                    "package gives projection '{}' conflicting authority",
                    projection.path
                )));
            }
        }
    }
    Ok(DesiredAuthority {
        repositories,
        projections,
    })
}

fn definition_sha256(definition: &RepositoryDefinition) -> Result<String> {
    definition.materialize_for(RepositoryOwnership::PackageProjection)?;
    Ok(crate::hash::sha256(&serde_json::to_vec(definition)?))
}

fn remaining_definition_hashes(
    conn: &Connection,
    repository_id: i64,
    outgoing_trove_id: Option<i64>,
) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT member.definition_sha256
         FROM package_repository_enrollment_members member
         JOIN package_repository_enrollments enrollment ON enrollment.id = member.enrollment_id
         WHERE member.repository_id = ?1
           AND (?2 IS NULL OR enrollment.trove_id IS NULL OR enrollment.trove_id != ?2)
         ORDER BY member.definition_sha256",
    )?;
    Ok(statement
        .query_map(params![repository_id, outgoing_trove_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn remaining_projection_authority(
    conn: &Connection,
    path: &str,
    outgoing_trove_id: Option<i64>,
) -> Result<Vec<(String, u32, PackageProjectionRole)>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT projection.sha256, projection.mode, projection.role
         FROM package_repository_projection_owners projection
         JOIN package_repository_enrollments enrollment ON enrollment.id = projection.enrollment_id
         WHERE projection.path = ?1
           AND (?2 IS NULL OR enrollment.trove_id IS NULL OR enrollment.trove_id != ?2)
         ORDER BY projection.sha256, projection.mode, projection.role",
    )?;
    Ok(statement
        .query_map(params![path, outgoing_trove_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                projection_role_from_db(row.get::<_, String>(2)?)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn repository_has_any_owner(conn: &Connection, repository_id: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM package_repository_enrollment_members WHERE repository_id = ?1 LIMIT 1",
            [repository_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn repository_ids_for_trove(conn: &Connection, trove_id: i64) -> Result<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT member.repository_id
         FROM package_repository_enrollment_members member
         JOIN package_repository_enrollments enrollment ON enrollment.id = member.enrollment_id
         WHERE enrollment.trove_id = ?1 ORDER BY member.repository_id",
    )?;
    Ok(statement
        .query_map([trove_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

fn remove_unowned_repositories(conn: &Connection, repository_ids: &[i64]) -> Result<()> {
    for repository_id in repository_ids {
        if !repository_has_any_owner(conn, *repository_id)? {
            Repository::delete(conn, *repository_id)?;
        }
    }
    Ok(())
}

fn remove_all_unowned_package_repositories(conn: &Connection) -> Result<()> {
    let repository_ids = {
        let mut statement = conn.prepare(
            "SELECT repository.id FROM repositories repository
             WHERE repository.managed_by = 'package-projection'
               AND NOT EXISTS (
                   SELECT 1 FROM package_repository_enrollment_members member
                   WHERE member.repository_id = repository.id
               )
             ORDER BY repository.id",
        )?;
        statement
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<i64>, _>>()?
    };
    for repository_id in repository_ids {
        Repository::delete(conn, repository_id)?;
    }
    Ok(())
}

struct EnrollmentRow {
    package_name: String,
    package_version: String,
    intent: PackageRepositoryEnrollmentIntent,
}

fn enrollment_rows_for_trove(conn: &Connection, trove_id: i64) -> Result<Vec<EnrollmentRow>> {
    let mut statement = conn.prepare(
        "SELECT source_package, source_version, intent_json, intent_sha256
         FROM package_repository_enrollments
         WHERE trove_id = ?1 ORDER BY intent_id",
    )?;
    let rows = statement
        .query_map([trove_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(package_name, package_version, json, expected)| {
            let intent: PackageRepositoryEnrollmentIntent = serde_json::from_str(&json)?;
            if intent.sha256()? != expected {
                return Err(Error::ParseError(format!(
                    "persisted package repository enrollment '{}' digest mismatch",
                    intent.intent_id
                )));
            }
            Ok(EnrollmentRow {
                package_name,
                package_version,
                intent,
            })
        })
        .collect()
}

fn last_owner_value(value: LastOwnerDisposition) -> &'static str {
    match value {
        LastOwnerDisposition::RemoveWhenUnowned => "remove-when-unowned",
        LastOwnerDisposition::Retain => "retain",
    }
}

fn projection_role_value(value: PackageProjectionRole) -> &'static str {
    match value {
        PackageProjectionRole::Declaration => "declaration",
        PackageProjectionRole::TrustRoot => "trust-root",
    }
}

fn projection_role_from_db(value: String) -> rusqlite::Result<PackageProjectionRole> {
    match value.as_str() {
        "declaration" => Ok(PackageProjectionRole::Declaration),
        "trust-root" => Ok(PackageProjectionRole::TrustRoot),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("unknown package repository projection role '{value}'").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Trove, TroveType};
    use crate::repository::versioning::VersionScheme;
    use base64::Engine;

    fn trove(conn: &Connection, name: &str, version: &str) -> i64 {
        Trove::new(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            VersionScheme::Rpm,
        )
        .insert(conn)
        .unwrap()
    }

    fn intent(name: &str, endpoint: &str) -> PackageRepositoryEnrollmentIntent {
        use crate::repository::enrollment::*;
        use crate::repository::{
            OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
        };
        let root = OpenPgpTrustRoot {
            url: format!(
                "data:application/pgp-keys;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(b"test")
            ),
            fingerprint: "A".repeat(40),
        };
        PackageRepositoryEnrollmentIntent {
            schema: PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA,
            intent_id: format!("intent-{name}"),
            repositories: vec![RepositoryDefinition {
                name: name.to_string(),
                source_identity: format!("source-{name}"),
                repository_identity: name.to_string(),
                scope: RepositoryPolicyScopeInput::Repository {
                    identity: name.to_string(),
                },
                stream: RepositorySourceStreamInput::Rolling {
                    identity: format!("stream-{name}"),
                },
                update: RepositoryUpdatePolicyInput::Follow,
                metadata_url: endpoint.to_string(),
                content_url: None,
                parser: RepositoryParserConfig::Rpm {
                    architecture: "x86_64".to_string(),
                },
                trust: RepositoryTrustPolicy::Rpm {
                    metadata: RpmMetadataAuthority::OpenPgp {
                        keys: vec![root.clone()],
                    },
                    package_keys: vec![root],
                },
                enabled: true,
                priority: 0,
                metadata_expire: 3600,
                source_profile: None,
            }],
            projections: vec![PackageProjectionReference {
                path: format!("/etc/yum.repos.d/{name}.repo"),
                sha256: crate::hash::sha256(endpoint.as_bytes()),
                mode: 0o644,
                role: PackageProjectionRole::Declaration,
            }],
            last_owner: LastOwnerDisposition::RemoveWhenUnowned,
        }
    }

    #[test]
    fn identical_owners_share_and_last_remove_deletes_repository() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let first = trove(&conn, "repo-release-a", "1");
        let second = trove(&conn, "repo-release-b", "1");
        let intent = intent("browser", "https://repo.example/rpm");

        let tx = conn.unchecked_transaction().unwrap();
        apply_transition(
            &tx,
            None,
            first,
            "repo-release-a",
            "1",
            std::slice::from_ref(&intent),
        )
        .unwrap();
        apply_transition(
            &tx,
            None,
            second,
            "repo-release-b",
            "1",
            std::slice::from_ref(&intent),
        )
        .unwrap();
        apply_removal(&tx, first).unwrap();
        assert!(Repository::find_by_name(&tx, "browser").unwrap().is_some());
        apply_removal(&tx, second).unwrap();
        assert!(Repository::find_by_name(&tx, "browser").unwrap().is_none());
        tx.commit().unwrap();
    }

    #[test]
    fn conflicting_second_owner_is_rejected_without_rows() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let first = trove(&conn, "repo-release-a", "1");
        let second = trove(&conn, "repo-release-b", "1");
        let original = intent("browser", "https://repo.example/one");
        let conflict = intent("browser", "https://repo.example/two");
        let tx = conn.unchecked_transaction().unwrap();
        apply_transition(&tx, None, first, "repo-release-a", "1", &[original]).unwrap();

        let error = preflight_transition(&tx, None, &[conflict]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("conflicts with an installed owner")
        );
        assert_eq!(
            tx.query_row(
                "SELECT COUNT(*) FROM package_repository_enrollments WHERE trove_id = ?1",
                [second],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn update_replaces_last_owner_and_retention_survives_removal() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let first = trove(&conn, "repo-release", "1");
        let second = trove(&conn, "repo-release", "2");
        let original = intent("browser", "https://repo.example/one");
        let mut replacement = intent("browser", "https://repo.example/two");
        replacement.last_owner = LastOwnerDisposition::Retain;
        let tx = conn.unchecked_transaction().unwrap();
        apply_transition(&tx, None, first, "repo-release", "1", &[original]).unwrap();

        apply_transition(
            &tx,
            Some(first),
            second,
            "repo-release",
            "2",
            std::slice::from_ref(&replacement),
        )
        .unwrap();
        assert_eq!(
            Repository::find_by_name(&tx, "browser")
                .unwrap()
                .unwrap()
                .url,
            "https://repo.example/two"
        );
        apply_removal(&tx, second).unwrap();

        assert!(Repository::find_by_name(&tx, "browser").unwrap().is_some());
        assert_eq!(
            tx.query_row(
                "SELECT COUNT(*) FROM package_repository_enrollments
                 WHERE owner_kind = 'retained' AND trove_id IS NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        tx.commit().unwrap();
    }

    #[test]
    fn native_takeover_projection_collision_fails_closed() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        conn.execute(
            "INSERT INTO native_repository_takeovers
             (selected_root, preview_sha256, preview_json)
             VALUES ('/root', ?1, '{}')",
            ["a".repeat(64)],
        )
        .unwrap();
        let takeover_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO native_repository_projections
             (takeover_id, path, content, sha256, mode, prior_existed, prior_content, prior_mode)
             VALUES (?1, '/etc/yum.repos.d/browser.repo', X'61', ?2, 420, 0, NULL, NULL)",
            params![takeover_id, crate::hash::sha256(b"a")],
        )
        .unwrap();

        let error = preflight_transition(
            &conn,
            None,
            &[intent("browser", "https://repo.example/rpm")],
        )
        .unwrap_err();

        assert!(error.to_string().contains("native takeover authority"));
        assert!(
            Repository::find_by_name(&conn, "browser")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn batch_preflight_rejects_conflicting_incoming_owners_before_mutation() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let first = intent("browser", "https://repo.example/one");
        let second = intent("browser", "https://repo.example/two");
        let transitions = vec![
            (None, std::slice::from_ref(&first)),
            (None, std::slice::from_ref(&second)),
        ];

        let error = preflight_batch(&conn, &transitions).unwrap_err();

        assert!(error.to_string().contains("atomic package batch"));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM package_repository_enrollments",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert!(
            Repository::find_by_name(&conn, "browser")
                .unwrap()
                .is_none()
        );
    }
}
