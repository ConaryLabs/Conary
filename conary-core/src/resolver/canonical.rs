// conary-core/src/resolver/canonical.rs

//! Canonical package resolver
//!
//! Expands canonical or distro-specific package names into resolver candidates,
//! ranks them by pin/affinity, and enforces mixing policy.

use crate::db::models::{
    CanonicalPackage, DistroPin, PackageImplementation, PackageOverride, SystemAffinity,
};
use crate::error::{Error, Result};
use crate::repository::resolution_policy::{RequestScope, ResolutionPolicy};
use rusqlite::Connection;

/// A candidate package from canonical expansion
#[derive(Debug, Clone)]
pub struct ResolverCandidate {
    pub distro_name: String,
    pub distro: String,
    pub canonical_id: i64,
    /// Repository name, if available from repository_packages canonical link.
    pub repository_name: Option<String>,
}

/// Result of a mixing policy check
#[derive(Debug)]
pub struct MixingResult {
    pub allowed: bool,
    pub warning: Option<String>,
}

impl MixingResult {
    /// Returns true if this result carries a warning
    pub fn has_warning(&self) -> bool {
        self.warning.is_some()
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

    /// Look up repository name for a canonical package implementation.
    fn lookup_repo_name(&self, canonical_id: i64, distro_name: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT r.name FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE rp.canonical_id = ?1 AND rp.name = ?2 AND r.enabled = 1
                 ORDER BY r.priority DESC, r.id ASC
                 LIMIT 1",
                rusqlite::params![canonical_id, distro_name],
                |row| row.get(0),
            )
            .ok()
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
            return Ok(impls
                .into_iter()
                .map(|i| {
                    let repo_name = self.lookup_repo_name(i.canonical_id, &i.distro_name);
                    ResolverCandidate {
                        distro_name: i.distro_name,
                        distro: i.distro,
                        canonical_id: i.canonical_id,
                        repository_name: repo_name,
                    }
                })
                .collect());
        }

        // Try as distro-specific name
        if let Some(impl_entry) = PackageImplementation::find_by_any_distro_name(self.conn, name)? {
            let canonical_id = impl_entry.canonical_id;
            let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;
            return Ok(impls
                .into_iter()
                .map(|i| {
                    let repo_name = self.lookup_repo_name(i.canonical_id, &i.distro_name);
                    ResolverCandidate {
                        distro_name: i.distro_name,
                        distro: i.distro,
                        canonical_id: i.canonical_id,
                        repository_name: repo_name,
                    }
                })
                .collect());
        }

