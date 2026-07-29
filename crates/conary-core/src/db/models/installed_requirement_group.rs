// conary-core/src/db/models/installed_requirement_group.rs

//! The single typed requirement authority for installed packages.

use crate::error::{Error, Result};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::distro::version_scheme_from_db;
use crate::repository::requirement::validate_requirement_group;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, Row, params};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRequirementGroup {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub kind: RepositoryRequirementKind,
    pub version_scheme: VersionScheme,
    pub requirement: RepositoryRequirementGroup,
}

impl InstalledRequirementGroup {
    const COLUMNS: &'static str = "id, trove_id, kind, version_scheme, requirement_json";

    pub fn from_group(
        trove_id: i64,
        version_scheme: VersionScheme,
        requirement: RepositoryRequirementGroup,
    ) -> Result<Self> {
        validate_requirement_group(&requirement, version_scheme).map_err(Error::ParseError)?;
        Ok(Self {
            id: None,
            trove_id,
            kind: requirement.kind,
            version_scheme,
            requirement,
        })
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        validate_requirement_group(&self.requirement, self.version_scheme)
            .map_err(Error::ParseError)?;
        if self.kind != self.requirement.kind {
            return Err(Error::ParseError(
                "installed requirement kind disagrees with its typed payload".to_string(),
            ));
        }
        let requirement_json = serde_json::to_string(&self.requirement)
            .map_err(|error| Error::ParseError(error.to_string()))?;
        conn.execute(
            "INSERT INTO package_requirement_groups
             (trove_id, kind, version_scheme, requirement_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                self.trove_id,
                self.kind.as_str(),
                self.version_scheme.as_str(),
                requirement_json,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    pub fn insert_groups(
        conn: &Connection,
        trove_id: i64,
        version_scheme: VersionScheme,
        requirements: &[RepositoryRequirementGroup],
    ) -> Result<()> {
        let mut inserted = HashSet::new();
        for requirement in requirements {
            if inserted.insert(requirement) {
                Self::from_group(trove_id, version_scheme, requirement.clone())?.insert(conn)?;
            }
        }
        Ok(())
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM package_requirement_groups
             WHERE trove_id = ?1 ORDER BY id",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        statement
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM package_requirement_groups ORDER BY trove_id, id",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        statement
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM package_requirement_groups WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM package_requirement_groups WHERE trove_id = ?1",
            [trove_id],
        )?;
        Ok(())
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind_raw: String = row.get(2)?;
        let scheme_raw: String = row.get(3)?;
        let requirement_json: String = row.get(4)?;
        let kind = RepositoryRequirementKind::from_str_exact(&kind_raw).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid installed requirement kind '{kind_raw}'"),
                )
                .into(),
            )
        })?;
        let version_scheme = version_scheme_from_db(Some(&scheme_raw)).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid installed requirement version scheme '{scheme_raw}'"),
                )
                .into(),
            )
        })?;
        let requirement: RepositoryRequirementGroup = serde_json::from_str(&requirement_json)
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        if requirement.kind != kind {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "installed requirement kind disagrees with requirement_json",
                )
                .into(),
            ));
        }
        validate_requirement_group(&requirement, version_scheme).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                std::io::Error::new(std::io::ErrorKind::InvalidData, error).into(),
            )
        })?;
        Ok(Self {
            id: row.get(0)?,
            trove_id: row.get(1)?,
            kind,
            version_scheme,
            requirement,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Trove, TroveType};
    use crate::db::testing::create_test_db;
    use crate::repository::package_relation::parse_native_relation;
    use crate::repository::requirement::parse_native_requirement;

    #[test]
    fn exact_requirement_round_trips_through_installed_state() {
        let (_temp, conn) = create_test_db();
        let mut trove = Trove::new(
            "new-libssl".to_string(),
            "3.0-1".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let relation = parse_native_relation(
            RepositoryRequirementKind::Replace,
            VersionScheme::Debian,
            "old-libssl (<< 3.0)",
        )
        .unwrap();

        InstalledRequirementGroup::insert_groups(
            &conn,
            trove_id,
            VersionScheme::Debian,
            std::slice::from_ref(&relation),
        )
        .unwrap();

        let stored = InstalledRequirementGroup::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].requirement, relation);
        assert_eq!(stored[0].version_scheme, VersionScheme::Debian);
    }

    #[test]
    fn rich_positive_requirement_round_trips_through_installed_state() {
        let (_temp, conn) = create_test_db();
        let mut trove = Trove::new(
            "rich-consumer".to_string(),
            "1-1".to_string(),
            TroveType::Package,
            VersionScheme::Rpm,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let requirement = parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Rpm,
            "((feature-a with feature-b) if engine else fallback)",
        )
        .unwrap();

        InstalledRequirementGroup::insert_groups(
            &conn,
            trove_id,
            VersionScheme::Rpm,
            std::slice::from_ref(&requirement),
        )
        .unwrap();

        let stored = InstalledRequirementGroup::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].requirement, requirement);
        assert_eq!(stored[0].kind, RepositoryRequirementKind::Depends);
    }

    #[test]
    fn repeated_requirement_facts_persist_once_in_first_seen_order() {
        let (_temp, conn) = create_test_db();
        let mut trove = Trove::new(
            "repeated-requirements".to_string(),
            "1-1".to_string(),
            TroveType::Package,
            VersionScheme::Rpm,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let first = parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Rpm,
            "/bin/sh",
        )
        .unwrap();
        let second = parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Rpm,
            "systemd-units",
        )
        .unwrap();

        InstalledRequirementGroup::insert_groups(
            &conn,
            trove_id,
            VersionScheme::Rpm,
            &[first.clone(), second.clone(), first.clone()],
        )
        .unwrap();

        let stored = InstalledRequirementGroup::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].requirement, first);
        assert_eq!(stored[1].requirement, second);
    }
}
