// conary-core/src/db/models/canonical.rs

//! Canonical package identity models
//!
//! These models map distro-neutral package identities to distro-specific
//! implementations, enabling cross-distro package resolution.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::fmt;

/// Exact authority that established a canonical package implementation.
///
/// Contract mappings are local, versioned repository truth. Remi mappings are
/// exact snapshots received from a configured Remi endpoint. Discovery caches
/// never produce this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CanonicalMappingAuthority {
    Contract,
    Remi,
}

impl CanonicalMappingAuthority {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contract => "contract",
            Self::Remi => "remi",
        }
    }

    fn from_persisted(value: &str) -> rusqlite::Result<Self> {
        match value {
            "contract" => Ok(Self::Contract),
            "remi" => Ok(Self::Remi),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown canonical mapping authority '{other}'"),
                )
                .into(),
            )),
        }
    }
}

impl fmt::Display for CanonicalMappingAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A distro-neutral package identity
#[derive(Debug, Clone)]
pub struct CanonicalPackage {
    pub id: Option<i64>,
    pub name: String,
    pub appstream_id: Option<String>,
    pub description: Option<String>,
    pub kind: String,
    pub category: Option<String>,
}

impl CanonicalPackage {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, name, appstream_id, description, kind, category";

    /// Create a new canonical package
    pub fn new(name: String, kind: String) -> Self {
        Self {
            id: None,
            name,
            appstream_id: None,
            description: None,
            kind,
            category: None,
        }
    }

    /// Insert into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO canonical_packages (name, appstream_id, description, kind, category)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.name,
                &self.appstream_id,
                &self.description,
                &self.kind,
                &self.category,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert this identity or verify and enrich an identical existing row.
    ///
    /// Missing optional metadata may be filled, but an existing non-null value
    /// cannot be replaced by a different value.
    pub fn insert_or_verify(&mut self, conn: &Connection) -> Result<i64> {
        let Some(existing) = Self::find_by_name(conn, &self.name)? else {
            return self.insert(conn);
        };

        if existing.kind != self.kind {
            return Err(crate::Error::ConflictError(format!(
                "canonical package '{}' has kind '{}' but incoming authority declares '{}'",
                self.name, existing.kind, self.kind
            )));
        }
        verify_optional_metadata(
            &self.name,
            "appstream_id",
            existing.appstream_id.as_deref(),
            self.appstream_id.as_deref(),
        )?;
        verify_optional_metadata(
            &self.name,
            "description",
            existing.description.as_deref(),
            self.description.as_deref(),
        )?;
        verify_optional_metadata(
            &self.name,
            "category",
            existing.category.as_deref(),
            self.category.as_deref(),
        )?;

        let id = existing.id.expect("persisted canonical package has an id");
        conn.execute(
            "UPDATE canonical_packages
             SET appstream_id = COALESCE(appstream_id, ?1),
                 description = COALESCE(description, ?2),
                 category = COALESCE(category, ?3)
             WHERE id = ?4",
            params![&self.appstream_id, &self.description, &self.category, id],
        )?;
        self.id = Some(id);
        Ok(id)
    }

