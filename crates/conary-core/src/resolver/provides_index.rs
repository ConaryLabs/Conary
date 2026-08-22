// conary-core/src/resolver/provides_index.rs

//! Demand-driven index mapping capability names to provider packages.
//!
//! Modeled after libsolv's `pool_createwhatprovides()`. Populated on demand
//! during pre-solve candidate loading from three data sources:
//! 1. `repository_provides` (per-distro provides from repo sync)
//! 2. `provides` (installed package provides)
//! 3. `appstream_provides` (cross-distro provides from AppStream)

use crate::db::models::{ProvideEntry, RepositoryProvide};
use crate::error::Result;
use crate::repository::dependency_model::ProvideVersionRelation;
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, provided_range_matches_requirement,
};
use rusqlite::Connection;
use std::collections::HashMap;

/// A single provider entry in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Demand-driven capability-to-provider index.
///
/// The resolver asks for exact capability names while it loads a bounded
/// candidate universe. Keep the database handle so the first lookup can seek
/// each source's capability index, then cache only the capabilities this
/// resolution actually names. The cache is populated before `resolvo` starts;
/// SAT callbacks continue to use the provider's in-memory snapshot.
pub struct ProvidesIndex<'db> {
    conn: &'db Connection,
    providers: HashMap<String, Vec<ProviderEntry>>,
}

impl<'db> ProvidesIndex<'db> {
    /// Create an empty demand-driven index backed by the database connection
    /// used for this resolution.
    ///
    /// No provider rows are read here. Each later lookup is an exact
    /// capability seek and is cached for the lifetime of this resolution.
    pub fn build(conn: &'db Connection) -> Result<Self> {
        Ok(Self {
            conn,
            providers: HashMap::new(),
        })
    }

    fn load_capability(&self, capability: &str) -> Result<Vec<ProviderEntry>> {
        let mut providers = Vec::new();

        // 1. Repository provides. RepositoryProvide::find_by_capability uses
        // idx_repository_provides_capability and filters disabled repositories.
        for provide in RepositoryProvide::find_by_capability(self.conn, capability)? {
            providers.push(ProviderEntry {
                repo_package_id: Some(provide.repository_package_id),
                installed_trove_id: None,
                canonical_id: None,
                provide_version: provide.version,
                version_relation: provide.version_relation,
                version_scheme: Some(provide.version_scheme),
            });
        }

        // 2. Installed provides. The installed capability index is also
        // queried by the exact key. Unlike the former whole-table build this
        // does not join `troves`: `provides.trove_id` is NOT NULL with ON
        // DELETE CASCADE and every connection opens with foreign_keys=ON
        // (db/mod.rs open pragmas), so a provides row without its trove
        // cannot exist and the join carried no filter.
        for provide in ProvideEntry::find_all_by_capability(self.conn, capability)? {
            providers.push(ProviderEntry {
                repo_package_id: None,
                installed_trove_id: Some(provide.trove_id),
                canonical_id: None,
                provide_version: provide.version,
                version_relation: provide.version_relation,
                version_scheme: Some(provide.version_scheme),
            });
        }

        // 3. AppStream cross-distro provides. This table has its own exact
        // capability index and is intentionally bounded by the same key.
        let mut stmt = self.conn.prepare(
            "SELECT ap.capability, ap.canonical_id
             FROM appstream_provides ap
             WHERE ap.capability = ?1
             ORDER BY ap.id",
        )?;
        let rows = stmt.query_map([capability], |row| {
            let capability: String = row.get(0)?;
            let canonical_id: i64 = row.get(1)?;
            Ok((capability, canonical_id))
        })?;
        for row in rows {
            // The selected capability is part of the row for parity with the
            // eager implementation; the cache key owns the grouping.
            let (_capability, canonical_id) = row?;
            providers.push(ProviderEntry {
                repo_package_id: None,
                installed_trove_id: None,
                canonical_id: Some(canonical_id),
                provide_version: None,
                version_relation: None,
                version_scheme: None,
            });
        }

        Ok(providers)
    }

    fn ensure_loaded(&mut self, capability: &str) -> Result<()> {
        if !self.providers.contains_key(capability) {
            let providers = self.load_capability(capability)?;
            self.providers.insert(capability.to_string(), providers);
        }
        Ok(())
    }

    /// Find all providers for a capability name.
    pub fn find_providers(&mut self, capability: &str) -> Result<Vec<ProviderEntry>> {
        self.ensure_loaded(capability)?;
        Ok(self.providers.get(capability).cloned().unwrap_or_default())
    }

