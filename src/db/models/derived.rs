// src/db/models/derived.rs

//! Derived package models for enterprise customization
//!
//! Derived packages allow creating custom versions of existing packages without
//! rebuilding from source. This enables enterprise customization such as:
//! - Custom configuration files (e.g., corporate nginx.conf)
//! - Security patches applied before upstream releases
//! - Monitoring/logging instrumentation
//! - Branding modifications

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Version policy for derived packages
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionPolicy {
    /// Inherit version from parent (e.g., parent 1.0.0 -> derived 1.0.0)
    Inherit,
    /// Add suffix to parent version (e.g., parent 1.0.0 + "+custom" -> 1.0.0+custom)
    Suffix(String),
    /// Use a specific version regardless of parent
    Specific(String),
}

impl VersionPolicy {
    /// Parse version policy from database strings
    pub fn from_db(policy: &str, suffix: Option<&str>, specific: Option<&str>) -> Self {
        match policy {
            "suffix" => VersionPolicy::Suffix(suffix.unwrap_or("+derived").to_string()),
            "specific" => VersionPolicy::Specific(specific.unwrap_or("1.0.0").to_string()),
            _ => VersionPolicy::Inherit,
        }
    }

    /// Get the policy type string for database storage
    pub fn policy_type(&self) -> &'static str {
        match self {
            VersionPolicy::Inherit => "inherit",
            VersionPolicy::Suffix(_) => "suffix",
            VersionPolicy::Specific(_) => "specific",
        }
    }

    /// Compute the derived version from the parent version
    pub fn compute_version(&self, parent_version: &str) -> String {
        match self {
            VersionPolicy::Inherit => parent_version.to_string(),
            VersionPolicy::Suffix(suffix) => format!("{parent_version}{suffix}"),
            VersionPolicy::Specific(version) => version.clone(),
        }
    }
}

/// Build status of a derived package
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedStatus {
    /// Not yet built
    Pending,
    /// Successfully built
    Built,
    /// Parent package was updated, rebuild needed
    Stale,
    /// Build failed
    Error,
}

impl DerivedStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DerivedStatus::Pending => "pending",
            DerivedStatus::Built => "built",
            DerivedStatus::Stale => "stale",
            DerivedStatus::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "built" => DerivedStatus::Built,
            "stale" => DerivedStatus::Stale,
            "error" => DerivedStatus::Error,
            _ => DerivedStatus::Pending,
        }
    }
}

/// Database representation of a derived package
#[derive(Debug, Clone)]
pub struct DerivedPackage {
    pub id: Option<i64>,
    /// Name of the derived package (must be unique)
    pub name: String,
    /// Parent trove ID (if parent is installed)
    pub parent_trove_id: Option<i64>,
    /// Parent package name
    pub parent_name: String,
    /// Parent version constraint (None = track latest)
    pub parent_version: Option<String>,
    /// Version policy
    pub version_policy: VersionPolicy,
    /// Description
    pub description: Option<String>,
    /// Build status
    pub status: DerivedStatus,
    /// Built trove ID (when status = Built)
    pub built_trove_id: Option<i64>,
    /// Model file this came from (None if created via CLI)
    pub model_source: Option<String>,
    /// Error message if status = Error
    pub error_message: Option<String>,
    /// Creation timestamp
    pub created_at: Option<String>,
    /// Last update timestamp
    pub updated_at: Option<String>,
}

impl DerivedPackage {
    /// Create a new derived package definition
    pub fn new(name: String, parent_name: String) -> Self {
        Self {
            id: None,
            name,
            parent_trove_id: None,
            parent_name,
            parent_version: None,
            version_policy: VersionPolicy::Inherit,
            description: None,
            status: DerivedStatus::Pending,
            built_trove_id: None,
            model_source: None,
            error_message: None,
            created_at: None,
            updated_at: None,
        }
    }

