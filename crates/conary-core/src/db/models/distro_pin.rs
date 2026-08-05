// conary-core/src/db/models/distro_pin.rs

//! Distro pin and system affinity models
//!
//! These models track which distribution the system is pinned to,
//! computed affinity statistics showing the distribution mix.

use crate::error::Result;
use crate::model::parser::SourcePinConfig;
use crate::repository::resolution_policy::DependencyMixingPolicy;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// System-level distro pin with mixing policy
#[derive(Debug, Clone)]
pub struct DistroPin {
    pub id: Option<i64>,
    pub distro: String,
    pub mixing_policy: DependencyMixingPolicy,
    pub created_at: String,
}

impl DistroPin {
    /// Set the system distro pin (replaces any existing pin)
    pub fn set(
        conn: &Connection,
        distro: &str,
        mixing_policy: DependencyMixingPolicy,
    ) -> Result<()> {
        if crate::repository::supported_profiles::profile_by_public_id(distro).is_none() {
            return Err(crate::Error::ConfigError(format!(
                "unsupported public distro profile '{distro}'"
            )));
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM distro_pin", [])?;
        tx.execute(
            "INSERT INTO distro_pin (distro, mixing_policy, created_at)
             VALUES (?1, ?2, datetime('now'))",
            params![distro, mixing_policy.as_str()],
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
    pub fn set_mixing_policy(conn: &Connection, policy: DependencyMixingPolicy) -> Result<()> {
        conn.execute(
            "UPDATE distro_pin SET mixing_policy = ?1",
            [policy.as_str()],
        )?;
        Ok(())
    }

    /// Persist a source pin.
    pub fn set_from_source_pin(conn: &Connection, pin: &SourcePinConfig) -> Result<()> {
        Self::set(conn, &pin.distro, pin.strength)
    }

    /// Read the persisted pin as the model source-pin shape.
    pub fn as_source_pin(&self) -> SourcePinConfig {
        SourcePinConfig {
            distro: self.distro.clone(),
            strength: self.mixing_policy,
        }
    }

    /// Map a database row to a `DistroPin`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            distro: row.get(1)?,
            mixing_policy: row.get::<_, String>(2)?.parse().map_err(|error: String| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error).into(),
                )
            })?,
            created_at: row.get(3)?,
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
    /// Recompute affinity from exact installed-package provenance.
    ///
    /// Deletes all existing rows and reinserts from a join of troves,
    /// repository provenance where distro is not null.
    pub fn recompute(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM system_affinity", [])?;
        // Each installed trove counts toward exactly one distro:
        //   1. Use troves.source_profile if populated (adopted/taken packages).
        //   2. Use installed_from_repository_id -> repositories.source_profile
        //      (set at install time with deterministic repo selection).
        conn.execute(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
             SELECT
                 distro,
                 COUNT(*) AS package_count,
                 CAST(COUNT(*) AS REAL) * 100.0
                     / MAX(1, (SELECT COUNT(*) FROM troves)) AS percentage,
                 datetime('now')
             FROM (
                 -- 1. Prefer source_profile (adopted/taken packages)
                 SELECT source_profile AS distro FROM troves
                 WHERE source_profile IS NOT NULL
                 UNION ALL
                 -- 2. Use stored install provenance
                 SELECT r.source_profile AS distro
                 FROM troves t
                 JOIN repositories r ON t.installed_from_repository_id = r.id
                 WHERE t.source_profile IS NULL
                   AND t.installed_from_repository_id IS NOT NULL
                   AND r.source_profile IS NOT NULL
             )
             WHERE distro IS NOT NULL
             GROUP BY distro",
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
    use crate::db::models::{Trove, TroveType};
    use crate::db::testing::create_test_db;

    #[test]
    fn test_set_and_get_pin() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Guarded).unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "fedora-44");
        assert_eq!(pin.mixing_policy, DependencyMixingPolicy::Guarded);
        assert!(!pin.created_at.is_empty());
    }

    #[test]
    fn set_rejects_non_catalog_profile_identity() {
        let (_temp, conn) = create_test_db();

        let error = DistroPin::set(&conn, "fedora", DependencyMixingPolicy::Guarded).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("unsupported public distro profile")
        );
        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[test]
    fn schema_rejects_untyped_mixing_policy_values() {
        let (_temp, conn) = create_test_db();

        let error = conn
            .execute(
                "INSERT INTO distro_pin (distro, mixing_policy, created_at)
                 VALUES ('arch', 'hard', datetime('now'))",
                [],
            )
            .unwrap_err();

        assert!(error.to_string().contains("CHECK constraint failed"));
    }

    #[test]
    fn schema_rejects_omitted_mixing_policy() {
        let (_temp, conn) = create_test_db();

        let error = conn
            .execute(
                "INSERT INTO distro_pin (distro, created_at)
                 VALUES ('arch', datetime('now'))",
                [],
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("NOT NULL constraint failed: distro_pin.mixing_policy"),
            "{error}"
        );
    }

    #[test]
    fn schema_rejects_non_catalog_profile_identity() {
        let (_temp, conn) = create_test_db();

        let error = conn
            .execute(
                "INSERT INTO distro_pin (distro, mixing_policy, created_at)
                 VALUES ('fedora', 'guarded', datetime('now'))",
                [],
            )
            .unwrap_err();

        assert!(error.to_string().contains("CHECK constraint failed"));
    }

    #[test]
    fn test_set_replaces_existing_pin() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Guarded).unwrap();
        DistroPin::set(&conn, "ubuntu-26.04", DependencyMixingPolicy::Strict).unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "ubuntu-26.04");
        assert_eq!(pin.mixing_policy, DependencyMixingPolicy::Strict);

        // Only one row exists
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM distro_pin", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_remove_pin() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Guarded).unwrap();
        DistroPin::remove(&conn).unwrap();

        let pin = DistroPin::get_current(&conn).unwrap();
        assert!(pin.is_none());
    }

    #[test]
    fn test_update_mixing_policy() {
        let (_temp, conn) = create_test_db();

        DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Guarded).unwrap();
        DistroPin::set_mixing_policy(&conn, DependencyMixingPolicy::Permissive).unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "fedora-44");
        assert_eq!(pin.mixing_policy, DependencyMixingPolicy::Permissive);
    }

    #[test]
    fn test_set_from_source_pin_preserves_explicit_strength() {
        let (_temp, conn) = create_test_db();
        let pin = SourcePinConfig {
            distro: "arch".to_string(),
            strength: DependencyMixingPolicy::Guarded,
        };

        DistroPin::set_from_source_pin(&conn, &pin).unwrap();

        let stored = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(stored.distro, "arch");
        assert_eq!(stored.mixing_policy, DependencyMixingPolicy::Guarded);
    }

    #[test]
    fn test_as_source_pin_preserves_strength() {
        let (_temp, conn) = create_test_db();
        DistroPin::set(&conn, "ubuntu-26.04", DependencyMixingPolicy::Strict).unwrap();

        let stored = DistroPin::get_current(&conn).unwrap().unwrap();
        let pin = stored.as_source_pin();

        assert_eq!(pin.distro, "ubuntu-26.04");
        assert_eq!(pin.strength, DependencyMixingPolicy::Strict);
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

    #[test]
    fn system_affinity_never_infers_missing_install_provenance_from_catalog_matches() {
        let (_temp, conn) = create_test_db();
        let mut trove = Trove::new(
            "catalog-match".to_string(),
            "1.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.insert(&conn).unwrap();

        conn.execute(
            "INSERT INTO repositories (name, url, source_profile)
             VALUES ('fedora', 'https://example.invalid', 'fedora-44')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_packages (
                repository_id, name, version, checksum, size, download_url,
                version_scheme
             ) VALUES (1, 'catalog-match', '1.0', 'sha256:fixture', 1,
                       'https://example.invalid/catalog-match.rpm', 'rpm')",
            [],
        )
        .unwrap();

        SystemAffinity::recompute(&conn).unwrap();
        assert!(SystemAffinity::list(&conn).unwrap().is_empty());
    }
}
