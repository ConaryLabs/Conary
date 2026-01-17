// src/db/models/redirect.rs

//! Redirect model for package name aliasing and supersession
//!
//! Redirects allow package names to be aliased or superseded by other packages.
//! This enables clean handling of:
//! - Package renames (old-name -> new-name)
//! - Package obsoletes (deprecated-pkg -> replacement-pkg)
//! - Package merges (pkg-a, pkg-b -> combined-pkg)
//! - Package splits (monolith-pkg -> pkg-core, pkg-extras)

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::str::FromStr;

/// Type of redirect operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectType {
    /// Package was renamed (old name -> new name)
    Rename,
    /// Package is obsoleted (deprecated -> replacement)
    Obsolete,
    /// Multiple packages merged into one (many -> one)
    Merge,
    /// Package split into multiple (one -> many)
    Split,
}

impl RedirectType {
    pub fn as_str(&self) -> &str {
        match self {
            RedirectType::Rename => "rename",
            RedirectType::Obsolete => "obsolete",
            RedirectType::Merge => "merge",
            RedirectType::Split => "split",
        }
    }
}

impl FromStr for RedirectType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "rename" => Ok(RedirectType::Rename),
            "obsolete" => Ok(RedirectType::Obsolete),
            "merge" => Ok(RedirectType::Merge),
            "split" => Ok(RedirectType::Split),
            _ => Err(format!("Invalid redirect type: {s}")),
        }
    }
}

impl std::fmt::Display for RedirectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Database representation of a package redirect
#[derive(Debug, Clone)]
pub struct Redirect {
    pub id: Option<i64>,
    /// Source package name (the name being redirected FROM)
    pub source_name: String,
    /// Source version constraint (None = all versions redirect)
    pub source_version: Option<String>,
    /// Target package name (the name being redirected TO)
    pub target_name: String,
    /// Target version constraint (None = use latest)
    pub target_version: Option<String>,
    /// Type of redirect
    pub redirect_type: RedirectType,
    /// Optional user-facing message explaining the redirect
    pub message: Option<String>,
    /// When the redirect was created
    pub created_at: Option<String>,
}

impl Redirect {
    /// Create a new redirect
    pub fn new(
        source_name: String,
        target_name: String,
        redirect_type: RedirectType,
    ) -> Self {
        Self {
            id: None,
            source_name,
            source_version: None,
            target_name,
            target_version: None,
            redirect_type,
            message: None,
            created_at: None,
        }
    }

    /// Create a rename redirect (most common case)
    pub fn rename(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self::new(old_name.into(), new_name.into(), RedirectType::Rename)
    }

    /// Create an obsolete redirect
    pub fn obsolete(deprecated: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self::new(deprecated.into(), replacement.into(), RedirectType::Obsolete)
    }

    /// Set the source version constraint
    pub fn with_source_version(mut self, version: impl Into<String>) -> Self {
        self.source_version = Some(version.into());
        self
    }

    /// Set the target version constraint
    pub fn with_target_version(mut self, version: impl Into<String>) -> Self {
        self.target_version = Some(version.into());
        self
    }

    /// Set the message
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Insert this redirect into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO redirects (source_name, source_version, target_name, target_version, redirect_type, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.source_name,
                &self.source_version,
                &self.target_name,
                &self.target_version,
                self.redirect_type.as_str(),
                &self.message,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a redirect by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_name, source_version, target_name, target_version, redirect_type, message, created_at
             FROM redirects WHERE id = ?1",
        )?;