    /// Insert this derived package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        let (suffix, specific) = match &self.version_policy {
            VersionPolicy::Suffix(s) => (Some(s.as_str()), None),
            VersionPolicy::Specific(v) => (None, Some(v.as_str())),
            VersionPolicy::Inherit => (None, None),
        };

        conn.execute(
            "INSERT INTO derived_packages (
                name, parent_trove_id, parent_name, parent_version,
                version_policy, version_suffix, specific_version,
                description, status, built_trove_id, model_source, error_message
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                &self.name,
                &self.parent_trove_id,
                &self.parent_name,
                &self.parent_version,
                self.version_policy.policy_type(),
                suffix,
                specific,
                &self.description,
                self.status.as_str(),
                &self.built_trove_id,
                &self.model_source,
                &self.error_message,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Update the derived package
    pub fn update(&self, conn: &Connection) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update derived package without ID".to_string())
        })?;

        let (suffix, specific) = match &self.version_policy {
            VersionPolicy::Suffix(s) => (Some(s.as_str()), None),
            VersionPolicy::Specific(v) => (None, Some(v.as_str())),
            VersionPolicy::Inherit => (None, None),
        };

        conn.execute(
            "UPDATE derived_packages SET
                parent_trove_id = ?1, parent_name = ?2, parent_version = ?3,
                version_policy = ?4, version_suffix = ?5, specific_version = ?6,
                description = ?7, status = ?8, built_trove_id = ?9,
                model_source = ?10, error_message = ?11, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?12",
            params![
                &self.parent_trove_id,
                &self.parent_name,
                &self.parent_version,
                self.version_policy.policy_type(),
                suffix,
                specific,
                &self.description,
                self.status.as_str(),
                &self.built_trove_id,
                &self.model_source,
                &self.error_message,
                id,
            ],
        )?;

        Ok(())
    }

    /// Find a derived package by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_trove_id, parent_name, parent_version,
                    version_policy, version_suffix, specific_version,
                    description, status, built_trove_id, model_source,
                    error_message, created_at, updated_at
             FROM derived_packages WHERE id = ?1",
        )?;

        let pkg = stmt.query_row([id], Self::from_row).optional()?;
        Ok(pkg)
    }

    /// Find a derived package by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_trove_id, parent_name, parent_version,
                    version_policy, version_suffix, specific_version,
                    description, status, built_trove_id, model_source,
                    error_message, created_at, updated_at
             FROM derived_packages WHERE name = ?1",
        )?;

        let pkg = stmt.query_row([name], Self::from_row).optional()?;
        Ok(pkg)
    }

    /// Find all derived packages from a parent
    pub fn find_by_parent(conn: &Connection, parent_name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_trove_id, parent_name, parent_version,
                    version_policy, version_suffix, specific_version,
                    description, status, built_trove_id, model_source,
                    error_message, created_at, updated_at
             FROM derived_packages WHERE parent_name = ?1 ORDER BY name",
        )?;

        let packages = stmt
            .query_map([parent_name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// Find all derived packages by status
    pub fn find_by_status(conn: &Connection, status: DerivedStatus) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_trove_id, parent_name, parent_version,
                    version_policy, version_suffix, specific_version,
                    description, status, built_trove_id, model_source,
                    error_message, created_at, updated_at
             FROM derived_packages WHERE status = ?1 ORDER BY name",
        )?;

        let packages = stmt
            .query_map([status.as_str()], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// List all derived packages
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_trove_id, parent_name, parent_version,
                    version_policy, version_suffix, specific_version,
                    description, status, built_trove_id, model_source,
                    error_message, created_at, updated_at
             FROM derived_packages ORDER BY name",
        )?;

        let packages = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// Update status
    pub fn set_status(&mut self, conn: &Connection, status: DerivedStatus) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update status without ID".to_string())
        })?;

        conn.execute(
            "UPDATE derived_packages SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![status.as_str(), id],
        )?;

        self.status = status;
        Ok(())
    }

    /// Mark as built with the built trove ID
    pub fn mark_built(&mut self, conn: &Connection, built_trove_id: i64) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot mark built without ID".to_string())
        })?;

        conn.execute(
            "UPDATE derived_packages SET status = 'built', built_trove_id = ?1,
             error_message = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![built_trove_id, id],
        )?;

        self.status = DerivedStatus::Built;
        self.built_trove_id = Some(built_trove_id);
        self.error_message = None;
        Ok(())
    }

    /// Mark as error with message
    pub fn mark_error(&mut self, conn: &Connection, message: &str) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot mark error without ID".to_string())
        })?;

        conn.execute(
            "UPDATE derived_packages SET status = 'error', error_message = ?1,
             updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![message, id],
        )?;

        self.status = DerivedStatus::Error;
        self.error_message = Some(message.to_string());
        Ok(())
    }

    /// Mark as stale (parent was updated)
    pub fn mark_stale(conn: &Connection, parent_name: &str) -> Result<usize> {
        let count = conn.execute(
            "UPDATE derived_packages SET status = 'stale', updated_at = CURRENT_TIMESTAMP
             WHERE parent_name = ?1 AND status = 'built'",
            [parent_name],
        )?;

        Ok(count)
    }

    /// Delete a derived package
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM derived_packages WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Get patches for this derived package
    pub fn patches(&self, conn: &Connection) -> Result<Vec<DerivedPatch>> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot get patches without ID".to_string())
        })?;
        DerivedPatch::find_by_derived(conn, id)
    }

    /// Get file overrides for this derived package
    pub fn overrides(&self, conn: &Connection) -> Result<Vec<DerivedOverride>> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot get overrides without ID".to_string())
        })?;
        DerivedOverride::find_by_derived(conn, id)
    }

    /// Convert a database row to a DerivedPackage
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let policy_str: String = row.get(5)?;
        let suffix: Option<String> = row.get(6)?;
        let specific: Option<String> = row.get(7)?;
        let status_str: String = row.get(9)?;

        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            parent_trove_id: row.get(2)?,
            parent_name: row.get(3)?,
            parent_version: row.get(4)?,
            version_policy: VersionPolicy::from_db(
                &policy_str,
                suffix.as_deref(),
                specific.as_deref(),
            ),
            description: row.get(8)?,
            status: DerivedStatus::parse(&status_str),
            built_trove_id: row.get(10)?,
            model_source: row.get(11)?,
            error_message: row.get(12)?,
            created_at: row.get(13)?,
            updated_at: row.get(14)?,
        })
    }
}

