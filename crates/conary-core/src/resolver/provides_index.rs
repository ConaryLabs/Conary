// conary-core/src/resolver/provides_index.rs

//! Pre-built index mapping capability names to provider packages.
//!
//! Modeled after libsolv's `pool_createwhatprovides()`. Built once at
//! resolution start from three data sources:
//! 1. `repository_provides` (per-distro provides from repo sync)
//! 2. `provides` (installed package provides)
//! 3. `appstream_provides` (cross-distro provides from AppStream)

use crate::error::Result;
use crate::repository::dependency_model::ProvideVersionRelation;
use crate::repository::distro::require_version_scheme_from_db;
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, provided_range_matches_requirement,
};
use rusqlite::Connection;
use std::collections::HashMap;

/// A single provider entry in the index.
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    /// Repository package ID (for repo-sourced provides)
    pub repo_package_id: Option<i64>,
    /// Installed trove ID (for locally installed provides)
    pub installed_trove_id: Option<i64>,
    /// Canonical package ID (for AppStream cross-distro provides)
    pub canonical_id: Option<i64>,
    /// Version of the provide (e.g., "3.2.0" for libssl.so.3)
    pub provide_version: Option<String>,
    /// Ordered relation associated with `provide_version`.
    pub version_relation: Option<ProvideVersionRelation>,
    /// Version comparison scheme
    pub version_scheme: Option<VersionScheme>,
}

/// Pre-built capability-to-provider index.
///
/// Built once at resolution start. All lookups are O(1) HashMap access.
pub struct ProvidesIndex {
    providers: HashMap<String, Vec<ProviderEntry>>,
}

impl ProvidesIndex {
    /// Build the index from all available provide sources.
    pub fn build(conn: &Connection) -> Result<Self> {
        let mut providers: HashMap<String, Vec<ProviderEntry>> = HashMap::new();

        // 1. Repository provides (from sync)
        {
            let mut stmt = conn.prepare(
                "SELECT rp.capability, rp.version, rp.version_relation, rp.version_scheme,
                        rp.repository_package_id
                 FROM repository_provides rp
                 JOIN repository_packages pkg ON rp.repository_package_id = pkg.id
                 JOIN repositories r ON pkg.repository_id = r.id
                 WHERE r.enabled = 1",
            )?;
            let rows = stmt.query_and_then([], |row| -> Result<(String, ProviderEntry)> {
                let cap: String = row.get(0)?;
                let version: Option<String> = row.get(1)?;
                let relation = provider_relation(row.get::<_, Option<String>>(2)?, &version)?;
                let scheme_str: Option<String> = row.get(3)?;
                let pkg_id: i64 = row.get(4)?;
                let version_scheme = Some(require_version_scheme_from_db(
                    scheme_str.as_deref(),
                    format!("repository provide '{cap}' for package row {pkg_id}"),
                )?);
                Ok((
                    cap,
                    ProviderEntry {
                        repo_package_id: Some(pkg_id),
                        installed_trove_id: None,
                        canonical_id: None,
                        provide_version: version,
                        version_relation: relation,
                        version_scheme,
                    },
                ))
            })?;
            for row in rows {
                let (capability, provider) = row?;
                providers.entry(capability).or_default().push(provider);
            }
        }

        // 2. Installed provides
        {
            let mut stmt = conn.prepare(
                "SELECT p.capability, p.version, p.version_relation, p.version_scheme, p.trove_id
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id",
            )?;
            let rows = stmt.query_and_then([], |row| -> Result<(String, ProviderEntry)> {
                let cap: String = row.get(0)?;
                let version: Option<String> = row.get(1)?;
                let relation = provider_relation(row.get::<_, Option<String>>(2)?, &version)?;
                let installed_scheme = crate::db::models::version_scheme_from_row(row, 3)?;
                let trove_id: i64 = row.get(4)?;
                let version_scheme = Some(installed_scheme);
                Ok((
                    cap,
                    ProviderEntry {
                        repo_package_id: None,
                        installed_trove_id: Some(trove_id),
                        canonical_id: None,
                        provide_version: version,
                        version_relation: relation,
                        version_scheme,
                    },
                ))
            })?;
            for row in rows {
                let (capability, provider) = row?;
                providers.entry(capability).or_default().push(provider);
            }
        }

        // 3. AppStream cross-distro provides
        {
            let mut stmt =
                conn.prepare("SELECT ap.capability, ap.canonical_id FROM appstream_provides ap")?;
            let rows = stmt.query_map([], |row| {
                let cap: String = row.get(0)?;
                let canonical_id: i64 = row.get(1)?;
                Ok((
                    cap,
                    ProviderEntry {
                        repo_package_id: None,
                        installed_trove_id: None,
                        canonical_id: Some(canonical_id),
                        provide_version: None,
                        version_relation: None,
                        version_scheme: None,
                    },
                ))
            })?;
            for row in rows {
                let (capability, provider) = row?;
                providers.entry(capability).or_default().push(provider);
            }
        }

        Ok(Self { providers })
    }

