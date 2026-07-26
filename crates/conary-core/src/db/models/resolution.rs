// conary-core/src/db/models/resolution.rs

//! Package resolution routing model
//!
//! This module provides per-package resolution strategies that transform
//! repositories from simple package storage into routing layers.
//!
//! # Resolution Strategies
//!
//! Each package can have multiple resolution strategies tried in order:
//! - **Remi**: Convert from distro package on-demand
//! - **Delegate**: Federate to another label/repository
//! - **RepositoryPackage**: Use the selected typed repository package row
//!
//! # Caching Policy
//!
//! Packages can specify caching policies:
//! - `cache_ttl`: How long to keep in cache (NULL = repository default)
//! - `cache_priority`: Higher = cached longer, lower priority for eviction
//!
//! # Example
//!
//! ```ignore
//! // Route one package through Remi.
//! let resolution = PackageResolution {
//!     name: "nginx".to_string(),
//!     strategies: vec![
//!         ResolutionStrategy::Remi {
//!             endpoint: "https://remi.example.com".to_string(),
//!             distro: "fedora-44".to_string(),
//!             source_name: None,
//!         },
//!     ],
//!     primary_strategy: PrimaryStrategy::Remi,
//!     cache_ttl: Some(2592000), // 30 days
//!     cache_priority: 100,
//!     ..Default::default()
//! };
//! ```

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

/// Resolution strategy for obtaining a package
///
/// Strategies are tried in order until one succeeds. Each variant
/// represents a different way to obtain the package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolutionStrategy {
    /// Convert from distro package via Remi proxy
    Remi {
        /// Remi server endpoint
        endpoint: String,
        /// Source distribution (fedora, arch, debian, etc.)
        distro: String,
        /// Source package name if different from target name
        #[serde(skip_serializing_if = "Option::is_none")]
        source_name: Option<String>,
    },

    /// Delegate to another label (federation)
    Delegate {
        /// Label to delegate to (e.g., "upstream@fedora:f43")
        label: String,
    },

    /// Use the selected typed repository package row.
    RepositoryPackage {
        /// ID of the repository_package row
        repository_package_id: i64,
    },
}

/// Primary strategy type for indexing
///
/// This is a denormalized column for fast filtering queries
/// (e.g., "find all Remi packages in this repo").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryStrategy {
    Remi,
    Delegate,
    RepositoryPackage,
}

impl PrimaryStrategy {
    /// Convert to database string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Remi => "remi",
            Self::Delegate => "delegate",
            Self::RepositoryPackage => "repository_package",
        }
    }

    /// Parse from database string
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "remi" => Ok(Self::Remi),
            "delegate" => Ok(Self::Delegate),
            "repository_package" => Ok(Self::RepositoryPackage),
            _ => Err(Error::ParseError(format!(
                "Unknown primary strategy: {}",
                s
            ))),
        }
    }
}

impl From<&ResolutionStrategy> for PrimaryStrategy {
    fn from(strategy: &ResolutionStrategy) -> Self {
        match strategy {
            ResolutionStrategy::Remi { .. } => Self::Remi,
            ResolutionStrategy::Delegate { .. } => Self::Delegate,
            ResolutionStrategy::RepositoryPackage { .. } => Self::RepositoryPackage,
        }
    }
}

/// Package resolution routing entry
///
/// Determines how to obtain a specific package from a repository.
#[derive(Debug, Clone)]
pub struct PackageResolution {
    pub id: Option<i64>,
    /// Repository this routing entry belongs to
    pub repository_id: i64,
    /// Package name to match
    pub name: String,
    /// Version constraint (None = any version)
    pub version: Option<String>,
    /// Resolution strategies to try (in order)
    pub strategies: Vec<ResolutionStrategy>,
    /// Primary strategy for indexing
    pub primary_strategy: PrimaryStrategy,
    /// Cache TTL in seconds (None = use repository default)
    pub cache_ttl: Option<i32>,
    /// Cache priority (higher = cached longer)
    pub cache_priority: i32,
}

impl PackageResolution {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, repository_id, name, version, strategies, \
         primary_strategy, cache_ttl, cache_priority";

    /// Create a new package resolution entry
    pub fn new(repository_id: i64, name: String, strategies: Vec<ResolutionStrategy>) -> Self {
        let primary_strategy = strategies
            .first()
            .map(PrimaryStrategy::from)
            .unwrap_or(PrimaryStrategy::RepositoryPackage);

        Self {
            id: None,
            repository_id,
            name,
            version: None,
            strategies,
            primary_strategy,
            cache_ttl: None,
            cache_priority: 0,
        }
    }