    /// Find by canonical name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM canonical_packages WHERE name = ?1",
            Self::COLUMNS
        );
        let result = conn.query_row(&sql, [name], Self::from_row).optional()?;
        Ok(result)
    }

    /// Find by an exact AppStream ID.
    ///
    /// Multiple owners are corrupt authority and fail rather than selecting a
    /// row by SQLite iteration order.
    pub fn find_by_appstream_id(conn: &Connection, appstream_id: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM canonical_packages WHERE appstream_id = ?1 ORDER BY id",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        let mut packages = statement
            .query_map([appstream_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match packages.len() {
            0 => Ok(None),
            1 => Ok(packages.pop()),
            count => Err(crate::Error::ResolutionError(format!(
                "AppStream ID '{appstream_id}' has {count} canonical owners"
            ))),
        }
    }

    /// Find by id
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM canonical_packages WHERE id = ?1",
            Self::COLUMNS
        );
        let result = conn.query_row(&sql, [id], Self::from_row).optional()?;
        Ok(result)
    }

    /// Resolve an exact identity to a canonical package:
    /// 1. Try as canonical name
    /// 2. Try as an exact AppStream ID
    /// 3. Try as an exact distro package name
    ///
    /// No string-shape heuristic participates. Ambiguous reverse mappings are
    /// errors and require the caller to name a canonical identity or profile.
    pub fn resolve_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        if let Some(pkg) = Self::find_by_name(conn, name)? {
            return Ok(Some(pkg));
        }

        if let Some(pkg) = Self::find_by_appstream_id(conn, name)? {
            return Ok(Some(pkg));
        }

        if let Some(impl_entry) = PackageImplementation::find_by_any_distro_name(conn, name)? {
            return Self::find_by_id(conn, impl_entry.canonical_id);
        }

        Ok(None)
    }

    /// List all canonical packages of a given kind
    pub fn list_by_kind(conn: &Connection, kind: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM canonical_packages WHERE kind = ?1 ORDER BY name",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([kind], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Search canonical packages by name or description (LIKE)
    pub fn search(conn: &Connection, query: &str) -> Result<Vec<Self>> {
        let pattern = format!("%{query}%");
        let sql = format!(
            "SELECT {} FROM canonical_packages \
             WHERE name LIKE ?1 OR description LIKE ?1 ORDER BY name",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([&pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Map a database row to a `CanonicalPackage`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            appstream_id: row.get(2)?,
            description: row.get(3)?,
            kind: row.get(4)?,
            category: row.get(5)?,
        })
    }
}

/// A distro-specific implementation of a canonical package
#[derive(Debug, Clone)]
pub struct PackageImplementation {
    pub id: Option<i64>,
    pub canonical_id: i64,
    pub distro: String,
    pub distro_name: String,
    pub source: CanonicalMappingAuthority,
}

impl PackageImplementation {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, canonical_id, distro, distro_name, source";

    /// Create a new implementation entry
    pub fn new(
        canonical_id: i64,
        distro: String,
        distro_name: String,
        source: CanonicalMappingAuthority,
    ) -> Self {
        Self {
            id: None,
            canonical_id,
            distro,
            distro_name,
            source,
        }
    }

    /// Insert into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate()?;
        conn.execute(
            "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &self.canonical_id,
                &self.distro,
                &self.distro_name,
                self.source.as_str(),
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert this exact mapping or verify the existing authority.
    ///
    /// A Contract mapping is never demoted by an identical Remi snapshot. If
    /// the same exact package was learned from Remi first, a later Contract
    /// write promotes it. Any package-name disagreement fails closed.
    pub fn insert_or_verify(&mut self, conn: &Connection) -> Result<bool> {
        self.validate()?;
        let Some(existing) = Self::find_for_distro(conn, self.canonical_id, &self.distro)? else {
            self.insert(conn)?;
            return Ok(true);
        };

        if existing.distro_name != self.distro_name {
            return Err(crate::Error::ConflictError(format!(
                "canonical package {} profile '{}' maps to '{}' from {} authority, not incoming '{}' from {} authority",
                self.canonical_id,
                self.distro,
                existing.distro_name,
                existing.source,
                self.distro_name,
                self.source
            )));
        }

        let effective_source = match (existing.source, self.source) {
            (current, incoming) if current == incoming => current,
            (CanonicalMappingAuthority::Contract, CanonicalMappingAuthority::Remi) => {
                CanonicalMappingAuthority::Contract
            }
            (CanonicalMappingAuthority::Remi, CanonicalMappingAuthority::Contract) => {
                conn.execute(
                    "UPDATE package_implementations SET source = 'contract' WHERE id = ?1",
                    [existing.id.expect("persisted implementation has an id")],
                )?;
                CanonicalMappingAuthority::Contract
            }
            _ => unreachable!("canonical authority has exactly two variants"),
        };

        self.id = existing.id;
        self.source = effective_source;
        Ok(false)
    }

    /// Find all implementations for a canonical package.
    pub fn find_by_canonical(conn: &Connection, canonical_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM package_implementations
             WHERE canonical_id = ?1 ORDER BY distro, distro_name, id",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([canonical_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find one exact distro/name mapping, rejecting multiple canonical owners.
    pub fn find_by_distro_name(
        conn: &Connection,
        distro: &str,
        name: &str,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM package_implementations \
             WHERE distro = ?1 AND distro_name = ?2 ORDER BY canonical_id, id",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        let mut implementations = statement
            .query_map(params![distro, name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match implementations.len() {
            0 => Ok(None),
            1 => Ok(implementations.pop()),
            count => Err(crate::Error::ResolutionError(format!(
                "package '{name}' in profile '{distro}' maps to {count} canonical identities"
            ))),
        }
    }

    /// Resolve a distro package name only when every matching profile row
    /// points to one canonical identity.
    pub fn find_by_any_distro_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM package_implementations
             WHERE distro_name = ?1 ORDER BY canonical_id, distro, id",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        let mut implementations = statement
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if implementations.is_empty() {
            return Ok(None);
        }
        let canonical_id = implementations[0].canonical_id;
        if implementations
            .iter()
            .any(|implementation| implementation.canonical_id != canonical_id)
        {
            return Err(crate::Error::ResolutionError(format!(
                "package name '{name}' maps to multiple canonical identities; use an exact canonical name or source profile"
            )));
        }
        Ok(Some(implementations.remove(0)))
    }

    /// Find the exact implementation for a canonical package/profile pair.
    pub fn find_for_distro(
        conn: &Connection,
        canonical_id: i64,
        distro: &str,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM package_implementations \
             WHERE canonical_id = ?1 AND distro = ?2 ORDER BY id",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        let mut implementations = statement
            .query_map(params![canonical_id, distro], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match implementations.len() {
            0 => Ok(None),
            1 => Ok(implementations.pop()),
            count => Err(crate::Error::ResolutionError(format!(
                "canonical package {canonical_id} has {count} implementations for profile '{distro}'"
            ))),
        }
    }

    /// Map a database row to a `PackageImplementation`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let source = row.get::<_, String>(4)?;
        Ok(Self {
            id: Some(row.get(0)?),
            canonical_id: row.get(1)?,
            distro: row.get(2)?,
            distro_name: row.get(3)?,
            source: CanonicalMappingAuthority::from_persisted(&source)?,
        })
    }

    fn validate(&self) -> Result<()> {
        if crate::repository::supported_profiles::profile_by_public_id(&self.distro).is_none() {
            return Err(crate::Error::ConfigError(format!(
                "canonical mapping names unsupported public profile '{}'",
                self.distro
            )));
        }
        if self.distro_name.is_empty()
            || self.distro_name != self.distro_name.trim()
            || self.distro_name.chars().any(char::is_whitespace)
            || self.distro_name.chars().any(char::is_control)
        {
            return Err(crate::Error::ConfigError(format!(
                "canonical mapping for profile '{}' has invalid exact package name '{}'",
                self.distro, self.distro_name
            )));
        }
        Ok(())
    }
}

fn verify_optional_metadata(
    canonical_name: &str,
    field: &str,
    existing: Option<&str>,
    incoming: Option<&str>,
) -> Result<()> {
    if let (Some(existing), Some(incoming)) = (existing, incoming)
        && existing != incoming
    {
        return Err(crate::Error::ConflictError(format!(
            "canonical package '{canonical_name}' has {field} '{existing}' but incoming authority declares '{incoming}'"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn test_insert_and_find_canonical() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg.description = Some("Web browser".to_string());
        pkg.category = Some("browser".to_string());
        let id = pkg.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by name
        let found = CanonicalPackage::find_by_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(found.name, "firefox");
        assert_eq!(found.kind, "package");
        assert_eq!(found.description, Some("Web browser".to_string()));
        assert_eq!(found.category, Some("browser".to_string()));

        // Find by id
        let found = CanonicalPackage::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "firefox");

        // insert_or_verify returns the identical existing id
        let mut dup = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        let dup_id = dup.insert_or_verify(&conn).unwrap();
        assert_eq!(dup_id, id);
    }

    #[test]
    fn test_find_by_appstream_id() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg.appstream_id = Some("org.mozilla.firefox".to_string());
        pkg.insert(&conn).unwrap();

        let found = CanonicalPackage::find_by_appstream_id(&conn, "org.mozilla.firefox")
            .unwrap()
            .unwrap();
        assert_eq!(found.name, "firefox");

        // Not found
        let missing = CanonicalPackage::find_by_appstream_id(&conn, "org.nonexistent.app").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn schema_rejects_duplicate_appstream_identity() {
        let (_temp, conn) = create_test_db();
        let mut first = CanonicalPackage::new("firefox".into(), "package".into());
        first.appstream_id = Some("org.mozilla.Firefox".into());
        first.insert(&conn).unwrap();

        let mut second = CanonicalPackage::new("firefox-esr".into(), "package".into());
        second.appstream_id = Some("org.mozilla.Firefox".into());
        assert!(second.insert(&conn).is_err());
    }

    #[test]
    fn test_insert_and_find_implementation() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        let can_id = pkg.insert(&conn).unwrap();

        let mut impl_fed = PackageImplementation::new(
            can_id,
            "fedora-44".to_string(),
            "firefox".to_string(),
            CanonicalMappingAuthority::Contract,
        );
        impl_fed.insert(&conn).unwrap();

        let mut impl_deb = PackageImplementation::new(
            can_id,
            "ubuntu-26.04".to_string(),
            "firefox-esr".to_string(),
            CanonicalMappingAuthority::Contract,
        );
        impl_deb.insert(&conn).unwrap();

        // Find by canonical
        let impls = PackageImplementation::find_by_canonical(&conn, can_id).unwrap();
        assert_eq!(impls.len(), 2);

        // Find by distro name
        let found =
            PackageImplementation::find_by_distro_name(&conn, "ubuntu-26.04", "firefox-esr")
                .unwrap()
                .unwrap();
        assert_eq!(found.distro, "ubuntu-26.04");
        assert_eq!(found.distro_name, "firefox-esr");

        // Find by any distro name
        let found = PackageImplementation::find_by_any_distro_name(&conn, "firefox-esr")
            .unwrap()
            .unwrap();
        assert_eq!(found.distro, "ubuntu-26.04");

        // Find for distro
        let found = PackageImplementation::find_for_distro(&conn, can_id, "fedora-44")
            .unwrap()
            .unwrap();
        assert_eq!(found.distro_name, "firefox");

        // insert_or_verify preserves an identical mapping
        let mut dup = PackageImplementation::new(
            can_id,
            "fedora-44".to_string(),
            "firefox".to_string(),
            CanonicalMappingAuthority::Remi,
        );
        assert!(!dup.insert_or_verify(&conn).unwrap());
        assert_eq!(dup.source, CanonicalMappingAuthority::Contract);
    }

    #[test]
    fn test_resolve_name_to_canonical() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg.appstream_id = Some("org.mozilla.firefox".to_string());
        let can_id = pkg.insert(&conn).unwrap();

        let mut impl_deb = PackageImplementation::new(
            can_id,
            "ubuntu-26.04".to_string(),
            "firefox-esr".to_string(),
            CanonicalMappingAuthority::Contract,
        );
        impl_deb.insert(&conn).unwrap();

        // Resolve by canonical name
        let resolved = CanonicalPackage::resolve_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        // Resolve by AppStream ID
        let resolved = CanonicalPackage::resolve_name(&conn, "org.mozilla.firefox")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        conn.execute(
            "UPDATE canonical_packages SET appstream_id = 'firefox-app' WHERE id = ?1",
            [can_id],
        )
        .unwrap();
        let resolved = CanonicalPackage::resolve_name(&conn, "firefox-app")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        // Resolve by distro-specific name
        let resolved = CanonicalPackage::resolve_name(&conn, "firefox-esr")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        // Unknown name returns None
        let missing = CanonicalPackage::resolve_name(&conn, "nonexistent").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn ambiguous_reverse_package_name_is_rejected() {
        let (_temp, conn) = create_test_db();
        let mut runtime = CanonicalPackage::new("openssl-runtime".into(), "package".into());
        let runtime_id = runtime.insert(&conn).unwrap();
        let mut development = CanonicalPackage::new("openssl-devel".into(), "package".into());
        let development_id = development.insert(&conn).unwrap();

        PackageImplementation::new(
            runtime_id,
            "arch".into(),
            "openssl".into(),
            CanonicalMappingAuthority::Contract,
        )
        .insert(&conn)
        .unwrap();
        PackageImplementation::new(
            development_id,
            "arch".into(),
            "openssl".into(),
            CanonicalMappingAuthority::Contract,
        )
        .insert(&conn)
        .unwrap();

        let error = CanonicalPackage::resolve_name(&conn, "openssl").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("maps to multiple canonical identities")
        );
    }

    #[test]
    fn implementation_conflict_is_rejected_without_demoting_contract_authority() {
        let (_temp, conn) = create_test_db();
        let mut canonical = CanonicalPackage::new("openssl".into(), "package".into());
        let canonical_id = canonical.insert(&conn).unwrap();

        PackageImplementation::new(
            canonical_id,
            "fedora-44".into(),
            "openssl".into(),
            CanonicalMappingAuthority::Contract,
        )
        .insert(&conn)
        .unwrap();

        let mut identical_remi = PackageImplementation::new(
            canonical_id,
            "fedora-44".into(),
            "openssl".into(),
            CanonicalMappingAuthority::Remi,
        );
        assert!(!identical_remi.insert_or_verify(&conn).unwrap());
        assert_eq!(identical_remi.source, CanonicalMappingAuthority::Contract);

        let mut conflicting_remi = PackageImplementation::new(
            canonical_id,
            "fedora-44".into(),
            "openssl-libs".into(),
            CanonicalMappingAuthority::Remi,
        );
        let error = conflicting_remi.insert_or_verify(&conn).unwrap_err();
        assert!(error.to_string().contains("not incoming 'openssl-libs'"));
    }

    #[test]
    fn schema_rejects_unknown_profiles_and_authority_values() {
        let (_temp, conn) = create_test_db();
        let mut canonical = CanonicalPackage::new("curl".into(), "package".into());
        let canonical_id = canonical.insert(&conn).unwrap();

        let unknown_profile = conn.execute(
            "INSERT INTO package_implementations
             (canonical_id, distro, distro_name, source)
             VALUES (?1, 'fedora', 'curl', 'contract')",
            [canonical_id],
        );
        assert!(unknown_profile.is_err());

        let unknown_authority = conn.execute(
            "INSERT INTO package_implementations
             (canonical_id, distro, distro_name, source)
             VALUES (?1, 'fedora-44', 'curl', 'auto')",
            [canonical_id],
        );
        assert!(unknown_authority.is_err());
    }

    #[test]
    fn test_list_groups() {
        let (_temp, conn) = create_test_db();

        let mut pkg1 = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg1.insert(&conn).unwrap();

        let mut pkg2 = CanonicalPackage::new("bash".to_string(), "package".to_string());
        pkg2.insert(&conn).unwrap();

        let mut grp = CanonicalPackage::new("web-browsers".to_string(), "group".to_string());
        grp.description = Some("Web browser packages".to_string());
        grp.insert(&conn).unwrap();

        // list_by_kind
        let packages = CanonicalPackage::list_by_kind(&conn, "package").unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "bash"); // ordered by name

        let groups = CanonicalPackage::list_by_kind(&conn, "group").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "web-browsers");

        // search
        let results = CanonicalPackage::search(&conn, "browser").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "web-browsers");
    }
}
