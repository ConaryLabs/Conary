// conary-core/src/resolver/provides_index.rs

//! Pre-built index mapping capability names to provider packages.
//!
//! Modeled after libsolv's `pool_createwhatprovides()`. Built once at
//! resolution start from three data sources:
//! 1. `repository_provides` (per-distro provides from repo sync)
//! 2. `provides` (installed package provides)
//! 3. `appstream_provides` (cross-distro provides from AppStream)

use crate::error::Result;
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme, repo_version_satisfies};
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
    /// Version comparison scheme
    pub version_scheme: VersionScheme,
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
                "SELECT rp.capability, rp.version, rp.version_scheme, rp.repository_package_id
                 FROM repository_provides rp
                 JOIN repository_packages pkg ON rp.repository_package_id = pkg.id
                 JOIN repositories r ON pkg.repository_id = r.id
                 WHERE r.enabled = 1",
            )?;
            let rows = stmt.query_map([], |row| {
                let cap: String = row.get(0)?;
                let version: Option<String> = row.get(1)?;
                let scheme_str: Option<String> = row.get(2)?;
                let pkg_id: i64 = row.get(3)?;
                Ok((
                    cap,
                    ProviderEntry {
                        repo_package_id: Some(pkg_id),
                        installed_trove_id: None,
                        canonical_id: None,
                        provide_version: version,
                        version_scheme: parse_scheme(scheme_str.as_deref()),
                    },
                ))
            })?;
            for row in rows.flatten() {
                providers.entry(row.0).or_default().push(row.1);
            }
        }

        // 2. Installed provides
        {
            let mut stmt = conn.prepare(
                "SELECT p.capability, p.version, t.version_scheme, p.trove_id
                 FROM provides p
                 JOIN troves t ON p.trove_id = t.id",
            )?;
            let rows = stmt.query_map([], |row| {
                let cap: String = row.get(0)?;
                let version: Option<String> = row.get(1)?;
                let scheme_str: Option<String> = row.get(2)?;
                let trove_id: i64 = row.get(3)?;
                Ok((
                    cap,
                    ProviderEntry {
                        repo_package_id: None,
                        installed_trove_id: Some(trove_id),
                        canonical_id: None,
                        provide_version: version,
                        version_scheme: parse_scheme(scheme_str.as_deref()),
                    },
                ))
            })?;
            for row in rows.flatten() {
                providers.entry(row.0).or_default().push(row.1);
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
                        version_scheme: VersionScheme::Rpm,
                    },
                ))
            })?;
            for row in rows.flatten() {
                providers.entry(row.0).or_default().push(row.1);
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
    ) -> Vec<&ProviderEntry> {
        self.find_providers(capability)
            .iter()
            .filter(|p| match &p.provide_version {
                Some(v) => repo_version_satisfies(scheme, v, constraint),
                None => matches!(constraint, RepoVersionConstraint::Any),
            })
            .collect()
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

fn parse_scheme(s: Option<&str>) -> VersionScheme {
    match s {
        Some("debian") => VersionScheme::Debian,
        Some("arch") => VersionScheme::Arch,
        _ => VersionScheme::Rpm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

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

        conn.execute(
            "INSERT INTO repository_provides (repository_package_id, capability, version, kind, version_scheme)
             VALUES (?1, 'libssl.so.3', '3.2.0', 'library', 'rpm')",
            [pkg_id],
        )
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
            "INSERT INTO troves (name, version, type, install_source, install_reason)
             VALUES ('openssl-libs', '3.2.0', 'package', 'repository', 'explicit')",
            [],
        )
        .unwrap();
        let trove_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO provides (trove_id, capability, kind)
             VALUES (?1, 'libssl.so.3', 'library')",
            [trove_id],
        )
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
            conn.execute(
                "INSERT INTO repository_provides (repository_package_id, capability, version, kind, version_scheme)
                 VALUES (?1, 'libfoo.so', ?2, 'library', 'rpm')",
                rusqlite::params![pkg_id, version],
            )
            .unwrap();
        }

        let index = ProvidesIndex::build(&conn).unwrap();

        // All providers
        assert_eq!(index.find_providers("libfoo.so").len(), 2);

        // Only >= 3.0
        let constrained = index.find_providers_constrained(
            "libfoo.so",
            &RepoVersionConstraint::GreaterOrEqual("3.0".to_string()),
            VersionScheme::Rpm,
        );
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
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'pkg', '1.0', 'sha256:x', 100, 'https://example.com/x')",
            [repo_id],
        )
        .unwrap();
        let pkg_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_provides (repository_package_id, capability, version, kind)
             VALUES (?1, 'libfoo.so', '1.0', 'library')",
            [pkg_id],
        )
        .unwrap();

        let index = ProvidesIndex::build(&conn).unwrap();
        assert!(index.find_providers("libfoo.so").is_empty());
    }
}
