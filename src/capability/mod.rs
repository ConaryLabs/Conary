// src/capability/mod.rs
//! Capability declarations for CCS packages
//!
//! This module provides types and utilities for declaring what system resources
//! a package requires (network, filesystem, syscalls). Capabilities serve multiple
//! purposes:
//!
//! 1. **Documentation**: Clear declaration of package requirements
//! 2. **Audit Mode**: Compare declared capabilities vs observed behavior
//! 3. **Enforcement** (future): Apply restrictions via landlock/seccomp
//!
//! # Example
//!
//! ```toml
//! [capabilities]
//! version = 1
//! rationale = "Web server requiring network listeners and cache access"
//!
//! [capabilities.network]
//! outbound = ["443", "80"]
//! listen = ["80", "443"]
//!
//! [capabilities.filesystem]
//! read = ["/etc/nginx", "/etc/ssl/certs"]
//! write = ["/var/cache/nginx", "/var/log/nginx"]
//!
//! [capabilities.syscalls]
//! profile = "network-server"
//! ```

mod declaration;
pub mod inference;

pub use declaration::{
    CapabilityDeclaration, CapabilityValidationError, FilesystemCapabilities, NetworkCapabilities,
    SyscallCapabilities, SyscallProfile,
};

use crate::ccs::manifest::CcsManifest;
use rusqlite::Connection;
use thiserror::Error;

/// Errors related to capability operations
#[derive(Error, Debug)]
pub enum CapabilityError {
    #[error("Manifest error: {0}")]
    Manifest(#[from] crate::ccs::manifest::ManifestError),

    #[error("Validation error: {0}")]
    Validation(#[from] CapabilityValidationError),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Package not found: {0}")]
    PackageNotFound(String),

    #[error("No capabilities declared for package: {0}")]
    NoCapabilities(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

/// Result type for capability operations
pub type CapabilityResult<T> = Result<T, CapabilityError>;

/// Store capability declaration in the database
pub fn store_capabilities(
    conn: &Connection,
    trove_id: i64,
    capabilities: &CapabilityDeclaration,
) -> CapabilityResult<()> {
    let declaration_json = serde_json::to_string(capabilities)
        .map_err(|e| CapabilityError::Other(format!("Failed to serialize capabilities: {}", e)))?;

    conn.execute(
        "INSERT OR REPLACE INTO capabilities (trove_id, declaration_json, declaration_version)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![trove_id, declaration_json, capabilities.version],
    )?;

    Ok(())
}

/// Load capability declaration from the database
pub fn load_capabilities(
    conn: &Connection,
    trove_id: i64,
) -> CapabilityResult<Option<CapabilityDeclaration>> {
    let result: Option<String> = conn
        .query_row(
            "SELECT declaration_json FROM capabilities WHERE trove_id = ?1",
            [trove_id],
            |row| row.get(0),
        )
        .ok();

    match result {
        Some(json) => {
            let capabilities: CapabilityDeclaration = serde_json::from_str(&json)
                .map_err(|e| CapabilityError::Other(format!("Failed to parse capabilities: {}", e)))?;
            Ok(Some(capabilities))
        }
        None => Ok(None),
    }
}

/// Load capabilities for a package by name
pub fn load_capabilities_by_name(
    conn: &Connection,
    package_name: &str,
) -> CapabilityResult<Option<CapabilityDeclaration>> {
    // First find the trove_id
    let trove_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM troves WHERE name = ?1 AND type = 'package' ORDER BY id DESC LIMIT 1",
            [package_name],
            |row| row.get(0),
        )
        .ok();

    match trove_id {
        Some(id) => load_capabilities(conn, id),
        None => Err(CapabilityError::PackageNotFound(package_name.to_string())),
    }
}

/// Validate a ccs.toml manifest file for capability syntax
pub fn validate_manifest_capabilities(path: &str) -> CapabilityResult<()> {
    let manifest = CcsManifest::from_file(std::path::Path::new(path))?;

    if let Some(ref capabilities) = manifest.capabilities {
        capabilities.validate()?;
    }

    Ok(())
}

/// Audit status for capability compliance
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditStatus {
    /// Capabilities match observed behavior
    Compliant,
    /// Package has more privileges than declared (potential security issue)
    OverPrivileged,
    /// Package declares capabilities it doesn't use (could be tightened)
    UnderUtilized,
    /// No capabilities declared
    Undeclared,
}

impl std::fmt::Display for AuditStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compliant => write!(f, "compliant"),
            Self::OverPrivileged => write!(f, "over_privileged"),
            Self::UnderUtilized => write!(f, "under_utilized"),
            Self::Undeclared => write!(f, "undeclared"),
        }
    }
}

