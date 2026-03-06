// conary-core/src/resolver/canonical.rs

//! Canonical package resolver
//!
//! Expands canonical or distro-specific package names into resolver candidates,
//! ranks them by pin/affinity, and enforces mixing policy.

use crate::db::models::{
    CanonicalPackage, DistroPin, PackageImplementation, PackageOverride, SystemAffinity,
};
use crate::error::{Error, Result};
use rusqlite::Connection;

/// A candidate package from canonical expansion
#[derive(Debug, Clone)]
pub struct ResolverCandidate {
    pub distro_name: String,
    pub distro: String,
    pub canonical_id: i64,
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

    /// Expand a package name into all known implementation candidates.
    ///
    /// First tries the name as a canonical package name. If not found,
    /// tries as a distro-specific name and resolves to sibling implementations.
    pub fn expand(&self, name: &str) -> Result<Vec<ResolverCandidate>> {
        // Try as canonical name first
        if let Some(canonical) = CanonicalPackage::find_by_name(self.conn, name)? {
            let canonical_id = canonical.id.unwrap_or(0);
            let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;
            return Ok(impls
                .into_iter()
                .map(|i| ResolverCandidate {
                    distro_name: i.distro_name,
                    distro: i.distro,
                    canonical_id: i.canonical_id,
                })
                .collect());
        }

        // Try as distro-specific name
        if let Some(impl_entry) =
            PackageImplementation::find_by_any_distro_name(self.conn, name)?
        {
            let canonical_id = impl_entry.canonical_id;
            let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;
            return Ok(impls
                .into_iter()
                .map(|i| ResolverCandidate {
                    distro_name: i.distro_name,
                    distro: i.distro,
                    canonical_id: i.canonical_id,
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

    /// Get the distro override for a canonical package, if one exists.
    pub fn get_override(&self, canonical_id: i64) -> Result<Option<String>> {
        let ovr = PackageOverride::get(self.conn, canonical_id)?;
        Ok(ovr.map(|o| o.from_distro))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CanonicalPackage, DistroPin, PackageImplementation, PackageOverride};
    use crate::db::schema;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_expand_canonical_name() {
        let (_t, conn) = create_test_db();
        let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
        let cid = pkg.insert(&conn).unwrap();
        let mut i1 = PackageImplementation::new(
            cid,
            "fedora-41".into(),
            "httpd".into(),
            "curated".into(),
        );
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
        let mut i1 = PackageImplementation::new(
            cid,
            "fedora-41".into(),
            "httpd".into(),
            "curated".into(),
        );
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
            },
            ResolverCandidate {
                distro_name: "apache2".into(),
                distro: "ubuntu-noble".into(),
                canonical_id: 1,
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
            },
            ResolverCandidate {
                distro_name: "curl".into(),
                distro: "ubuntu-noble".into(),
                canonical_id: 1,
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
}
