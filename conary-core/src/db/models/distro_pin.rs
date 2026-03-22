// conary-core/src/db/models/distro_pin.rs

//! Distro pin, package override, and system affinity models
//!
//! These models track which distribution the system is pinned to,
//! per-package overrides for sourcing from alternate distros, and
//! computed affinity statistics showing the distribution mix.

use crate::error::Result;
use crate::model::parser::SourcePinConfig;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// System-level distro pin with mixing policy
#[derive(Debug, Clone)]
pub struct DistroPin {
    pub id: Option<i64>,
    pub distro: String,
    pub mixing_policy: String,
    pub created_at: String,
}

impl DistroPin {
    /// Set the system distro pin (replaces any existing pin)
    pub fn set(conn: &Connection, distro: &str, mixing_policy: &str) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM distro_pin", [])?;
        tx.execute(
            "INSERT INTO distro_pin (distro, mixing_policy, created_at)
             VALUES (?1, ?2, datetime('now'))",
            params![distro, mixing_policy],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Get the current distro pin (if any)
    pub fn get_current(conn: &Connection) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, distro, mixing_policy, created_at
                 FROM distro_pin LIMIT 1",
                [],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Remove the distro pin
    pub fn remove(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM distro_pin", [])?;
        Ok(())
    }

    /// Update the mixing policy on the existing pin
    pub fn set_mixing_policy(conn: &Connection, policy: &str) -> Result<()> {
        conn.execute("UPDATE distro_pin SET mixing_policy = ?1", [policy])?;
        Ok(())
    }

    /// Set the compatibility table from a richer source-pin shape.
    pub fn set_from_source_pin(conn: &Connection, pin: &SourcePinConfig) -> Result<()> {
        let strength = pin.strength.as_deref().unwrap_or("guarded");
        Self::set(conn, &pin.distro, strength)
    }

    /// Convert the compatibility row into the richer source-pin shape.
    pub fn as_source_pin(&self) -> SourcePinConfig {
        SourcePinConfig {
            distro: self.distro.clone(),
            strength: Some(self.mixing_policy.clone()),
        }
    }

    /// Map a database row to a `DistroPin`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            distro: row.get(1)?,
            mixing_policy: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}

/// Per-package override to source from a specific distro
#[derive(Debug, Clone)]
pub struct PackageOverride {
    pub id: Option<i64>,
    pub canonical_id: i64,
    pub from_distro: String,
    pub reason: Option<String>,
}

impl PackageOverride {
    /// Set (or replace) an override for a canonical package
    pub fn set(
        conn: &Connection,
        canonical_id: i64,
        from_distro: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "DELETE FROM package_overrides WHERE canonical_id = ?1",
            [canonical_id],
        )?;
        conn.execute(
            "INSERT INTO package_overrides (canonical_id, from_distro, reason)
             VALUES (?1, ?2, ?3)",
            params![canonical_id, from_distro, reason],
        )?;
        Ok(())
    }

    /// Get the override for a canonical package
    pub fn get(conn: &Connection, canonical_id: i64) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, canonical_id, from_distro, reason
                 FROM package_overrides WHERE canonical_id = ?1",
                [canonical_id],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Remove the override for a canonical package
    pub fn remove(conn: &Connection, canonical_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM package_overrides WHERE canonical_id = ?1",
            [canonical_id],
        )?;
        Ok(())
    }

    /// List all overrides
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, canonical_id, from_distro, reason
             FROM package_overrides ORDER BY canonical_id",
        )?;
        let rows = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Map a database row to a `PackageOverride`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            canonical_id: row.get(1)?,
            from_distro: row.get(2)?,
            reason: row.get(3)?,
        })
    }
}

/// Computed distro affinity statistics
#[derive(Debug, Clone)]
pub struct SystemAffinity {
    pub distro: String,
    pub package_count: i64,
    pub percentage: f64,
}

