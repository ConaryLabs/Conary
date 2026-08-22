// conary-core/src/resolver/canonical.rs

//! Canonical package resolver
//!
//! Expands canonical or distro-specific package names into resolver candidates,
//! ranks them by exact pin authority, and enforces mixing policy.

use crate::db::models::{CanonicalPackage, PackageImplementation};
use crate::error::{Error, Result};
use crate::repository::resolution_policy::{
    DependencyMixingPolicy, RequestScope, ResolutionPolicy,
};
use rusqlite::Connection;
use std::cmp::Ordering;

/// A candidate package from canonical expansion
#[derive(Debug, Clone)]
pub struct ResolverCandidate {
    pub distro_name: String,
    pub distro: String,
    pub canonical_id: i64,
    /// Repository name, if available from repository_packages canonical link.
    pub repository_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct AuthorityRank {
    pinned: bool,
}

impl AuthorityRank {
    fn compare_best_first(self, other: Self) -> Ordering {
        other.pinned.cmp(&self.pinned)
    }

    fn is_tied_with(self, other: Self) -> bool {
        self.pinned == other.pinned
    }
}

/// Resolves canonical package names into distro-specific candidates
pub struct CanonicalResolver<'db> {
    conn: &'db Connection,
}

impl<'db> CanonicalResolver<'db> {
    /// Create a new canonical resolver backed by the given database connection
    pub fn new(conn: &'db Connection) -> Self {
        Self { conn }
    }