/// A patch to apply to a derived package
#[derive(Debug, Clone)]
pub struct DerivedPatch {
    pub id: Option<i64>,
    pub derived_id: i64,
    /// Order of patch application (1, 2, 3...)
    pub patch_order: i32,
    /// Human-readable patch name
    pub patch_name: String,
    /// Patch content hash (stored in CAS)
    pub patch_hash: String,
    /// Strip level for patch application (default -p1)
    pub strip_level: i32,
}

impl DerivedPatch {
    /// Create a new patch entry
    pub fn new(derived_id: i64, patch_order: i32, patch_name: String, patch_hash: String) -> Self {
        Self {
            id: None,
            derived_id,
            patch_order,
            patch_name,
            patch_hash,
            strip_level: 1,
        }
    }

    /// Insert this patch into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO derived_patches (derived_id, patch_order, patch_name, patch_hash, strip_level)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.derived_id,
                &self.patch_order,
                &self.patch_name,
                &self.patch_hash,
                &self.strip_level,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all patches for a derived package
    pub fn find_by_derived(conn: &Connection, derived_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, derived_id, patch_order, patch_name, patch_hash, strip_level
             FROM derived_patches WHERE derived_id = ?1 ORDER BY patch_order",
        )?;

        let patches = stmt
            .query_map([derived_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(patches)
    }

    /// Delete all patches for a derived package
    pub fn delete_all(conn: &Connection, derived_id: i64) -> Result<()> {
        conn.execute("DELETE FROM derived_patches WHERE derived_id = ?1", [derived_id])?;
        Ok(())
    }

    /// Convert a database row to a DerivedPatch
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            derived_id: row.get(1)?,
            patch_order: row.get(2)?,
            patch_name: row.get(3)?,
            patch_hash: row.get(4)?,
            strip_level: row.get(5)?,
        })
    }
}