        let redirect = stmt.query_row([id], Self::from_row).optional()?;
        Ok(redirect)
    }

    /// Find a redirect by source package name
    ///
    /// If version is provided, looks for version-specific redirect first,
    /// then falls back to unversioned redirect.
    pub fn find_by_source(conn: &Connection, source_name: &str, version: Option<&str>) -> Result<Option<Self>> {
        // First try to find a version-specific redirect
        if let Some(ver) = version {
            let mut stmt = conn.prepare(
                "SELECT id, source_name, source_version, target_name, target_version, redirect_type, message, created_at
                 FROM redirects WHERE source_name = ?1 AND source_version = ?2",
            )?;

            if let Some(redirect) = stmt.query_row([source_name, ver], Self::from_row).optional()? {
                return Ok(Some(redirect));
            }
        }

        // Fall back to unversioned redirect
        let mut stmt = conn.prepare(
            "SELECT id, source_name, source_version, target_name, target_version, redirect_type, message, created_at
             FROM redirects WHERE source_name = ?1 AND source_version IS NULL",
        )?;

        let redirect = stmt.query_row([source_name], Self::from_row).optional()?;
        Ok(redirect)
    }

    /// Find all redirects pointing to a target package
    pub fn find_by_target(conn: &Connection, target_name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_name, source_version, target_name, target_version, redirect_type, message, created_at
             FROM redirects WHERE target_name = ?1 ORDER BY source_name",
        )?;

        let redirects = stmt
            .query_map([target_name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(redirects)
    }

    /// List all redirects
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_name, source_version, target_name, target_version, redirect_type, message, created_at
             FROM redirects ORDER BY source_name, source_version",
        )?;

        let redirects = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(redirects)
    }

    /// List redirects by type
    pub fn list_by_type(conn: &Connection, redirect_type: RedirectType) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_name, source_version, target_name, target_version, redirect_type, message, created_at
             FROM redirects WHERE redirect_type = ?1 ORDER BY source_name",
        )?;

        let redirects = stmt
            .query_map([redirect_type.as_str()], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(redirects)
    }

    /// Delete a redirect by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM redirects WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete a redirect by source name
    pub fn delete_by_source(conn: &Connection, source_name: &str) -> Result<u64> {
        let count = conn.execute(
            "DELETE FROM redirects WHERE source_name = ?1",
            [source_name],
        )?;
        Ok(count as u64)
    }

    /// Follow redirect chain to find the final target
    ///
    /// Resolves transitive redirects (A -> B -> C returns C).
    /// Detects and returns error on circular redirects.
    pub fn resolve(conn: &Connection, package_name: &str, version: Option<&str>) -> Result<ResolveResult> {
        let mut current = package_name.to_string();
        let mut current_version = version.map(String::from);
        let mut chain = vec![current.clone()];
        let mut messages = Vec::new();

        // Maximum redirect depth to prevent infinite loops
        const MAX_DEPTH: usize = 10;

        for _ in 0..MAX_DEPTH {
            if let Some(redirect) = Self::find_by_source(conn, &current, current_version.as_deref())? {
                // Detect circular redirect
                if chain.contains(&redirect.target_name) {
                    return Err(crate::error::Error::ConflictError(format!(
                        "Circular redirect detected: {} -> {}",
                        current, redirect.target_name
                    )));
                }

                // Collect message if present
                if let Some(msg) = &redirect.message {
                    messages.push(msg.clone());
                }

                // Move to target
                current = redirect.target_name.clone();
                current_version = redirect.target_version.clone();
                chain.push(current.clone());
            } else {
                // No more redirects, we've reached the final target
                break;
            }
        }

        let was_redirected = chain.len() > 1;
        Ok(ResolveResult {
            original: package_name.to_string(),
            resolved: current,
            version: current_version,
            chain,
            messages,
            was_redirected,
        })
    }

    /// Convert a database row to a Redirect
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let type_str: String = row.get(5)?;
        let redirect_type = type_str.parse::<RedirectType>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        Ok(Self {
            id: Some(row.get(0)?),
            source_name: row.get(1)?,
            source_version: row.get(2)?,
            target_name: row.get(3)?,
            target_version: row.get(4)?,
            redirect_type,
            message: row.get(6)?,
            created_at: row.get(7)?,
        })
    }
}

impl std::fmt::Display for Redirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = if let Some(ref ver) = self.source_version {
            format!("{}={}", self.source_name, ver)
        } else {
            self.source_name.clone()
        };

        let target = if let Some(ref ver) = self.target_version {
            format!("{}={}", self.target_name, ver)
        } else {
            self.target_name.clone()
        };

        write!(f, "{} -> {} ({})", source, target, self.redirect_type)
    }
}