    /// Look up every enabled repository that carries an exact canonical
    /// implementation. Repository-scoped requests select from these concrete
    /// variants rather than treating a distro name as a repository alias.
    fn lookup_repo_names(&self, canonical_id: i64, distro_name: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT r.name FROM resolved_repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE rp.canonical_id = ?1 AND rp.name = ?2 AND r.enabled = 1
                 ORDER BY r.priority DESC, r.id ASC",
        )?;
        let rows = statement.query_map(rusqlite::params![canonical_id, distro_name], |row| {
            row.get(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn expand_implementations(
        &self,
        implementations: Vec<PackageImplementation>,
    ) -> Result<Vec<ResolverCandidate>> {
        let mut candidates = Vec::new();
        for implementation in implementations {
            let repository_names =
                self.lookup_repo_names(implementation.canonical_id, &implementation.distro_name)?;
            if repository_names.is_empty() {
                candidates.push(ResolverCandidate {
                    distro_name: implementation.distro_name,
                    distro: implementation.distro,
                    canonical_id: implementation.canonical_id,
                    repository_name: None,
                });
                continue;
            }
            for repository_name in repository_names {
                candidates.push(ResolverCandidate {
                    distro_name: implementation.distro_name.clone(),
                    distro: implementation.distro.clone(),
                    canonical_id: implementation.canonical_id,
                    repository_name: Some(repository_name),
                });
            }
        }
        Ok(candidates)
    }

    /// Expand a package name into all known implementation candidates.
    ///
    /// First tries the name as a canonical package name. If not found,
    /// tries as a distro-specific name and resolves to sibling implementations.
    pub fn expand(&self, name: &str) -> Result<Vec<ResolverCandidate>> {
        // Try as canonical name first
        if let Some(canonical) = CanonicalPackage::find_by_name(self.conn, name)? {
            let Some(canonical_id) = canonical.id else {
                return Ok(vec![]);
            };
            let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;
            return self.expand_implementations(impls);
        }

        // Try as distro-specific name
        if let Some(impl_entry) = PackageImplementation::find_by_any_distro_name(self.conn, name)? {
            let canonical_id = impl_entry.canonical_id;
            let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;
            return self.expand_implementations(impls);
        }

        Ok(Vec::new())
    }

    /// Rank candidates by exact source pin.
    ///
    /// Stable ordering is presentation only. Call [`Self::select_candidate_with_policy`]
    /// before mutation so an authority tie becomes an error.
    pub fn rank_candidates(
        &self,
        candidates: &[ResolverCandidate],
    ) -> Result<Vec<ResolverCandidate>> {
        Ok(self
            .rank_with_authority(candidates.to_vec(), None)?
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect())
    }

    /// Rank candidates after exact request-scope and allowlist filtering.
    ///
    /// Discovery metadata and measured installed affinity never contribute a
    /// mutation-authority rank.
    pub fn rank_candidates_with_policy(
        &self,
        candidates: &[ResolverCandidate],
        policy: &ResolutionPolicy,
    ) -> Result<Vec<ResolverCandidate>> {
        let candidates = candidates
            .iter()
            .filter(|candidate| {
                candidate_allowed_by_policy(candidate, policy)
                    && candidate_matches_request_scope(candidate, &policy.request_scope)
            })
            .cloned()
            .collect();
        Ok(self
            .rank_with_authority(candidates, policy.primary_source_identity())?
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect())
    }

    /// Select one candidate only when typed policy establishes a unique winner.
    pub fn select_candidate_with_policy(
        &self,
        candidates: &[ResolverCandidate],
        policy: &ResolutionPolicy,
    ) -> Result<Option<ResolverCandidate>> {
        let candidates = candidates
            .iter()
            .filter(|candidate| {
                candidate_allowed_by_policy(candidate, policy)
                    && candidate_matches_request_scope(candidate, &policy.request_scope)
            })
            .cloned()
            .collect();
        let ranked = self.rank_with_authority(candidates, policy.primary_source_identity())?;
        let Some((winner, winner_rank)) = ranked.first() else {
            return Ok(None);
        };
        if let Some((_, runner_up_rank)) = ranked.get(1)
            && winner_rank.is_tied_with(*runner_up_rank)
        {
            return Err(Error::AmbiguousPackageSelection {
                package: winner.distro_name.clone(),
                candidates: ranked
                    .iter()
                    .take_while(|(_, rank)| winner_rank.is_tied_with(*rank))
                    .map(|(candidate, _)| {
                        format!(
                            "{}:{}:{}",
                            candidate.distro,
                            candidate
                                .repository_name
                                .as_deref()
                                .unwrap_or("<no-repository>"),
                            candidate.distro_name
                        )
                    })
                    .collect(),
            });
        }
        Ok(Some(winner.clone()))
    }

    fn rank_with_authority(
        &self,
        candidates: Vec<ResolverCandidate>,
        preferred_source: Option<&str>,
    ) -> Result<Vec<(ResolverCandidate, AuthorityRank)>> {
        let mut ranked = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let pinned = preferred_source.is_some_and(|source| source == candidate.distro);
            ranked.push((candidate, AuthorityRank { pinned }));
        }
        ranked.sort_by(|(_, left), (_, right)| left.compare_best_first(*right));
        Ok(ranked)
    }

    /// Get packages that conflict with the given package (canonical equivalents).
    /// All distro implementations of the same canonical package conflict with each other.
    pub fn get_conflicts(&self, package_name: &str) -> Result<Vec<String>> {
        let canonical = CanonicalPackage::resolve_name(self.conn, package_name)?;
        let Some(canonical) = canonical else {
            return Ok(vec![]);
        };
        let canonical_id = canonical
            .id
            .ok_or_else(|| Error::MissingId("resolved canonical package has no id".to_string()))?;

        let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;

        Ok(impls
            .into_iter()
            .map(|i| i.distro_name)
            .filter(|name| name != package_name)
            .collect())
    }
}

fn candidate_allowed_by_policy(candidate: &ResolverCandidate, policy: &ResolutionPolicy) -> bool {
    policy.mixing != DependencyMixingPolicy::Strict
        || policy
            .primary_source_identity()
            .is_none_or(|source| source == candidate.distro)
}

fn candidate_matches_request_scope(candidate: &ResolverCandidate, scope: &RequestScope) -> bool {
    match scope {
        RequestScope::Any => true,
        RequestScope::Repository(repository) => {
            candidate.repository_name.as_deref() == Some(repository)
        }
        RequestScope::SourceIdentity(source_identity) => &candidate.distro == source_identity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CanonicalMappingAuthority, CanonicalPackage, PackageImplementation};
    use crate::db::testing::create_test_db;
    use crate::repository::resolution_policy::{
        DependencyMixingPolicy, RequestScope, ResolutionPolicy,
    };

    #[test]
    fn test_expand_canonical_name() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "httpd".into(),
            CanonicalMappingAuthority::Contract,
        );
        i1.insert_or_verify(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-26.04".into(),
            "apache2".into(),
            CanonicalMappingAuthority::Contract,
        );
        i2.insert_or_verify(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("apache-httpd").unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|c| c.distro_name == "httpd"));
        assert!(candidates.iter().any(|c| c.distro_name == "apache2"));
    }

    #[test]
    fn test_expand_distro_name_resolves() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "httpd".into(),
            CanonicalMappingAuthority::Contract,
        );
        i1.insert_or_verify(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-26.04".into(),
            "apache2".into(),
            CanonicalMappingAuthority::Contract,
        );
        i2.insert_or_verify(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("httpd").unwrap();
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn test_rank_uses_explicit_source_identity() {
        let (_t, conn) = create_test_db();

        let candidates = vec![
            ResolverCandidate {
                distro_name: "httpd".into(),
                distro: "fedora-44".into(),
                canonical_id: 1,
                repository_name: None,
            },
            ResolverCandidate {
                distro_name: "apache2".into(),
                distro: "ubuntu-26.04".into(),
                canonical_id: 1,
                repository_name: None,
            },
        ];

        let resolver = CanonicalResolver::new(&conn);
        let policy = ResolutionPolicy::new()
            .with_mixing(DependencyMixingPolicy::Guarded)
            .with_primary_source_identity("ubuntu-26.04");
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(ranked[0].distro, "ubuntu-26.04");
    }

    #[test]
    fn measured_affinity_cannot_break_a_mutation_authority_tie() {
        let (_t, conn) = create_test_db();
        conn.execute(
            "INSERT INTO system_affinity (source_identity, package_count, percentage, updated_at) VALUES ('ubuntu-26.04', 80, 80.0, '2026-03-05')",
            [],
        )
        .unwrap();

        let candidates = vec![
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "fedora-44".into(),
                canonical_id: 1,
                repository_name: None,
            },
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "ubuntu-26.04".into(),
                canonical_id: 1,
                repository_name: None,
            },
        ];

        let resolver = CanonicalResolver::new(&conn);
        let error = resolver
            .select_candidate_with_policy(&candidates, &ResolutionPolicy::new())
            .unwrap_err();
        assert!(matches!(error, Error::AmbiguousPackageSelection { .. }));
    }

    #[test]
    fn test_canonical_equivalents_conflict() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "httpd".into(),
            CanonicalMappingAuthority::Contract,
        );
        i1.insert_or_verify(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-26.04".into(),
            "apache2".into(),
            CanonicalMappingAuthority::Contract,
        );
        i2.insert_or_verify(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let conflicts = resolver.get_conflicts("httpd").unwrap();
        assert!(conflicts.contains(&"apache2".to_string()));
    }

    #[test]
    fn test_no_conflict_for_different_canonicals() {
        let (_t, conn) = create_test_db();
        let mut c1 = CanonicalPackage::new("curl".into(), "package".into());
        c1.insert(&conn).unwrap();
        let mut c2 = CanonicalPackage::new("wget".into(), "package".into());
        c2.insert(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let conflicts = resolver.get_conflicts("curl").unwrap();
        assert!(!conflicts.contains(&"wget".to_string()));
    }

    #[test]
    fn test_unknown_package_no_conflicts() {
        let (_t, conn) = create_test_db();
        let resolver = CanonicalResolver::new(&conn);
        let conflicts = resolver.get_conflicts("nonexistent").unwrap();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_rank_with_policy_scope_repo() {
        let (_t, conn) = create_test_db();
        let resolver = CanonicalResolver::new(&conn);
        let candidates = vec![
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "fedora-44".into(),
                canonical_id: 1,
                repository_name: Some("fedora-base".into()),
            },
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "ubuntu-26.04".into(),
                canonical_id: 1,
                repository_name: Some("ubuntu-main".into()),
            },
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "ubuntu-main".into(),
                canonical_id: 1,
                repository_name: None,
            },
        ];

        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("ubuntu-main".into()));
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].distro, "ubuntu-26.04");
    }

    #[test]
    fn test_rank_with_exact_profile_scope() {
        let (_t, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("curl".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "curl".into(),
            CanonicalMappingAuthority::Contract,
        );
        i1.insert_or_verify(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-26.04".into(),
            "curl".into(),
            CanonicalMappingAuthority::Contract,
        );
        i2.insert_or_verify(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("curl").unwrap();

        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::SourceIdentity("ubuntu-26.04".into()));
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(ranked[0].distro, "ubuntu-26.04");
    }

    #[test]
    fn repology_status_cannot_break_a_mutation_authority_tie() {
        let (_t, conn) = create_test_db();
        let fresh = chrono::Utc::now().to_rfc3339();

        let mut pkg = CanonicalPackage::new("python".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut fedora_impl = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "python3".into(),
            CanonicalMappingAuthority::Contract,
        );
        fedora_impl.insert_or_verify(&conn).unwrap();
        let mut arch_impl = PackageImplementation::new(
            cid,
            "arch".into(),
            "python".into(),
            CanonicalMappingAuthority::Contract,
        );
        arch_impl.insert_or_verify(&conn).unwrap();

        crate::db::models::RepologyCacheEntry::insert_or_replace(
            &conn,
            &crate::db::models::RepologyCacheEntry {
                project_name: "python".into(),
                distro: "fedora-44".into(),
                distro_name: "python3".into(),
                version: Some("3.12.0".into()),
                status: Some("outdated".into()),
                fetched_at: fresh.clone(),
            },
        )
        .unwrap();
        crate::db::models::RepologyCacheEntry::insert_or_replace(
            &conn,
            &crate::db::models::RepologyCacheEntry {
                project_name: "python".into(),
                distro: "arch".into(),
                distro_name: "python".into(),
                version: Some("3.13.0".into()),
                status: Some("newest".into()),
                fetched_at: fresh.clone(),
            },
        )
        .unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("python").unwrap();
        let error = resolver
            .select_candidate_with_policy(&candidates, &ResolutionPolicy::new())
            .unwrap_err();
        assert!(matches!(error, Error::AmbiguousPackageSelection { .. }));
    }

    #[test]
    fn exact_source_scope_can_resolve_otherwise_ambiguous_candidates() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("python".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut fedora_impl = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "python3".into(),
            CanonicalMappingAuthority::Contract,
        );
        fedora_impl.insert_or_verify(&conn).unwrap();
        let mut arch_impl = PackageImplementation::new(
            cid,
            "arch".into(),
            "python".into(),
            CanonicalMappingAuthority::Contract,
        );
        arch_impl.insert_or_verify(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("python").unwrap();
        let policy = ResolutionPolicy::new()
            .with_scope(RequestScope::SourceIdentity("fedora-44".to_string()))
            .with_primary_source_identity("fedora-44");
        let selected = resolver
            .select_candidate_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(
            selected.map(|candidate| candidate.distro),
            Some("fedora-44".into())
        );
    }

    #[test]
    fn expansion_preserves_exact_repository_variants() {
        let (_t, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("httpd-web".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();

        // Two repos for the same distro, different priorities
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('fedora-base', 'https://base.com', 1, 10, 'fedora-44')",
            [],
        )
        .unwrap();
        let base_repo = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('fedora-updates', 'https://updates.com', 1, 20, 'fedora-44')",
            [],
        )
        .unwrap();
        let updates_repo = conn.last_insert_rowid();

        // Same package in both repos, both linked to same canonical
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'httpd', '2.4.58', 'sha256:a', 100, 'https://base.com/httpd', 'rpm', ?2)",
            rusqlite::params![base_repo, cid],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'httpd', '2.4.59', 'sha256:b', 100, 'https://updates.com/httpd', 'rpm', ?2)",
            rusqlite::params![updates_repo, cid],
        )
        .unwrap();

        // Create the implementation so expand() finds it
        let mut impl1 = PackageImplementation::new(
            cid,
            "fedora-44".into(),
            "httpd".into(),
            CanonicalMappingAuthority::Contract,
        );
        impl1.insert_or_verify(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("httpd").unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[0].repository_name.as_deref(),
            Some("fedora-updates")
        );
        assert_eq!(
            candidates[1].repository_name.as_deref(),
            Some("fedora-base")
        );

        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("fedora-base".into()));
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].repository_name.as_deref(), Some("fedora-base"));
    }
}
