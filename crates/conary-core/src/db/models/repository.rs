// conary-core/src/db/models/repository.rs

//! Repository package-row persistence and repository source re-exports.

mod source;

pub use source::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
    RepositoryOwnership, RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
    SecurityAdvisorySupport,
};

use crate::error::Result;
use crate::repository::dependency_model::DebianMultiArch;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::io;

/// RepositoryPackage represents a package available from a repository
#[derive(Debug, Clone)]
pub struct RepositoryPackage {
    pub id: Option<i64>,
    pub repository_id: i64,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    /// Exact Debian `Multi-Arch` behavior; absent for non-Debian packages.
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: i64,
    pub download_url: String,
    pub metadata: Option<String>,
    pub synced_at: Option<String>,
    /// Whether this update is a security update
    pub is_security_update: bool,
    /// Severity level: critical, important, moderate, low
    pub severity: Option<String>,
    /// Comma-separated list of CVE IDs (e.g., "CVE-2024-1234,CVE-2024-5678")
    pub cve_ids: Option<String>,
    /// Advisory ID (e.g., "RHSA-2024:1234", "DSA-5678-1")
    pub advisory_id: Option<String>,
    /// URL to the advisory
    pub advisory_url: Option<String>,
    /// Exact public source-profile identity this package came from.
    pub source_profile: Option<String>,
    /// Exact native version grammar and comparison authority.
    pub version_scheme: VersionScheme,
    /// Cross-distro canonical identity for this package.
    pub canonical_id: Option<i64>,
}

impl RepositoryPackage {
    /// Column list for SELECT queries.
    pub const COLUMNS: &'static str = "id, repository_id, name, version, package_release, architecture, debian_multi_arch, description, \
         checksum, size, download_url, metadata, synced_at, \
         is_security_update, severity, cve_ids, advisory_id, advisory_url, \
         source_profile, version_scheme, canonical_id";

    /// Column list for SELECT queries with table alias prefix (rp.).
    pub const COLUMNS_PREFIXED: &'static str = "rp.id, rp.repository_id, rp.name, rp.version, \
         rp.package_release, rp.architecture, rp.debian_multi_arch, rp.description, rp.checksum, rp.size, rp.download_url, \
         rp.metadata, rp.synced_at, rp.is_security_update, \
         rp.severity, rp.cve_ids, rp.advisory_id, rp.advisory_url, rp.source_profile, \
         rp.version_scheme, rp.canonical_id";

    /// INSERT SQL shared by `batch_insert` and `batch_insert_with_ids`.
    const BATCH_INSERT_SQL: &'static str = "\
         INSERT INTO repository_packages \
         (repository_id, name, version, package_release, architecture, debian_multi_arch, description, checksum, size, \
          download_url, metadata, is_security_update, severity, cve_ids, \
          advisory_id, advisory_url, source_profile, version_scheme, canonical_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)";

    /// Create a new RepositoryPackage
    pub fn new(
        repository_id: i64,
        name: String,
        version: String,
        version_scheme: VersionScheme,
        checksum: String,
        size: i64,
        download_url: String,
    ) -> Self {
        Self {
            id: None,
            repository_id,
            name,
            version,
            package_release: String::new(),
            architecture: None,
            debian_multi_arch: (version_scheme == VersionScheme::Debian)
                .then_some(DebianMultiArch::No),
            description: None,
            checksum,
            size,
            download_url,
            metadata: None,
            synced_at: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            source_profile: None,
            version_scheme,
            canonical_id: None,
        }
    }