    /// Find providers whose version satisfies a constraint.
    pub fn find_providers_constrained(
        &mut self,
        capability: &str,
        constraint: &RepoVersionConstraint,
        scheme: VersionScheme,
    ) -> Result<Vec<ProviderEntry>> {
        let mut matches = Vec::new();
        for provider in self.find_providers(capability)? {
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

    /// Total number of unique capabilities indexed with at least one
    /// provider. Keys cached as empty by a negative lookup are not counted,
    /// matching what the former eager build would have contained.
    pub fn capability_count(&self) -> usize {
        self.providers
            .values()
            .filter(|entries| !entries.is_empty())
            .count()
    }

    /// Total number of provider entries across all capabilities.
    pub fn provider_count(&self) -> usize {
        self.providers.values().map(|v| v.len()).sum()
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

    /// Test-only copy of the former whole-table build. It is deliberately
    /// retained as an oracle so the demand-driven lookup can be compared as a
    /// property over every capability in a fixture, rather than only against
    /// one hand-picked row.
    fn legacy_eager_build(conn: &Connection) -> Result<HashMap<String, Vec<ProviderEntry>>> {
        let mut providers: HashMap<String, Vec<ProviderEntry>> = HashMap::new();

        let mut stmt = conn.prepare(
            "SELECT rp.capability, rp.version, rp.version_relation, rp.version_scheme,
                    rp.repository_package_id
             FROM resolved_repository_provides rp
             JOIN resolved_repository_packages pkg ON rp.repository_package_id = pkg.id
             JOIN repositories r ON pkg.repository_id = r.id
             WHERE r.enabled = 1",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (capability, version, relation, scheme, pkg_id) in rows {
            providers
                .entry(capability)
                .or_default()
                .push(ProviderEntry {
                    repo_package_id: Some(pkg_id),
                    installed_trove_id: None,
                    canonical_id: None,
                    provide_version: version.clone(),
                    version_relation: legacy_relation(relation, &version)?,
                    version_scheme: Some(legacy_scheme(&scheme)?),
                });
        }

        let mut stmt = conn.prepare(
            "SELECT p.capability, p.version, p.version_relation, p.version_scheme, p.trove_id
             FROM provides p
             JOIN troves t ON p.trove_id = t.id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (capability, version, relation, scheme, trove_id) in rows {
            providers
                .entry(capability)
                .or_default()
                .push(ProviderEntry {
                    repo_package_id: None,
                    installed_trove_id: Some(trove_id),
                    canonical_id: None,
                    provide_version: version.clone(),
                    version_relation: legacy_relation(relation, &version)?,
                    version_scheme: Some(legacy_scheme(&scheme)?),
                });
        }

        let mut stmt = conn.prepare("SELECT capability, canonical_id FROM appstream_provides")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (capability, canonical_id) in rows {
            providers
                .entry(capability)
                .or_default()
                .push(ProviderEntry {
                    repo_package_id: None,
                    installed_trove_id: None,
                    canonical_id: Some(canonical_id),
                    provide_version: None,
                    version_relation: None,
                    version_scheme: None,
                });
        }

        Ok(providers)
    }

    fn legacy_relation(
        relation: Option<String>,
        version: &Option<String>,
    ) -> Result<Option<ProvideVersionRelation>> {
        match (relation, version) {
            (None, None) => Ok(None),
            (Some(relation), Some(_)) => ProvideVersionRelation::parse_exact(&relation)
                .map(Some)
                .map_err(|error| crate::error::Error::ConfigError(error.to_string())),
            _ => Err(crate::error::Error::ConfigError(
                "provider range relation and version must be paired".to_string(),
            )),
        }
    }

    fn legacy_scheme(value: &str) -> Result<VersionScheme> {
        value
            .parse()
            .map_err(|error: String| crate::error::Error::ConfigError(error))
    }

    fn sorted_entries(mut entries: Vec<ProviderEntry>) -> Vec<ProviderEntry> {
        entries.sort_by(|left, right| {
            left.repo_package_id
                .cmp(&right.repo_package_id)
                .then_with(|| left.installed_trove_id.cmp(&right.installed_trove_id))
                .then_with(|| left.canonical_id.cmp(&right.canonical_id))
                .then_with(|| left.provide_version.cmp(&right.provide_version))
                .then_with(|| left.version_relation.cmp(&right.version_relation))
                .then_with(|| {
                    left.version_scheme
                        .map(VersionScheme::as_str)
                        .cmp(&right.version_scheme.map(VersionScheme::as_str))
                })
        });
        entries
    }

    fn measurement_fixture(row_count: usize) -> (tempfile::NamedTempFile, Connection, Vec<String>) {
        let (temp, mut conn) = create_test_db();
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('measurement-repo', 'https://example.com/measurement', 1, 10)",
            [],
        )
        .unwrap();
        let repository_id = conn.last_insert_rowid();
        let transaction = conn.transaction().unwrap();
        let mut provides = Vec::with_capacity(row_count);
        for index in 0..row_count {
            transaction
                .execute(
                    "INSERT INTO repository_packages
                     (repository_id, name, version, checksum, size, download_url, version_scheme)
                     VALUES (?1, ?2, '1.0', ?3, 100, 'https://example.com/pkg', 'rpm')",
                    rusqlite::params![
                        repository_id,
                        format!("measurement-package-{index}"),
                        format!("sha256:measurement-{index}")
                    ],
                )
                .unwrap();
            let package_id = transaction.last_insert_rowid();
            provides.push(RepositoryProvide::new(
                package_id,
                format!("measurement-capability-{index}"),
                Some("1.0".to_string()),
                "file".to_string(),
                None,
                VersionScheme::Rpm,
            ));
        }
        transaction.commit().unwrap();
        RepositoryProvide::batch_insert(&conn, &provides).unwrap();
        drop(provides);

        let lookup_capabilities = [0, row_count / 2, row_count.saturating_sub(1)]
            .into_iter()
            .map(|index| format!("measurement-capability-{index}"))
            .collect();
        (temp, conn, lookup_capabilities)
    }

    fn peak_rss_kib() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|line| {
            let value = line.strip_prefix("VmHWM:")?.split_whitespace().next()?;
            value.parse().ok()
        })
    }

    /// Run manually in separate processes for `/usr/bin/time -v` comparison.
    ///
    /// Example modes are `legacy` and `demand`; the fixture size is selected
    /// with `CONARY_PROVIDES_INDEX_MEASUREMENT_ROWS` and is reported by the
    /// test. This is intentionally ignored so normal resolver tests do not
    /// pay for a large synthetic database.
    #[test]
    #[ignore]
    fn measure_provides_index_fixture() {
        let row_count = std::env::var("CONARY_PROVIDES_INDEX_MEASUREMENT_ROWS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(50_000);
        let mode = std::env::var("CONARY_PROVIDES_INDEX_MEASUREMENT_MODE")
            .unwrap_or_else(|_| "demand".to_string());
        let (_temp, conn, lookup_capabilities) = measurement_fixture(row_count);

        let build_start = std::time::Instant::now();
        match mode.as_str() {
            "legacy" => {
                let index = legacy_eager_build(&conn).unwrap();
                let build_elapsed = build_start.elapsed();
                let lookup_start = std::time::Instant::now();
                let lookup_count = lookup_capabilities
                    .iter()
                    .map(|capability| index.get(capability).map_or(0, Vec::len))
                    .sum::<usize>();
                let lookup_elapsed = lookup_start.elapsed();
                println!(
                    "provides_index_measurement mode=legacy rows={row_count} capabilities={} providers={lookup_count} build_ms={} lookup_ms={} peak_rss_kib={:?}",
                    index.len(),
                    build_elapsed.as_secs_f64() * 1000.0,
                    lookup_elapsed.as_secs_f64() * 1000.0,
                    peak_rss_kib(),
                );
            }
            "demand" => {
                let mut index = ProvidesIndex::build(&conn).unwrap();
                let build_elapsed = build_start.elapsed();
                let lookup_start = std::time::Instant::now();
                let lookup_count = lookup_capabilities
                    .iter()
                    .map(|capability| index.find_providers(capability).unwrap().len())
                    .sum::<usize>();
                let lookup_elapsed = lookup_start.elapsed();
                println!(
                    "provides_index_measurement mode=demand rows={row_count} capabilities={} providers={lookup_count} build_ms={} lookup_ms={} peak_rss_kib={:?}",
                    index.capability_count(),
                    build_elapsed.as_secs_f64() * 1000.0,
                    lookup_elapsed.as_secs_f64() * 1000.0,
                    peak_rss_kib(),
                );
            }
            other => panic!("unsupported measurement mode {other:?}"),
        }
    }

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

        let mut index = ProvidesIndex::build(&conn).unwrap();
        let providers = index.find_providers("libssl.so.3").unwrap();
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

        let mut index = ProvidesIndex::build(&conn).unwrap();
        let providers = index.find_providers("libssl.so.3").unwrap();
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

        let mut index = ProvidesIndex::build(&conn).unwrap();
        let providers = index.find_providers("libssl.so.3").unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].canonical_id, Some(canonical_id));
    }

    #[test]
    fn test_provides_index_empty_for_unknown() {
        let (_temp, conn) = create_test_db();
        let mut index = ProvidesIndex::build(&conn).unwrap();
        assert!(index.find_providers("nonexistent.so.1").unwrap().is_empty());
        assert_eq!(index.capability_count(), 0);
        assert_eq!(index.provider_count(), 0);
    }

    #[test]
    fn demand_driven_lookup_matches_legacy_eager_lookup_for_fixture() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fixture-enabled', 'https://example.com/enabled', 1, 10)",
            [],
        )
        .unwrap();
        let enabled_repo_id = conn.last_insert_rowid();
        for index in 0..32 {
            conn.execute(
                "INSERT INTO repository_packages
                 (repository_id, name, version, checksum, size, download_url, version_scheme)
                 VALUES (?1, ?2, '1.0', ?3, 100, 'https://example.com/pkg', 'rpm')",
                rusqlite::params![
                    enabled_repo_id,
                    format!("fixture-package-{index}"),
                    format!("sha256:fixture-{index}")
                ],
            )
            .unwrap();
            let package_id = conn.last_insert_rowid();
            let capability = if index == 0 {
                "fixture-shared-capability".to_string()
            } else {
                format!("fixture-capability-{index}")
            };
            RepositoryProvide::new(
                package_id,
                capability,
                Some("1.0".to_string()),
                "file".to_string(),
                None,
                VersionScheme::Rpm,
            )
            .insert(&conn)
            .unwrap();
        }

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fixture-disabled', 'https://example.com/disabled', 0, 10)",
            [],
        )
        .unwrap();
        let disabled_repo_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'disabled-package', '1.0', 'sha256:disabled', 100,
                     'https://example.com/disabled', 'rpm')",
            [disabled_repo_id],
        )
        .unwrap();
        let disabled_package_id = conn.last_insert_rowid();
        RepositoryProvide::new(
            disabled_package_id,
            "disabled-capability".to_string(),
            None,
            "file".to_string(),
            None,
            VersionScheme::Rpm,
        )
        .insert(&conn)
        .unwrap();

        conn.execute(
            "INSERT INTO troves (name, version, type, install_source, install_reason, version_scheme)
             VALUES ('fixture-installed', '2.0', 'package', 'repository', 'explicit', 'rpm')",
            [],
        )
        .unwrap();
        let trove_id = conn.last_insert_rowid();
        ProvideEntry::new_typed(
            trove_id,
            RepositoryCapabilityKind::Virtual,
            "fixture-shared-capability".to_string(),
            Some("2.0".to_string()),
            VersionScheme::Rpm,
            ProvideArchitectureQualifier::Implicit,
        )
        .insert(&conn)
        .unwrap();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('fixture-canonical', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
             VALUES (?1, 'library', 'fixture-shared-capability')",
            [canonical_id],
        )
        .unwrap();

        let legacy = legacy_eager_build(&conn).unwrap();
        let mut demand = ProvidesIndex::build(&conn).unwrap();
        assert_eq!(demand.capability_count(), 0);
        assert_eq!(demand.provider_count(), 0);

        let mut capabilities = legacy.keys().cloned().collect::<Vec<_>>();
        capabilities.push("unknown-fixture-capability".to_string());
        for capability in capabilities {
            let expected = sorted_entries(legacy.get(&capability).cloned().unwrap_or_default());
            let actual = sorted_entries(demand.find_providers(&capability).unwrap());
            assert_eq!(actual, expected, "lookup mismatch for {capability}");
        }
        assert_eq!(demand.capability_count(), legacy.len());
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

        let mut index = ProvidesIndex::build(&conn).unwrap();

        // All providers
        assert_eq!(index.find_providers("libfoo.so").unwrap().len(), 2);

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

        let mut index = ProvidesIndex::build(&conn).unwrap();
        assert!(index.find_providers("libfoo.so").unwrap().is_empty());
    }
}