/// Result of resolving a package name through redirects
#[derive(Debug, Clone)]
pub struct ResolveResult {
    /// Original package name that was requested
    pub original: String,
    /// Final resolved package name
    pub resolved: String,
    /// Final resolved version constraint (if any)
    pub version: Option<String>,
    /// Chain of package names followed (original -> ... -> resolved)
    pub chain: Vec<String>,
    /// Messages collected from redirects
    pub messages: Vec<String>,
    /// Whether any redirect was followed
    pub was_redirected: bool,
}

impl ResolveResult {
    /// Print redirect messages to the user
    pub fn print_messages(&self) {
        for msg in &self.messages {
            eprintln!("Note: {}", msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_redirect_crud() {
        let (_temp, conn) = create_test_db();

        // Create a rename redirect
        let mut redirect = Redirect::rename("old-pkg", "new-pkg")
            .with_message("Package was renamed in v2.0");

        let id = redirect.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by ID
        let found = Redirect::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.source_name, "old-pkg");
        assert_eq!(found.target_name, "new-pkg");
        assert_eq!(found.redirect_type, RedirectType::Rename);
        assert_eq!(found.message, Some("Package was renamed in v2.0".to_string()));

        // Find by source
        let found = Redirect::find_by_source(&conn, "old-pkg", None).unwrap().unwrap();
        assert_eq!(found.target_name, "new-pkg");
    }

    #[test]
    fn test_redirect_resolve() {
        let (_temp, conn) = create_test_db();

        // Create chain: a -> b -> c
        let mut r1 = Redirect::rename("a", "b").with_message("a renamed to b");
        r1.insert(&conn).unwrap();

        let mut r2 = Redirect::rename("b", "c").with_message("b renamed to c");
        r2.insert(&conn).unwrap();

        // Resolve a -> should get c
        let result = Redirect::resolve(&conn, "a", None).unwrap();
        assert_eq!(result.original, "a");
        assert_eq!(result.resolved, "c");
        assert!(result.was_redirected);
        assert_eq!(result.chain, vec!["a", "b", "c"]);
        assert_eq!(result.messages.len(), 2);

        // Resolve c -> should stay c (no redirect)
        let result = Redirect::resolve(&conn, "c", None).unwrap();
        assert_eq!(result.resolved, "c");
        assert!(!result.was_redirected);
    }

    #[test]
    fn test_redirect_circular_detection() {
        let (_temp, conn) = create_test_db();

        // Create circular: a -> b -> a
        let mut r1 = Redirect::rename("a", "b");
        r1.insert(&conn).unwrap();

        let mut r2 = Redirect::rename("b", "a");
        r2.insert(&conn).unwrap();

        // Resolve should detect cycle
        let result = Redirect::resolve(&conn, "a", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Circular"));
    }

    #[test]
    fn test_redirect_version_specific() {
        let (_temp, conn) = create_test_db();

        // Create version-specific redirect
        let mut r1 = Redirect::rename("pkg", "new-pkg")
            .with_source_version("1.0");
        r1.insert(&conn).unwrap();

        // Create unversioned redirect
        let mut r2 = Redirect::rename("pkg", "other-pkg");
        r2.insert(&conn).unwrap();

        // Version 1.0 should go to new-pkg
        let found = Redirect::find_by_source(&conn, "pkg", Some("1.0")).unwrap().unwrap();
        assert_eq!(found.target_name, "new-pkg");

        // Version 2.0 should fall back to unversioned (other-pkg)
        let found = Redirect::find_by_source(&conn, "pkg", Some("2.0")).unwrap().unwrap();
        assert_eq!(found.target_name, "other-pkg");
    }

    #[test]
    fn test_redirect_list_by_type() {
        let (_temp, conn) = create_test_db();

        let mut r1 = Redirect::rename("old1", "new1");
        r1.insert(&conn).unwrap();

        let mut r2 = Redirect::obsolete("deprecated", "replacement");
        r2.insert(&conn).unwrap();

        let mut r3 = Redirect::rename("old2", "new2");
        r3.insert(&conn).unwrap();

        // List renames
        let renames = Redirect::list_by_type(&conn, RedirectType::Rename).unwrap();
        assert_eq!(renames.len(), 2);

        // List obsoletes
        let obsoletes = Redirect::list_by_type(&conn, RedirectType::Obsolete).unwrap();
        assert_eq!(obsoletes.len(), 1);
    }
}