    /// Insert this repository package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, package_release, architecture, debian_multi_arch, description, checksum, size, download_url, metadata,
              is_security_update, severity, cve_ids, advisory_id, advisory_url, source_profile, version_scheme, canonical_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                &self.repository_id,
                &self.name,
                &self.version,
                &self.package_release,
                &self.architecture,
                self.debian_multi_arch.map(DebianMultiArch::as_str),
                &self.description,
                &self.checksum,
                &self.size,
                &self.download_url,
                &self.metadata,
                self.is_security_update as i32,
                &self.severity,
                &self.cve_ids,
                &self.advisory_id,
                &self.advisory_url,
                &self.source_profile,
                self.version_scheme.as_str(),
                &self.canonical_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a repository package by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages WHERE id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let pkg = stmt.query_row([id], Self::from_row).optional()?;
        Ok(pkg)
    }

    /// Find repository packages by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages WHERE name = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Find repository packages by repository ID
    pub fn find_by_repository(conn: &Connection, repository_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages WHERE repository_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([repository_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Search repository packages by pattern (name or description)
    ///
    /// The pattern is escaped for SQL LIKE to prevent `%` and `_` in user
    /// input from acting as wildcards. Uses `\` as the LIKE escape character.
    pub fn search(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        // Escape SQL LIKE wildcards in user input
        let escaped = pattern
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let search_pattern = format!("%{escaped}%");
        let sql = format!(
            "SELECT {} FROM repository_packages \
             WHERE name LIKE ?1 ESCAPE '\\' OR description LIKE ?1 ESCAPE '\\' \
             ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([&search_pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Delete all packages for a repository (used when syncing)
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_packages WHERE repository_id = ?1",
            [repository_id],
        )?;
        Ok(())
    }

    /// Delete a specific package by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM repository_packages WHERE id = ?1", [id])?;
        Ok(())
    }

    /// List all packages in all enabled repositories
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 \
             ORDER BY rp.name, rp.version",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Find packages in enabled repositories whose metadata JSON contains `dependency_name`
    /// as a literal substring.  This is a coarse pre-filter; callers must re-check the
    /// parsed provides list for an exact match.
    pub fn find_in_enabled_repos_with_metadata_like(
        conn: &Connection,
        dependency_name: &str,
    ) -> Result<Vec<Self>> {
        let pattern = format!("%{dependency_name}%");
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 AND rp.metadata LIKE ?1",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([&pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Find a specific package by name and version in enabled repositories
    pub fn find_by_name_version(
        conn: &Connection,
        name: &str,
        version: &str,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 AND rp.name = ?1 AND rp.version = ?2",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let pkg = stmt.query_row([name, version], Self::from_row).optional()?;
        Ok(pkg)
    }

    /// Get repository name for this package
    pub fn get_repository_name(&self, conn: &Connection) -> Result<String> {
        if let Some(repo) = Repository::find_by_id(conn, self.repository_id)? {
            Ok(repo.name)
        } else {
            Ok("unknown".to_string())
        }
    }

    /// Format size as human-readable string
    pub fn size_human(&self) -> String {
        super::format_size(self.size)
    }

    /// Convert a database row to a RepositoryPackage
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            package_release: row.get(4)?,
            architecture: row.get(5)?,
            debian_multi_arch: debian_multi_arch_from_row(row, 6)?,
            description: row.get(7)?,
            checksum: row.get(8)?,
            size: row.get(9)?,
            download_url: row.get(10)?,
            metadata: row.get(11)?,
            synced_at: row.get(12)?,
            is_security_update: row.get::<_, i32>(13)? != 0,
            severity: row.get(14)?,
            cve_ids: row.get(15)?,
            advisory_id: row.get(16)?,
            advisory_url: row.get(17)?,
            source_profile: row.get(18)?,
            version_scheme: version_scheme_from_row(row, 19)?,
            canonical_id: row.get(20)?,
        })
    }

    /// Find all security updates available
    pub fn find_security_updates(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 AND rp.is_security_update = 1 \
             ORDER BY CASE rp.severity \
                 WHEN 'critical' THEN 1 \
                 WHEN 'important' THEN 2 \
                 WHEN 'moderate' THEN 3 \
                 WHEN 'low' THEN 4 \
                 ELSE 5 \
             END, rp.name",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Batch insert multiple repository packages efficiently
    ///
    /// Uses a prepared statement within a transaction for much better performance
    /// than individual inserts. For 77k packages, this reduces sync time from
    /// ~5 minutes to ~10 seconds.
    ///
    /// Caller must wrap this in a transaction for atomicity.
    pub fn batch_insert(conn: &Connection, packages: &[Self]) -> Result<usize> {
        if packages.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(Self::BATCH_INSERT_SQL)?;

        for pkg in packages {
            Self::execute_batch_row(&mut stmt, pkg)?;
        }

        Ok(packages.len())
    }

    /// Batch insert repository packages and populate their generated IDs.
    pub fn batch_insert_with_ids(conn: &Connection, packages: &mut [Self]) -> Result<usize> {
        if packages.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(Self::BATCH_INSERT_SQL)?;

        for pkg in packages.iter_mut() {
            Self::execute_batch_row(&mut stmt, pkg)?;
            pkg.id = Some(conn.last_insert_rowid());
        }

        Ok(packages.len())
    }

    /// Execute a single batch INSERT row with the shared parameter list.
    fn execute_batch_row(stmt: &mut rusqlite::CachedStatement<'_>, pkg: &Self) -> Result<()> {
        stmt.execute(params![
            &pkg.repository_id,
            &pkg.name,
            &pkg.version,
            &pkg.package_release,
            &pkg.architecture,
            pkg.debian_multi_arch.map(DebianMultiArch::as_str),
            &pkg.description,
            &pkg.checksum,
            &pkg.size,
            &pkg.download_url,
            &pkg.metadata,
            pkg.is_security_update as i32,
            &pkg.severity,
            &pkg.cve_ids,
            &pkg.advisory_id,
            &pkg.advisory_url,
            &pkg.source_profile,
            pkg.version_scheme.as_str(),
            &pkg.canonical_id,
        ])?;
        Ok(())
    }
}

pub(crate) fn version_scheme_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<VersionScheme> {
    let raw = row.get::<_, String>(index)?;
    raw.parse().map_err(|error: String| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
        )
    })
}

fn debian_multi_arch_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<DebianMultiArch>> {
    let Some(raw) = row.get::<_, Option<String>>(index)? else {
        return Ok(None);
    };
    DebianMultiArch::parse_exact(&raw)
        .map(Some)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
            )
        })
}

#[cfg(test)]
#[path = "repository/tests.rs"]
mod tests;