        Ok(Vec::new())
    }

    /// Rank candidates by pin preference, system affinity, then alphabetical distro.
    pub fn rank_candidates(
        &self,
        candidates: &[ResolverCandidate],
    ) -> Result<Vec<ResolverCandidate>> {
        let mut ranked = candidates.to_vec();

        let pin = DistroPin::get_current(self.conn)?;
        let affinities = SystemAffinity::list(self.conn)?;

        ranked.sort_by(|a, b| {
            // 1. Pinned distro first
            if let Some(ref p) = pin {
                let a_pinned = a.distro == p.distro;
                let b_pinned = b.distro == p.distro;
                if a_pinned != b_pinned {
                    return b_pinned.cmp(&a_pinned);
                }
            }

            // 2. Highest affinity percentage
            let a_affinity = affinities
                .iter()
                .find(|af| af.distro == a.distro)
                .map_or(0.0, |af| af.percentage);
            let b_affinity = affinities
                .iter()
                .find(|af| af.distro == b.distro)
                .map_or(0.0, |af| af.percentage);
            if (a_affinity - b_affinity).abs() > f64::EPSILON {
                return b_affinity
                    .partial_cmp(&a_affinity)
                    .unwrap_or(std::cmp::Ordering::Equal);
            }

            // 3. Alphabetical distro as tiebreaker
            a.distro.cmp(&b.distro)
        });

        Ok(ranked)
    }

    /// Check whether installing from a given distro is allowed under the current mixing policy.
    ///
    /// Returns `Err` if the policy is `strict` and the distro does not match the pin.
    /// Returns `Ok(MixingResult)` with a warning for `guarded` policy mismatches.
    pub fn check_mixing_policy(&self, candidate_distro: &str) -> Result<MixingResult> {
        let pin = DistroPin::get_current(self.conn)?;

        let Some(pin) = pin else {
            return Ok(MixingResult {
                allowed: true,
                warning: None,
            });
        };

        if candidate_distro == pin.distro {
            return Ok(MixingResult {
                allowed: true,
                warning: None,
            });
        }

        match pin.mixing_policy.as_str() {
            "strict" => Err(Error::ResolutionError(format!(
                "strict pin to '{}' forbids packages from '{candidate_distro}'",
                pin.distro,
            ))),
            "guarded" => Ok(MixingResult {
                allowed: true,
                warning: Some(format!(
                    "system is pinned to '{}'; installing from '{candidate_distro}' requires override",
                    pin.distro,
                )),
            }),
            _ => Ok(MixingResult {
                allowed: true,
                warning: None,
            }),
        }
    }

    /// Rank candidates with explicit request scope applied first.
    ///
    /// When `policy` carries a `RequestScope::Repository` or
    /// `RequestScope::DistroFlavor`, candidates matching the scope sort before
    /// all others.  The remaining tie-breaking follows the same
    /// override > pin > affinity > alphabetical order as `rank_candidates`.
    pub fn rank_candidates_with_policy(
        &self,
        candidates: &[ResolverCandidate],
        policy: &ResolutionPolicy,
    ) -> Result<Vec<ResolverCandidate>> {
        let mut ranked = candidates.to_vec();

        let pin = DistroPin::get_current(self.conn)?;
        let affinities = SystemAffinity::list(self.conn)?;

        ranked.sort_by(|a, b| {
            // 0. Explicit request scope first (root requests only)
            match &policy.request_scope {
                RequestScope::Repository(repo) => {
                    // Use repository_name when available for exact repo scoping,
                    // fall back to distro identity when repo name isn't known.
                    let a_match = a
                        .repository_name
                        .as_deref()
                        .map_or(a.distro == repo.as_str(), |rn| rn == repo.as_str());
                    let b_match = b
                        .repository_name
                        .as_deref()
                        .map_or(b.distro == repo.as_str(), |rn| rn == repo.as_str());
                    if a_match != b_match {
                        return b_match.cmp(&a_match);
                    }
                }
                RequestScope::DistroFlavor(flavor) => {
                    let a_match = distro_matches_flavor(&a.distro, *flavor);
                    let b_match = distro_matches_flavor(&b.distro, *flavor);
                    if a_match != b_match {
                        return b_match.cmp(&a_match);
                    }
                }
                RequestScope::Any => {}
            }

            // 1. Package override takes priority
            let a_override = PackageOverride::get(self.conn, a.canonical_id)
                .ok()
                .flatten()
                .is_some_and(|o| o.from_distro == a.distro);
            let b_override = PackageOverride::get(self.conn, b.canonical_id)
                .ok()
                .flatten()
                .is_some_and(|o| o.from_distro == b.distro);
            if a_override != b_override {
                return b_override.cmp(&a_override);
            }

            // 2. Pinned distro
            if let Some(ref p) = pin {
                let a_pinned = a.distro == p.distro;
                let b_pinned = b.distro == p.distro;
                if a_pinned != b_pinned {
                    return b_pinned.cmp(&a_pinned);
                }
            }

            // 3. Highest affinity percentage
            let a_affinity = affinities
                .iter()
                .find(|af| af.distro == a.distro)
                .map_or(0.0, |af| af.percentage);
            let b_affinity = affinities
                .iter()
                .find(|af| af.distro == b.distro)
                .map_or(0.0, |af| af.percentage);
            if (a_affinity - b_affinity).abs() > f64::EPSILON {
                return b_affinity
                    .partial_cmp(&a_affinity)
                    .unwrap_or(std::cmp::Ordering::Equal);
            }

            // 4. Alphabetical distro as tiebreaker
            a.distro.cmp(&b.distro)
        });

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

    /// Get the distro override for a canonical package, if one exists.
    pub fn get_override(&self, canonical_id: i64) -> Result<Option<String>> {
        let ovr = PackageOverride::get(self.conn, canonical_id)?;
        Ok(ovr.map(|o| o.from_distro))
    }
}