/// A file override for a derived package
#[derive(Debug, Clone)]
pub struct DerivedOverride {
    pub id: Option<i64>,
    pub derived_id: i64,
    /// Target path in the package to override
    pub target_path: String,
    /// Source content hash (stored in CAS); None means remove the file
    pub source_hash: Option<String>,
    /// Original source path (for reference in model file)
    pub source_path: Option<String>,
    /// Permissions override (None = inherit from parent)
    pub permissions: Option<i32>,
}

impl DerivedOverride {
    /// Create a new file override (replace file)
    pub fn new_replace(derived_id: i64, target_path: String, source_hash: String) -> Self {
        Self {
            id: None,
            derived_id,
            target_path,
            source_hash: Some(source_hash),
            source_path: None,
            permissions: None,
        }
    }

    /// Create a new file override (remove file)
    pub fn new_remove(derived_id: i64, target_path: String) -> Self {
        Self {
            id: None,
            derived_id,
            target_path,
            source_hash: None,
            source_path: None,
            permissions: None,
        }
    }

    /// Insert this override into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO derived_overrides (derived_id, target_path, source_hash, source_path, permissions)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.derived_id,
                &self.target_path,
                &self.source_hash,
                &self.source_path,
                &self.permissions,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all overrides for a derived package
    pub fn find_by_derived(conn: &Connection, derived_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, derived_id, target_path, source_hash, source_path, permissions
             FROM derived_overrides WHERE derived_id = ?1 ORDER BY target_path",
        )?;

        let overrides = stmt
            .query_map([derived_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(overrides)
    }

    /// Find an override by target path
    pub fn find_by_path(conn: &Connection, derived_id: i64, target_path: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, derived_id, target_path, source_hash, source_path, permissions
             FROM derived_overrides WHERE derived_id = ?1 AND target_path = ?2",
        )?;

        let ov = stmt.query_row(params![derived_id, target_path], Self::from_row).optional()?;
        Ok(ov)
    }

    /// Delete all overrides for a derived package
    pub fn delete_all(conn: &Connection, derived_id: i64) -> Result<()> {
        conn.execute("DELETE FROM derived_overrides WHERE derived_id = ?1", [derived_id])?;
        Ok(())
    }

    /// Check if this is a removal override
    pub fn is_removal(&self) -> bool {
        self.source_hash.is_none()
    }

    /// Convert a database row to a DerivedOverride
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            derived_id: row.get(1)?,
            target_path: row.get(2)?,
            source_hash: row.get(3)?,
            source_path: row.get(4)?,
            permissions: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_derived_package_crud() {
        let (_temp, conn) = create_test_db();

        // Create a derived package
        let mut derived = DerivedPackage::new("nginx-custom".to_string(), "nginx".to_string());
        derived.description = Some("Custom nginx with corporate config".to_string());
        derived.version_policy = VersionPolicy::Suffix("+corp".to_string());

        let id = derived.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by ID
        let found = DerivedPackage::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "nginx-custom");
        assert_eq!(found.parent_name, "nginx");
        assert_eq!(found.version_policy, VersionPolicy::Suffix("+corp".to_string()));
        assert_eq!(found.status, DerivedStatus::Pending);

        // Find by name
        let found = DerivedPackage::find_by_name(&conn, "nginx-custom").unwrap().unwrap();
        assert_eq!(found.id, Some(id));

        // Find by parent
        let derived_from_nginx = DerivedPackage::find_by_parent(&conn, "nginx").unwrap();
        assert_eq!(derived_from_nginx.len(), 1);

        // Create a trove for the built package (required for FK constraint)
        use crate::db::models::{Trove, TroveType};
        let mut trove = Trove::new("nginx-custom".to_string(), "1.0.0+corp".to_string(), TroveType::Package);
        let trove_id = trove.insert(&conn).unwrap();

        // Update status
        let mut derived = found;
        derived.mark_built(&conn, trove_id).unwrap();
        assert_eq!(derived.status, DerivedStatus::Built);
        assert_eq!(derived.built_trove_id, Some(trove_id));