    /// Find all providers for a capability name.
    pub fn find_providers(&self, capability: &str) -> &[ProviderEntry] {
        self.providers
            .get(capability)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Find providers whose version satisfies a constraint.
    pub fn find_providers_constrained(
        &self,
        capability: &str,
        constraint: &RepoVersionConstraint,
        scheme: VersionScheme,
    ) -> Result<Vec<&ProviderEntry>> {
        let mut matches = Vec::new();
        for provider in self.find_providers(capability) {
            let matched = match provider.version_scheme {
                Some(provider_scheme) if provider_scheme == scheme => {
                    provided_range_matches_requirement(
                        scheme,
                        provider.version_relation,
                        provider.provide_version.as_deref(),
                        constraint,
                    )?
                }
                Some(_) => false,
                None => matches!(constraint, RepoVersionConstraint::Any),
            };
            if matched {
                matches.push(provider);
            }
        }
        Ok(matches)
    }

    /// Total number of unique capabilities indexed.
    pub fn capability_count(&self) -> usize {
        self.providers.len()
    }

    /// Total number of provider entries across all capabilities.
    pub fn provider_count(&self) -> usize {
        self.providers.values().map(|v| v.len()).sum()
    }
}

fn provider_relation(
    relation: Option<String>,
    version: &Option<String>,
) -> rusqlite::Result<Option<ProvideVersionRelation>> {
    match (relation, version) {
        (None, None) => Ok(None),
        (Some(relation), Some(_)) => ProvideVersionRelation::parse_exact(&relation)
            .map(Some)
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                )
            }),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "provider range relation and version must be paired",
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{ProvideEntry, RepositoryProvide};
    use crate::db::testing::create_test_db;
    use crate::repository::dependency_model::{
        ProvideArchitectureQualifier, RepositoryCapabilityKind,
    };

    #[test]
    fn test_provides_index_finds_repo_providers() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora-41', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'openssl-libs', '3.2.0', 'sha256:abc', 1024, 'https://example.com/pkg.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();
        let pkg_id = conn.last_insert_rowid();

        RepositoryProvide::new(
            pkg_id,
            "libssl.so.3".to_string(),
            Some("3.2.0".to_string()),
            "soname".to_string(),
            None,
            VersionScheme::Rpm,
        )
        .insert(&conn)
        .unwrap();

        let index = ProvidesIndex::build(&conn).unwrap();
        let providers = index.find_providers("libssl.so.3");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].repo_package_id, Some(pkg_id));
        assert_eq!(providers[0].provide_version.as_deref(), Some("3.2.0"));
    }

    #[test]
    fn test_provides_index_finds_installed_providers() {
        let (_temp, conn) = create_test_db();

        // Insert a trove and a provide for it
        conn.execute(
            "INSERT INTO troves (name, version, type, install_source, install_reason, version_scheme)
             VALUES ('openssl-libs', '3.2.0', 'package', 'repository', 'explicit', 'rpm')",
            [],
        )
        .unwrap();
        let trove_id = conn.last_insert_rowid();

        ProvideEntry::new_typed(
            trove_id,
            RepositoryCapabilityKind::Soname,
            "libssl.so.3".to_string(),
            None,
            VersionScheme::Rpm,
            ProvideArchitectureQualifier::Implicit,
        )
        .insert(&conn)
        .unwrap();

        let index = ProvidesIndex::build(&conn).unwrap();
        let providers = index.find_providers("libssl.so.3");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].installed_trove_id, Some(trove_id));
    }

    #[test]
    fn test_provides_index_finds_appstream_providers() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('openssl', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
             VALUES (?1, 'library', 'libssl.so.3')",
            [canonical_id],
        )
        .unwrap();

        let index = ProvidesIndex::build(&conn).unwrap();
        let providers = index.find_providers("libssl.so.3");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].canonical_id, Some(canonical_id));
    }

    #[test]
    fn test_provides_index_empty_for_unknown() {
        let (_temp, conn) = create_test_db();
        let index = ProvidesIndex::build(&conn).unwrap();
        assert!(index.find_providers("nonexistent.so.1").is_empty());
        assert_eq!(index.capability_count(), 0);
        assert_eq!(index.provider_count(), 0);
    }

    #[test]
    fn test_provides_index_constrained_lookup() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        // Two versions of the same capability
        for (version, suffix) in [("2.0", "a"), ("3.0", "b")] {
            conn.execute(
                "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
                 VALUES (?1, ?2, ?3, ?4, 100, 'https://example.com/x', 'rpm')",
                rusqlite::params![repo_id, format!("libfoo-{version}"), version, format!("sha256:{suffix}")],
            )
            .unwrap();
            let pkg_id = conn.last_insert_rowid();
            RepositoryProvide::new(
                pkg_id,
                "libfoo.so".to_string(),
                Some(version.to_string()),
                "soname".to_string(),
                None,
                VersionScheme::Rpm,
            )
            .insert(&conn)
            .unwrap();
        }

        let index = ProvidesIndex::build(&conn).unwrap();

        // All providers
        assert_eq!(index.find_providers("libfoo.so").len(), 2);

        // Only >= 3.0
        let constrained = index
            .find_providers_constrained(
                "libfoo.so",
                &RepoVersionConstraint::GreaterOrEqual("3.0".to_string()),
                VersionScheme::Rpm,
            )
            .unwrap();
        assert_eq!(constrained.len(), 1);
        assert_eq!(constrained[0].provide_version.as_deref(), Some("3.0"));
    }

    #[test]
    fn test_provides_index_excludes_disabled_repos() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('disabled', 'https://example.com', 0, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'pkg', '1.0', 'sha256:x', 100, 'https://example.com/x', 'rpm')",
            [repo_id],
        )
        .unwrap();
        let pkg_id = conn.last_insert_rowid();

        RepositoryProvide::new(
            pkg_id,
            "libfoo.so".to_string(),
            Some("1.0".to_string()),
            "soname".to_string(),
            None,
            VersionScheme::Rpm,
        )
        .insert(&conn)
        .unwrap();

        let index = ProvidesIndex::build(&conn).unwrap();
        assert!(index.find_providers("libfoo.so").is_empty());
    }
}