/// Check whether a distro identifier matches a `RepositoryDependencyFlavor`.
fn distro_matches_flavor(
    distro: &str,
    flavor: crate::repository::dependency_model::RepositoryDependencyFlavor,
) -> bool {
    use crate::repository::dependency_model::RepositoryDependencyFlavor;
    let d = distro.to_lowercase();
    match flavor {
        RepositoryDependencyFlavor::Rpm => {
            d.contains("fedora") || d.contains("rhel") || d.contains("centos") || d.contains("suse")
        }
        RepositoryDependencyFlavor::Deb => {
            d.contains("ubuntu") || d.contains("debian") || d.contains("mint")
        }
        RepositoryDependencyFlavor::Arch => d.contains("arch") || d.contains("manjaro"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CanonicalPackage, DistroPin, PackageImplementation, PackageOverride};
    use crate::db::testing::create_test_db;
    use crate::repository::dependency_model::RepositoryDependencyFlavor;
    use crate::repository::resolution_policy::{RequestScope, ResolutionPolicy};

    #[test]
    fn test_expand_canonical_name() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 =
            PackageImplementation::new(cid, "fedora-41".into(), "httpd".into(), "curated".into());
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-noble".into(),
            "apache2".into(),
            "curated".into(),
        );
        i2.insert_or_ignore(&conn).unwrap();

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
        let mut i1 =
            PackageImplementation::new(cid, "fedora-41".into(), "httpd".into(), "curated".into());
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-noble".into(),
            "apache2".into(),
            "curated".into(),
        );
        i2.insert_or_ignore(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("httpd").unwrap();
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn test_rank_pinned() {
        let (_t, conn) = create_test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

        let candidates = vec![
            ResolverCandidate {
                distro_name: "httpd".into(),
                distro: "fedora-41".into(),
                canonical_id: 1,
                repository_name: None,
            },
            ResolverCandidate {
                distro_name: "apache2".into(),
                distro: "ubuntu-noble".into(),
                canonical_id: 1,
                repository_name: None,
            },
        ];

        let resolver = CanonicalResolver::new(&conn);
        let ranked = resolver.rank_candidates(&candidates).unwrap();
        assert_eq!(ranked[0].distro, "ubuntu-noble");
    }

    #[test]
    fn test_rank_affinity() {
        let (_t, conn) = create_test_db();
        conn.execute(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at) VALUES ('ubuntu-noble', 80, 80.0, '2026-03-05')",
            [],
        )
        .unwrap();

        let candidates = vec![
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "fedora-41".into(),
                canonical_id: 1,
                repository_name: None,
            },
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "ubuntu-noble".into(),
                canonical_id: 1,
                repository_name: None,
            },
        ];

        let resolver = CanonicalResolver::new(&conn);
        let ranked = resolver.rank_candidates(&candidates).unwrap();
        assert_eq!(ranked[0].distro, "ubuntu-noble");
    }

    #[test]
    fn test_strict_rejects() {
        let (_t, conn) = create_test_db();
        DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();
        let resolver = CanonicalResolver::new(&conn);
        let result = resolver.check_mixing_policy("fedora-41");
        assert!(result.is_err());
    }

    #[test]
    fn test_guarded_warns() {
        let (_t, conn) = create_test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();
        let resolver = CanonicalResolver::new(&conn);
        let result = resolver.check_mixing_policy("fedora-41").unwrap();
        assert!(result.has_warning());
    }

    #[test]
    fn test_override() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("mesa".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        PackageOverride::set(&conn, cid, "fedora-41", None).unwrap();
        let resolver = CanonicalResolver::new(&conn);
        assert_eq!(
            resolver.get_override(cid).unwrap().as_deref(),
            Some("fedora-41")
        );
    }

    #[test]
    fn test_canonical_equivalents_conflict() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 =
            PackageImplementation::new(cid, "fedora-41".into(), "httpd".into(), "curated".into());
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 = PackageImplementation::new(
            cid,
            "ubuntu-noble".into(),
            "apache2".into(),
            "curated".into(),
        );
        i2.insert_or_ignore(&conn).unwrap();

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

        let mut pkg = CanonicalPackage::new("curl".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 =
            PackageImplementation::new(cid, "fedora-41".into(), "curl".into(), "auto".into());
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 =
            PackageImplementation::new(cid, "ubuntu-noble".into(), "curl".into(), "auto".into());
        i2.insert_or_ignore(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("curl").unwrap();

        // Policy scope: prefer ubuntu-noble
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("ubuntu-noble".into()));
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(ranked[0].distro, "ubuntu-noble");
    }

    #[test]
    fn test_rank_with_policy_scope_flavor() {
        let (_t, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("curl".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 =
            PackageImplementation::new(cid, "fedora-41".into(), "curl".into(), "auto".into());
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 =
            PackageImplementation::new(cid, "ubuntu-noble".into(), "curl".into(), "auto".into());
        i2.insert_or_ignore(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("curl").unwrap();

        // Policy scope: prefer Deb flavor
        let policy = ResolutionPolicy::new()
            .with_scope(RequestScope::DistroFlavor(RepositoryDependencyFlavor::Deb));
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        assert_eq!(ranked[0].distro, "ubuntu-noble");
    }

    #[test]
    fn test_rank_override_beats_pin() {
        let (_t, conn) = create_test_db();

        // Pin to ubuntu-noble
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

        let mut pkg = CanonicalPackage::new("mesa".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 =
            PackageImplementation::new(cid, "fedora-41".into(), "mesa".into(), "auto".into());
        i1.insert_or_ignore(&conn).unwrap();
        let mut i2 =
            PackageImplementation::new(cid, "ubuntu-noble".into(), "mesa".into(), "auto".into());
        i2.insert_or_ignore(&conn).unwrap();

        // Override mesa to fedora
        PackageOverride::set(&conn, cid, "fedora-41", None).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("mesa").unwrap();

        let policy = ResolutionPolicy::new();
        let ranked = resolver
            .rank_candidates_with_policy(&candidates, &policy)
            .unwrap();
        // Override should beat pin
        assert_eq!(ranked[0].distro, "fedora-41");
    }

    #[test]
    fn test_distro_matches_flavor() {
        assert!(distro_matches_flavor(
            "fedora-41",
            RepositoryDependencyFlavor::Rpm
        ));
        assert!(distro_matches_flavor(
            "ubuntu-noble",
            RepositoryDependencyFlavor::Deb
        ));
        assert!(distro_matches_flavor(
            "arch",
            RepositoryDependencyFlavor::Arch
        ));
        assert!(!distro_matches_flavor(
            "fedora-41",
            RepositoryDependencyFlavor::Deb
        ));
    }
}