/// Capability audit result
#[derive(Debug, Clone)]
pub struct AuditResult {
    pub package_name: String,
    pub status: AuditStatus,
    pub violations: Vec<AuditViolation>,
}

/// A specific audit violation
#[derive(Debug, Clone)]
pub struct AuditViolation {
    pub category: String, // "network", "filesystem", "syscall"
    pub expected: String,
    pub observed: String,
    pub severity: ViolationSeverity,
}

/// Severity of an audit violation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationSeverity {
    /// Just a note (e.g., unused capability)
    Info,
    /// Should be addressed (e.g., missing declaration)
    Warning,
    /// Security concern (e.g., undeclared privileged access)
    Error,
}

/// List packages with capability status
pub fn list_packages_with_capabilities(
    conn: &Connection,
    missing_only: bool,
) -> CapabilityResult<Vec<(String, String, bool)>> {
    // (package_name, version, has_capabilities)
    let mut stmt = if missing_only {
        conn.prepare(
            "SELECT t.name, t.version, 0 as has_caps
             FROM troves t
             LEFT JOIN capabilities c ON t.id = c.trove_id
             WHERE t.type = 'package' AND c.id IS NULL
             ORDER BY t.name",
        )?
    } else {
        conn.prepare(
            "SELECT t.name, t.version,
                    CASE WHEN c.id IS NOT NULL THEN 1 ELSE 0 END as has_caps
             FROM troves t
             LEFT JOIN capabilities c ON t.id = c.trove_id
             WHERE t.type = 'package'
             ORDER BY t.name",
        )?
    };

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i32>(2)? == 1,
        ))
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                type TEXT NOT NULL
            );
            CREATE TABLE capabilities (
                id INTEGER PRIMARY KEY,
                trove_id INTEGER UNIQUE REFERENCES troves(id),
                declaration_json TEXT NOT NULL,
                declaration_version INTEGER DEFAULT 1,
                declared_at TEXT DEFAULT CURRENT_TIMESTAMP
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_store_and_load_capabilities() {
        let conn = setup_test_db();

        // Insert a test trove
        conn.execute(
            "INSERT INTO troves (id, name, version, type) VALUES (1, 'test-pkg', '1.0.0', 'package')",
            [],
        )
        .unwrap();

        // Create and store capabilities
        let mut caps = CapabilityDeclaration::default();
        caps.network.listen.push("80".to_string());
        caps.filesystem.read.push("/etc".to_string());

        store_capabilities(&conn, 1, &caps).unwrap();

        // Load and verify
        let loaded = load_capabilities(&conn, 1).unwrap().unwrap();
        assert_eq!(loaded.network.listen, vec!["80".to_string()]);
        assert_eq!(loaded.filesystem.read, vec!["/etc".to_string()]);
    }

    #[test]
    fn test_load_by_name() {
        let conn = setup_test_db();

        // Insert a test trove
        conn.execute(
            "INSERT INTO troves (id, name, version, type) VALUES (1, 'nginx', '1.24.0', 'package')",
            [],
        )
        .unwrap();

        let caps = CapabilityDeclaration::default();
        store_capabilities(&conn, 1, &caps).unwrap();

        let loaded = load_capabilities_by_name(&conn, "nginx").unwrap();
        assert!(loaded.is_some());

        let not_found = load_capabilities_by_name(&conn, "nonexistent");
        assert!(not_found.is_err());
    }

    #[test]
    fn test_list_packages() {
        let conn = setup_test_db();

        // Insert troves - some with capabilities, some without
        conn.execute(
            "INSERT INTO troves (id, name, version, type) VALUES (1, 'nginx', '1.24.0', 'package')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO troves (id, name, version, type) VALUES (2, 'curl', '8.0.0', 'package')",
            [],
        )
        .unwrap();

        let caps = CapabilityDeclaration::default();
        store_capabilities(&conn, 1, &caps).unwrap();

        // List all packages
        let all = list_packages_with_capabilities(&conn, false).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().find(|(n, _, has)| n == "nginx" && *has).is_some());
        assert!(all.iter().find(|(n, _, has)| n == "curl" && !*has).is_some());

        // List only missing
        let missing = list_packages_with_capabilities(&conn, true).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "curl");
    }
}
