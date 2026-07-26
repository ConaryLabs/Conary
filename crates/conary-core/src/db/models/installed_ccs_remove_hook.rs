// conary-core/src/db/models/installed_ccs_remove_hook.rs

//! Exact persisted authority for an installed CCS package's pre-remove hook.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// The CCS-authored hook that must run before its owning trove is removed.
///
/// CCS defines this hook as a POSIX shell script. Native RPM, Debian, and Arch
/// lifecycle entries are persisted separately in
/// `installed_native_lifecycle_bundles` and must never be projected into this
/// model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCcsRemoveHook {
    pub trove_id: i64,
    pub script: String,
    pub reversible: Option<bool>,
}

impl InstalledCcsRemoveHook {
    const COLUMNS: &'static str = "trove_id, script, reversible";

    pub fn new(trove_id: i64, script: String, reversible: Option<bool>) -> Self {
        Self {
            trove_id,
            script,
            reversible,
        }
    }

    /// Persist the one CCS remove hook owned by this trove.
    pub fn insert_or_replace(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO installed_ccs_remove_hooks (trove_id, script, reversible)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(trove_id) DO UPDATE SET
                 script = excluded.script,
                 reversible = excluded.reversible",
            params![self.trove_id, self.script, self.reversible],
        )?;
        Ok(())
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM installed_ccs_remove_hooks WHERE trove_id = ?1",
            Self::COLUMNS
        );
        Ok(conn
            .query_row(&sql, [trove_id], Self::from_row)
            .optional()?)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            trove_id: row.get(0)?,
            script: row.get(1)?,
            reversible: row.get(2)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::InstalledCcsRemoveHook;
    use crate::db::models::{Trove, TroveType};
    use crate::db::testing::create_test_db;

    #[test]
    fn round_trips_exact_ccs_remove_hook_contract() {
        let (_temp, conn) = create_test_db();
        let trove_id = Trove::new(
            "fixture".to_string(),
            "1".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        )
        .insert(&conn)
        .unwrap();
        let hook = InstalledCcsRemoveHook::new(trove_id, "echo removing\n".to_string(), Some(true));

        hook.insert_or_replace(&conn).unwrap();

        assert_eq!(
            InstalledCcsRemoveHook::find_by_trove(&conn, trove_id).unwrap(),
            Some(hook)
        );
    }
}