        // Mark stale
        DerivedPackage::mark_stale(&conn, "nginx").unwrap();
        let stale = DerivedPackage::find_by_status(&conn, DerivedStatus::Stale).unwrap();
        assert_eq!(stale.len(), 1);
    }

    #[test]
    fn test_version_policy() {
        let inherit = VersionPolicy::Inherit;
        assert_eq!(inherit.compute_version("1.0.0"), "1.0.0");

        let suffix = VersionPolicy::Suffix("+custom".to_string());
        assert_eq!(suffix.compute_version("1.0.0"), "1.0.0+custom");

        let specific = VersionPolicy::Specific("2.0.0".to_string());
        assert_eq!(specific.compute_version("1.0.0"), "2.0.0");
    }

    #[test]
    fn test_derived_patches() {
        let (_temp, conn) = create_test_db();

        // Create a derived package
        let mut derived = DerivedPackage::new("nginx-patched".to_string(), "nginx".to_string());
        let derived_id = derived.insert(&conn).unwrap();

        // Add patches
        let mut patch1 = DerivedPatch::new(
            derived_id,
            1,
            "fix-security.patch".to_string(),
            "abc123".to_string(),
        );
        patch1.insert(&conn).unwrap();

        let mut patch2 = DerivedPatch::new(
            derived_id,
            2,
            "add-monitoring.patch".to_string(),
            "def456".to_string(),
        );
        patch2.strip_level = 0;
        patch2.insert(&conn).unwrap();

        // Get patches (should be ordered)
        let patches = derived.patches(&conn).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].patch_order, 1);
        assert_eq!(patches[0].patch_name, "fix-security.patch");
        assert_eq!(patches[1].patch_order, 2);
    }

    #[test]
    fn test_derived_overrides() {
        let (_temp, conn) = create_test_db();

        // Create a derived package
        let mut derived = DerivedPackage::new("nginx-corp".to_string(), "nginx".to_string());
        let derived_id = derived.insert(&conn).unwrap();

        // Add file override (replace)
        let mut override1 = DerivedOverride::new_replace(
            derived_id,
            "/etc/nginx/nginx.conf".to_string(),
            "hash123".to_string(),
        );
        override1.source_path = Some("files/nginx.conf".to_string());
        override1.insert(&conn).unwrap();

        // Add file override (remove)
        let mut override2 = DerivedOverride::new_remove(
            derived_id,
            "/etc/nginx/default.conf".to_string(),
        );
        override2.insert(&conn).unwrap();

        // Get overrides
        let overrides = derived.overrides(&conn).unwrap();
        assert_eq!(overrides.len(), 2);

        // First is default.conf (alphabetical order)
        assert!(overrides[0].is_removal());
        assert_eq!(overrides[0].target_path, "/etc/nginx/default.conf");

        // Second is nginx.conf
        assert!(!overrides[1].is_removal());
        assert_eq!(overrides[1].target_path, "/etc/nginx/nginx.conf");
        assert_eq!(overrides[1].source_hash, Some("hash123".to_string()));
    }

    #[test]
    fn test_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a derived package with patches and overrides
        let mut derived = DerivedPackage::new("test-derived".to_string(), "test".to_string());
        let derived_id = derived.insert(&conn).unwrap();

        let mut patch = DerivedPatch::new(derived_id, 1, "test.patch".to_string(), "hash".to_string());
        patch.insert(&conn).unwrap();

        let mut override_entry = DerivedOverride::new_replace(derived_id, "/etc/test.conf".to_string(), "hash".to_string());
        override_entry.insert(&conn).unwrap();

        // Delete the derived package
        DerivedPackage::delete(&conn, derived_id).unwrap();

        // Verify patches and overrides are gone
        let patches = DerivedPatch::find_by_derived(&conn, derived_id).unwrap();
        assert!(patches.is_empty());

        let overrides = DerivedOverride::find_by_derived(&conn, derived_id).unwrap();
        assert!(overrides.is_empty());
    }
}