    /// Create a Remi resolution entry
    pub fn remi(repository_id: i64, name: String, endpoint: String, distro: String) -> Self {
        Self::new(
            repository_id,
            name,
            vec![ResolutionStrategy::Remi {
                endpoint,
                distro,
                source_name: None,
            }],
        )
    }

    /// Create a resolution entry for one selected repository package.
    pub fn repository_package(
        repository_id: i64,
        name: String,
        repository_package_id: i64,
    ) -> Self {
        Self::new(
            repository_id,
            name,
            vec![ResolutionStrategy::RepositoryPackage {
                repository_package_id,
            }],
        )
    }

    fn validate(&self) -> Result<()> {
        let first = self.strategies.first().ok_or_else(|| {
            Error::ConfigError(format!(
                "package resolution for '{}' must declare at least one strategy",
                self.name
            ))
        })?;
        let expected = PrimaryStrategy::from(first);
        if self.primary_strategy != expected {
            return Err(Error::ConfigError(format!(
                "package resolution for '{}' projects primary strategy '{}' but its first strategy is '{}'",
                self.name,
                self.primary_strategy.as_str(),
                expected.as_str()
            )));
        }
        Ok(())
    }

    /// Insert this resolution entry into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate()?;
        let strategies_json = serde_json::to_string(&self.strategies)
            .map_err(|e| Error::ParseError(format!("Failed to serialize strategies: {e}")))?;

