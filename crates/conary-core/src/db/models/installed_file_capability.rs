// conary-core/src/db/models/installed_file_capability.rs

//! Installed CCS file capability authority persisted for generation inputs.

use crate::ccs::manifest::FileCapability;
use anyhow::{Context, bail};
use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFileCapability {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub path: String,
    pub capabilities: Vec<String>,
    pub capabilities_json: String,
    pub permitted: bool,
    pub effective: bool,
    pub inheritable: bool,
    pub created_at: Option<String>,
}

impl InstalledFileCapability {
    const COLUMNS: &'static str = "id, trove_id, path, capabilities_json, permitted, effective, \
         inheritable, created_at";

    pub fn replace_for_trove(
        conn: &Connection,
        trove_id: i64,
        capabilities: &[FileCapability],
    ) -> anyhow::Result<()> {
        let rows = capabilities
            .iter()
            .map(|capability| Self::from_manifest_capability(trove_id, capability))
            .collect::<anyhow::Result<Vec<_>>>()?;

        conn.execute(
            "DELETE FROM installed_file_capabilities WHERE trove_id = ?1",
            [trove_id],
        )?;

        for row in rows {
            conn.execute(
                "INSERT INTO installed_file_capabilities (
                    trove_id, path, capabilities_json, permitted, effective, inheritable
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(trove_id, path) DO UPDATE SET
                    capabilities_json = excluded.capabilities_json,
                    permitted = excluded.permitted,
                    effective = excluded.effective,
                    inheritable = excluded.inheritable",
                params![
                    row.trove_id,
                    row.path,
                    row.capabilities_json,
                    row.permitted,
                    row.effective,
                    row.inheritable,
                ],
            )?;
        }

        Ok(())
    }

    pub fn find_all_ordered(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM installed_file_capabilities ORDER BY trove_id, path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> anyhow::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM installed_file_capabilities WHERE trove_id = ?1 ORDER BY path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([trove_id], Self::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn from_manifest_capability(
        trove_id: i64,
        capability: &FileCapability,
    ) -> anyhow::Result<Self> {
        let normalized = Self::normalized_manifest_capability(capability)?;
        let capabilities_json = serde_json::to_string(&normalized.capabilities)
            .context("serialize installed file capabilities")?;

        Ok(Self {
            id: None,
            trove_id,
            path: normalized.path,
            capabilities: normalized.capabilities,
            capabilities_json,
            permitted: normalized.permitted,
            effective: normalized.effective,
            inheritable: normalized.inheritable,
            created_at: None,
        })
    }

    fn normalized_manifest_capability(
        capability: &FileCapability,
    ) -> anyhow::Result<FileCapability> {
        let capabilities = normalize_capability_names(&capability.capabilities)?;
        let normalized = FileCapability {
            path: capability.path.clone(),
            capabilities,
            permitted: capability.permitted,
            effective: capability.effective,
            inheritable: capability.inheritable,
        };
        normalized
            .validate()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(normalized)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let capabilities_json: String = row.get(3)?;
        let capabilities = normalize_capability_json(&capabilities_json)
            .map_err(|error| row_conversion_error(3, error))?;
        let capability = FileCapability {
            path: row.get(2)?,
            capabilities,
            permitted: row.get::<_, i64>(4)? != 0,
            effective: row.get::<_, i64>(5)? != 0,
            inheritable: row.get::<_, i64>(6)? != 0,
        };
        let normalized = Self::normalized_manifest_capability(&capability)
            .map_err(|error| row_conversion_error(3, error))?;
        let capabilities_json = serde_json::to_string(&normalized.capabilities)
            .map_err(|error| row_conversion_error(3, error))?;

        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            path: normalized.path,
            capabilities: normalized.capabilities,
            capabilities_json,
            permitted: normalized.permitted,
            effective: normalized.effective,
            inheritable: normalized.inheritable,
            created_at: row.get(7)?,
        })
    }
}

fn normalize_capability_json(value: &str) -> anyhow::Result<Vec<String>> {
    let capabilities: Vec<String> =
        serde_json::from_str(value).context("parse installed file capabilities JSON")?;
    normalize_capability_names(&capabilities)
}