impl SystemAffinity {
    /// Recompute affinity from installed troves joined with repository metadata
    ///
    /// Deletes all existing rows and reinserts from a join of troves,
    /// repository_packages, and repositories where distro is not null.
    pub fn recompute(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM system_affinity", [])?;
        conn.execute(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
             SELECT
                 rp.distro,
                 COUNT(*) AS package_count,
                 CAST(COUNT(*) AS REAL) * 100.0 / MAX(1, (SELECT COUNT(*) FROM troves)) AS percentage,
                 datetime('now')
             FROM troves t
             JOIN repository_packages rp ON t.name = rp.name
             WHERE rp.distro IS NOT NULL
             GROUP BY rp.distro",
            [],
        )?;
        Ok(())
    }

    /// List all affinity entries, ordered by percentage descending
    pub fn list(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT distro, package_count, percentage
             FROM system_affinity ORDER BY percentage DESC",
        )?;
        let rows = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get affinity for a specific distro
    pub fn get_for_distro(conn: &Connection, distro: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT distro, package_count, percentage
                 FROM system_affinity WHERE distro = ?1",
                [distro],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Map a database row to a `SystemAffinity`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            distro: row.get(0)?,
            package_count: row.get(1)?,
            percentage: row.get(2)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn test_set_and_get_pin() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora", "guarded").unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "fedora");
        assert_eq!(pin.mixing_policy, "guarded");
        assert!(!pin.created_at.is_empty());
    }

    #[test]
    fn test_set_replaces_existing_pin() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora", "guarded").unwrap();
        DistroPin::set(&conn, "debian", "strict").unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "debian");
        assert_eq!(pin.mixing_policy, "strict");

        // Only one row exists
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM distro_pin", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_remove_pin() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora", "guarded").unwrap();
        DistroPin::remove(&conn).unwrap();

        let pin = DistroPin::get_current(&conn).unwrap();
        assert!(pin.is_none());
    }

    #[test]
    fn test_update_mixing_policy() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora", "guarded").unwrap();
        DistroPin::set_mixing_policy(&conn, "permissive").unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "fedora");
        assert_eq!(pin.mixing_policy, "permissive");
    }

    #[test]
    fn test_set_from_source_pin_uses_default_strength() {
        let (_temp, conn) = create_test_db();
        let pin = SourcePinConfig {
            distro: "arch".to_string(),
            strength: None,
        };

        DistroPin::set_from_source_pin(&conn, &pin).unwrap();

        let stored = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(stored.distro, "arch");
        assert_eq!(stored.mixing_policy, "guarded");
    }

    #[test]
    fn test_as_source_pin_preserves_strength() {
        let (_temp, conn) = create_test_db();
        DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();

        let stored = DistroPin::get_current(&conn).unwrap().unwrap();
        let pin = stored.as_source_pin();

        assert_eq!(pin.distro, "ubuntu-noble");
        assert_eq!(pin.strength.as_deref(), Some("strict"));
    }

    #[test]
    fn test_package_override() {
        let (_temp, conn) = create_test_db();

        // Insert a canonical package first (FK constraint)
        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('firefox', 'package')",
            [],
        )
        .unwrap();
        let can_id: i64 = conn
            .query_row(
                "SELECT id FROM canonical_packages WHERE name = 'firefox'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Set override
        PackageOverride::set(&conn, can_id, "debian", Some("newer ESR")).unwrap();

        let ovr = PackageOverride::get(&conn, can_id).unwrap().unwrap();
        assert_eq!(ovr.from_distro, "debian");
        assert_eq!(ovr.reason, Some("newer ESR".to_string()));

        // Replace override
        PackageOverride::set(&conn, can_id, "arch", None).unwrap();
        let ovr = PackageOverride::get(&conn, can_id).unwrap().unwrap();
        assert_eq!(ovr.from_distro, "arch");
        assert!(ovr.reason.is_none());

        // List
        let all = PackageOverride::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);

        // Remove
        PackageOverride::remove(&conn, can_id).unwrap();
        let gone = PackageOverride::get(&conn, can_id).unwrap();
        assert!(gone.is_none());
    }

    #[test]
    fn test_system_affinity_recompute() {
        let (_temp, conn) = create_test_db();

        // On an empty DB, recompute should succeed and produce no rows
        SystemAffinity::recompute(&conn).unwrap();
        let list = SystemAffinity::list(&conn).unwrap();
        assert!(list.is_empty());

        let fedora = SystemAffinity::get_for_distro(&conn, "fedora").unwrap();
        assert!(fedora.is_none());
    }
}