        conn.execute(
            "INSERT INTO package_resolution
             (repository_id, name, version, strategies, primary_strategy, cache_ttl, cache_priority)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                self.repository_id,
                &self.name,
                &self.version,
                &strategies_json,
                self.primary_strategy.as_str(),
                self.cache_ttl,
                self.cache_priority,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find resolution entry by repository, name, and optional version
    ///
    /// Tries exact version match first, then falls back to any-version entry.
    pub fn find(
        conn: &Connection,
        repository_id: i64,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Self>> {
        // Try exact version match first
        if let Some(v) = version {
            let sql = format!(
                "SELECT {} FROM package_resolution \
                 WHERE repository_id = ?1 AND name = ?2 AND version = ?3",
                Self::COLUMNS
            );
            let mut stmt = conn.prepare(&sql)?;

            if let Some(entry) = stmt
                .query_row(
                    [repository_id.to_string(), name.to_string(), v.to_string()],
                    Self::from_row,
                )
                .optional()?
            {
                return Ok(Some(entry));
            }
        }

        // Fall back to any-version entry
        let sql = format!(
            "SELECT {} FROM package_resolution \
             WHERE repository_id = ?1 AND name = ?2 AND version IS NULL",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        stmt.query_row(params![repository_id, name], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Find all resolution entries for a repository
    pub fn find_by_repository(conn: &Connection, repository_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM package_resolution \
             WHERE repository_id = ?1 ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let entries = stmt
            .query_map([repository_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Find all resolution entries with a specific primary strategy
    pub fn find_by_strategy(
        conn: &Connection,
        repository_id: i64,
        strategy: PrimaryStrategy,
    ) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM package_resolution \
             WHERE repository_id = ?1 AND primary_strategy = ?2 \
             ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let entries = stmt
            .query_map(params![repository_id, strategy.as_str()], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Update this resolution entry
    pub fn update(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        let id = self.id.ok_or_else(|| {
            Error::MissingId("Cannot update resolution entry without ID".to_string())
        })?;

        let strategies_json = serde_json::to_string(&self.strategies)
            .map_err(|e| Error::ParseError(format!("Failed to serialize strategies: {e}")))?;

        conn.execute(
            "UPDATE package_resolution
             SET repository_id = ?1, name = ?2, version = ?3, strategies = ?4,
                 primary_strategy = ?5, cache_ttl = ?6, cache_priority = ?7
             WHERE id = ?8",
            params![
                self.repository_id,
                &self.name,
                &self.version,
                &strategies_json,
                self.primary_strategy.as_str(),
                self.cache_ttl,
                self.cache_priority,
                id,
            ],
        )?;

        Ok(())
    }

    /// Delete a resolution entry by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM package_resolution WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all resolution entries for a repository
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM package_resolution WHERE repository_id = ?1",
            [repository_id],
        )?;
        Ok(())
    }

    /// Convert from database row
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let strategies_json: String = row.get(4)?;
        let strategies: Vec<ResolutionStrategy> =
            serde_json::from_str(&strategies_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;

        let primary_strategy_str: String = row.get(5)?;
        let primary_strategy = PrimaryStrategy::parse(&primary_strategy_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
        })?;

        Ok(Self {
            id: Some(row.get(0)?),
            repository_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            strategies,
            primary_strategy,
            cache_ttl: row.get(6)?,
            cache_priority: row.get(7)?,
        })
    }
}

/// Cache tier for packages
///
/// Determines how long packages should be cached based on their importance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    /// Base system packages (glibc, kernel) - cache forever, delta updates
    BaseSystem,
    /// Popular applications (nginx, postgres) - cache 30 days
    Popular,
    /// Common libraries (openssl, zlib) - cache 7 days
    Common,
    /// Obscure packages - do not retain in cache
    Obscure,
    /// Group/collection metadata only
    Metadata,
}

impl CacheTier {
    /// Get the default cache TTL for this tier in seconds
    pub fn default_ttl(&self) -> Option<i32> {
        match self {
            Self::BaseSystem => None,                 // Never expire
            Self::Popular => Some(30 * 24 * 60 * 60), // 30 days
            Self::Common => Some(7 * 24 * 60 * 60),   // 7 days
            Self::Obscure => Some(0),                 // Don't cache
            Self::Metadata => Some(3600),             // 1 hour
        }
    }

    /// Get the default cache priority for this tier
    pub fn default_priority(&self) -> i32 {
        match self {
            Self::BaseSystem => 1000,
            Self::Popular => 100,
            Self::Common => 50,
            Self::Obscure => 0,
            Self::Metadata => 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    fn create_test_repo(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO repositories (name, url) VALUES ('test-repo', 'https://example.com')",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn test_resolution_strategy_serialization() {
        let repository_package = ResolutionStrategy::RepositoryPackage {
            repository_package_id: 42,
        };

        let json = serde_json::to_string(&repository_package).unwrap();
        let parsed: ResolutionStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(repository_package, parsed);
    }

    #[test]
    fn test_resolution_strategy_remi() {
        let remi = ResolutionStrategy::Remi {
            endpoint: "https://remi.example.com".to_string(),
            distro: "fedora".to_string(),
            source_name: Some("nginx-mainline".to_string()),
        };

        let json = serde_json::to_string(&remi).unwrap();
        assert!(json.contains("\"type\":\"remi\""));

        let parsed: ResolutionStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(remi, parsed);
    }

    #[test]
    fn test_package_resolution_crud() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);

        // Create
        let mut resolution =
            PackageResolution::repository_package(repo_id, "nginx".to_string(), 101);
        resolution.cache_ttl = Some(86400);
        resolution.cache_priority = 100;

        let id = resolution.insert(&conn).unwrap();
        assert!(id > 0);

        // Find
        let found = PackageResolution::find(&conn, repo_id, "nginx", None)
            .unwrap()
            .unwrap();
        assert_eq!(found.name, "nginx");
        assert_eq!(found.primary_strategy, PrimaryStrategy::RepositoryPackage);
        assert_eq!(found.cache_priority, 100);

        // Update
        let mut updated = found;
        updated.cache_priority = 200;
        updated.update(&conn).unwrap();

        let reloaded = PackageResolution::find(&conn, repo_id, "nginx", None)
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.cache_priority, 200);

        // Delete
        PackageResolution::delete(&conn, id).unwrap();
        let deleted = PackageResolution::find(&conn, repo_id, "nginx", None).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_package_resolution_version_matching() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);

        // Create version-specific entry
        let mut specific = PackageResolution::repository_package(repo_id, "nginx".to_string(), 101);
        specific.version = Some("1.24.0".to_string());
        specific.insert(&conn).unwrap();

        // Create any-version entry
        let mut any_version =
            PackageResolution::repository_package(repo_id, "nginx".to_string(), 102);
        any_version.insert(&conn).unwrap();

        // Exact version match should find specific
        let found = PackageResolution::find(&conn, repo_id, "nginx", Some("1.24.0"))
            .unwrap()
            .unwrap();
        assert!(
            matches!(&found.strategies[0], ResolutionStrategy::RepositoryPackage { repository_package_id } if *repository_package_id == 101)
        );

        // Different version should fall back to any-version
        let found = PackageResolution::find(&conn, repo_id, "nginx", Some("1.23.0"))
            .unwrap()
            .unwrap();
        assert!(
            matches!(&found.strategies[0], ResolutionStrategy::RepositoryPackage { repository_package_id } if *repository_package_id == 102)
        );

        // No version should use any-version
        let found = PackageResolution::find(&conn, repo_id, "nginx", None)
            .unwrap()
            .unwrap();
        assert!(
            matches!(&found.strategies[0], ResolutionStrategy::RepositoryPackage { repository_package_id } if *repository_package_id == 102)
        );
    }

    #[test]
    fn test_find_by_strategy() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);

        // Create exact repository-package entry
        let mut repository_package =
            PackageResolution::repository_package(repo_id, "nginx".to_string(), 101);
        repository_package.insert(&conn).unwrap();

        // Create remi entry
        let mut remi = PackageResolution::remi(
            repo_id,
            "obscure-tool".to_string(),
            "https://remi.example.com".to_string(),
            "fedora".to_string(),
        );
        remi.insert(&conn).unwrap();

        // Find only exact repository-package entries
        let repository_packages =
            PackageResolution::find_by_strategy(&conn, repo_id, PrimaryStrategy::RepositoryPackage)
                .unwrap();
        assert_eq!(repository_packages.len(), 1);
        assert_eq!(repository_packages[0].name, "nginx");

        // Find only remi entries
        let remis =
            PackageResolution::find_by_strategy(&conn, repo_id, PrimaryStrategy::Remi).unwrap();
        assert_eq!(remis.len(), 1);
        assert_eq!(remis[0].name, "obscure-tool");
    }

    #[test]
    fn test_primary_strategy_from_resolution() {
        let remi = ResolutionStrategy::Remi {
            endpoint: "ep".to_string(),
            distro: "fedora".to_string(),
            source_name: None,
        };
        assert_eq!(PrimaryStrategy::from(&remi), PrimaryStrategy::Remi);

        let delegate = ResolutionStrategy::Delegate {
            label: "upstream@fedora:f43".to_string(),
        };
        assert_eq!(PrimaryStrategy::from(&delegate), PrimaryStrategy::Delegate);

        let repository_package = ResolutionStrategy::RepositoryPackage {
            repository_package_id: 123,
        };
        assert_eq!(
            PrimaryStrategy::from(&repository_package),
            PrimaryStrategy::RepositoryPackage
        );
        assert!(PrimaryStrategy::parse("recipe").is_err());
        assert!(PrimaryStrategy::parse("legacy").is_err());
    }

    #[test]
    fn retired_recipe_resolution_strategy_is_rejected() {
        let old = serde_json::json!({
            "type": "recipe",
            "recipe_url": "https://example.test/package.recipe.toml",
            "source_urls": [],
            "patches": []
        });
        assert!(serde_json::from_value::<ResolutionStrategy>(old).is_err());
    }

    #[test]
    fn retired_binary_resolution_strategy_is_rejected() {
        let old = serde_json::json!({
            "type": "binary",
            "url": "https://unbound.example.invalid/package.ccs",
            "checksum": "sha256:unbound",
            "delta_base": null
        });
        assert!(serde_json::from_value::<ResolutionStrategy>(old).is_err());
        assert!(PrimaryStrategy::parse("binary").is_err());
    }

    #[test]
    fn test_cache_tier_defaults() {
        assert_eq!(CacheTier::BaseSystem.default_ttl(), None);
        assert_eq!(CacheTier::Popular.default_ttl(), Some(30 * 24 * 60 * 60));
        assert_eq!(CacheTier::Common.default_ttl(), Some(7 * 24 * 60 * 60));
        assert_eq!(CacheTier::Obscure.default_ttl(), Some(0));

        assert!(CacheTier::BaseSystem.default_priority() > CacheTier::Popular.default_priority());
        assert!(CacheTier::Popular.default_priority() > CacheTier::Common.default_priority());
    }

    #[test]
    fn test_multiple_strategies() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);

        // Create entry with fallback strategies
        let mut resolution = PackageResolution::new(
            repo_id,
            "complex-pkg".to_string(),
            vec![
                // Try the exact authenticated repository row first.
                ResolutionStrategy::RepositoryPackage {
                    repository_package_id: 101,
                },
                // Fall back to Remi
                ResolutionStrategy::Remi {
                    endpoint: "https://remi.example.com".to_string(),
                    distro: "arch".to_string(),
                    source_name: None,
                },
            ],
        );
        resolution.insert(&conn).unwrap();

        let found = PackageResolution::find(&conn, repo_id, "complex-pkg", None)
            .unwrap()
            .unwrap();
        assert_eq!(found.strategies.len(), 2);
        assert_eq!(found.primary_strategy, PrimaryStrategy::RepositoryPackage);

        // Verify strategies are preserved in order
        assert!(matches!(
            found.strategies[0],
            ResolutionStrategy::RepositoryPackage { .. }
        ));
        assert!(matches!(
            found.strategies[1],
            ResolutionStrategy::Remi { .. }
        ));
    }
}