fn normalize_capability_names(capabilities: &[String]) -> anyhow::Result<Vec<String>> {
    let normalized = capabilities
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        bail!("installed file capability rows require at least one capability");
    }
    Ok(normalized)
}

fn row_conversion_error(column: usize, error: impl ToString) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use crate::ccs::manifest::FileCapability;
    use crate::db::models::{InstalledFileCapability, Trove, TroveType};
    use crate::db::testing::create_test_db;

    fn fixture_trove(conn: &rusqlite::Connection) -> i64 {
        let mut trove = Trove::new(
            "capability-fixture".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.insert(conn).expect("insert trove")
    }

    fn bind_service_capability(path: &str) -> FileCapability {
        FileCapability {
            path: path.to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }
    }

    #[test]
    fn installed_file_capability_round_trips_ordered_rows() {
        let (_tmp, conn) = create_test_db();
        let trove_id = fixture_trove(&conn);

        InstalledFileCapability::replace_for_trove(
            &conn,
            trove_id,
            &[bind_service_capability("/usr/bin/server")],
        )
        .expect("replace capabilities");

        let rows = InstalledFileCapability::find_by_trove(&conn, trove_id)
            .expect("find capabilities for trove");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].trove_id, trove_id);
        assert_eq!(rows[0].path, "/usr/bin/server");
        assert_eq!(rows[0].capabilities, vec!["cap_net_bind_service"]);
        assert_eq!(rows[0].capabilities_json, "[\"cap_net_bind_service\"]");
        assert!(rows[0].permitted);
        assert!(rows[0].effective);
        assert!(!rows[0].inheritable);

        let all = InstalledFileCapability::find_all_ordered(&conn)
            .expect("find all installed file capabilities");
        assert_eq!(all, rows);
    }

    #[test]
    fn installed_file_capability_replace_is_idempotent_and_normalizes_names() {
        let (_tmp, conn) = create_test_db();
        let trove_id = fixture_trove(&conn);
        let mut capability = bind_service_capability("/usr/bin/server");
        capability.capabilities = vec![
            "cap_net_raw".to_string(),
            "cap_net_bind_service".to_string(),
            "cap_net_raw".to_string(),
        ];

        InstalledFileCapability::replace_for_trove(&conn, trove_id, &[capability])
            .expect("first replace");
        InstalledFileCapability::replace_for_trove(
            &conn,
            trove_id,
            &[bind_service_capability("/usr/bin/server")],
        )
        .expect("second replace");

        let rows = InstalledFileCapability::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].capabilities, vec!["cap_net_bind_service"]);
    }

    #[test]
    fn installed_file_capability_rejects_invalid_manifest_capability() {
        let (_tmp, conn) = create_test_db();
        let trove_id = fixture_trove(&conn);
        let mut capability = bind_service_capability("/usr/bin/server");
        capability.effective = true;
        capability.permitted = false;

        let error = InstalledFileCapability::replace_for_trove(&conn, trove_id, &[capability])
            .expect_err("manifest validation must reject invalid flags");

        assert!(error.to_string().contains("requires permitted"));
    }

    #[test]
    fn installed_file_capability_cascades_when_trove_is_deleted() {
        let (_tmp, conn) = create_test_db();
        let trove_id = fixture_trove(&conn);
        InstalledFileCapability::replace_for_trove(
            &conn,
            trove_id,
            &[bind_service_capability("/usr/bin/server")],
        )
        .expect("insert capability");

        Trove::delete(&conn, trove_id).expect("delete trove");

        let rows = InstalledFileCapability::find_by_trove(&conn, trove_id).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn installed_file_capability_rejects_unknown_trove() {
        let (_tmp, conn) = create_test_db();

        let error = InstalledFileCapability::replace_for_trove(
            &conn,
            42,
            &[bind_service_capability("/usr/bin/server")],
        )
        .expect_err("unknown trove foreign key must fail");

        assert!(error.to_string().contains("FOREIGN KEY"));
    }
}
