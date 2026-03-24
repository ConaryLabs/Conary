// conary-core/src/derivation/convergence.rs

//! Cross-seed convergence verification.
//!
//! Compares output hashes produced by two independent bootstrap seeds to verify
//! that the build is seed-independent (i.e. reproducible across different starting
//! points). A fully converged build means every package produced the same
//! content-addressed output regardless of which seed was used.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;

/// Per-package comparison between two seeds.
#[derive(Debug, Clone)]
pub struct PackageComparison {
    pub package: String,
    pub hash_a: String,
    pub hash_b: String,
}

impl PackageComparison {
    /// Returns `true` if both seeds produced the same output hash.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.hash_a == self.hash_b
    }
}

/// Summary of convergence across all packages.
///
/// Fields are computed on demand from the underlying comparisons vector.
#[derive(Debug)]
pub struct ConvergenceReport {
    comparisons: Vec<PackageComparison>,
}

impl ConvergenceReport {
    /// Build a report from a pre-computed list of per-package comparisons.
    #[must_use]
    pub fn from_comparisons(comparisons: Vec<PackageComparison>) -> Self {
        Self { comparisons }
    }

    /// Total number of compared packages.
    #[must_use]
    pub fn total(&self) -> usize {
        self.comparisons.len()
    }

    /// Number of packages with matching output hashes.
    #[must_use]
    pub fn matched(&self) -> usize {
        self.comparisons.iter().filter(|c| c.matches()).count()
    }

    /// Number of packages with divergent output hashes.
    #[must_use]
    pub fn mismatched(&self) -> usize {
        self.total() - self.matched()
    }

    /// Returns `true` when every compared package converged (no mismatches).
    #[must_use]
    pub fn is_fully_converged(&self) -> bool {
        self.comparisons.iter().all(PackageComparison::matches)
    }

    /// Percentage of packages that converged, in the range `[0.0, 100.0]`.
    ///
    /// Returns `100.0` for an empty comparison set (vacuous convergence).
    #[must_use]
    pub fn convergence_pct(&self) -> f64 {
        if self.comparisons.is_empty() {
            return 100.0;
        }
        (self.matched() as f64 / self.comparisons.len() as f64) * 100.0
    }

    /// Returns references to all comparisons where the two seeds disagreed.
    #[must_use]
    pub fn mismatches(&self) -> Vec<&PackageComparison> {
        self.comparisons.iter().filter(|c| !c.matches()).collect()
    }

    /// Access the underlying comparisons.
    #[must_use]
    pub fn comparisons(&self) -> &[PackageComparison] {
        &self.comparisons
    }
}

/// Query `derivation_index` for all builds produced by a given seed.
///
/// The seed is identified by `build_env_hash`, which is set to the seed's
/// SHA-256 hash at build time.
fn builds_for_seed(conn: &Connection, seed_id: &str) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT package_name, output_hash
         FROM derivation_index
         WHERE build_env_hash = ?1",
    )?;

    let rows = stmt.query_map([seed_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (pkg, hash) = row?;
        // Last writer wins when a seed has multiple builds for the same package
        // (e.g. rebuild after a cache clear). The caller can use by_package()
        // on DerivationIndex if finer-grained control is needed.
        map.insert(pkg, hash);
    }
    Ok(map)
}

/// Compare output hashes from two seeds using the derivation index.
///
/// Identifies builds from each seed via `build_env_hash`. Only packages that
/// appear in **both** seeds are compared; packages present in only one seed are
/// silently skipped (they cannot be convergence-tested without a counterpart).
///
/// Returns a [`ConvergenceReport`] summarising how many packages matched and
/// which ones differed.
pub fn compare_seed_builds(
    conn: &Connection,
    seed_a_id: &str,
    seed_b_id: &str,
) -> Result<ConvergenceReport> {
    let builds_a = builds_for_seed(conn, seed_a_id)?;
    let builds_b = builds_for_seed(conn, seed_b_id)?;

    let mut comparisons: Vec<PackageComparison> = builds_a
        .iter()
        .filter_map(|(pkg, hash_a)| {
            builds_b.get(pkg).map(|hash_b| PackageComparison {
                package: pkg.clone(),
                hash_a: hash_a.clone(),
                hash_b: hash_b.clone(),
            })
        })
        .collect();

    // Stable ordering for deterministic reports / test assertions.
    comparisons.sort_by(|a, b| a.package.cmp(&b.package));

    Ok(ConvergenceReport::from_comparisons(comparisons))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;
    use crate::derivation::index::{DerivationIndex, DerivationRecord};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn insert_build(conn: &Connection, pkg: &str, output_hash: &str, seed_id: &str) {
        let idx = DerivationIndex::new(conn);
        let record = DerivationRecord {
            derivation_id: format!("{pkg}-{seed_id}"),
            output_hash: output_hash.to_owned(),
            package_name: pkg.to_owned(),
            package_version: "1.0.0".to_owned(),
            manifest_cas_hash: format!("manifest-{pkg}-{seed_id}"),
            stage: Some("phase1".to_owned()),
            build_env_hash: Some(seed_id.to_owned()),
            built_at: "2026-03-23T00:00:00Z".to_owned(),
            build_duration_secs: 1,
            trust_level: 2,
            provenance_cas_hash: None,
            reproducible: None,
        };
        idx.insert(&record).unwrap();
    }

    // --- pure unit tests (no DB) ---

    #[test]
    fn test_full_convergence() {
        let comparisons = vec![
            PackageComparison {
                package: "gcc".into(),
                hash_a: "aaa".into(),
                hash_b: "aaa".into(),
            },
            PackageComparison {
                package: "bash".into(),
                hash_a: "bbb".into(),
                hash_b: "bbb".into(),
            },
        ];
        let report = ConvergenceReport::from_comparisons(comparisons);
        assert!(report.is_fully_converged());
        assert_eq!(report.convergence_pct(), 100.0);
        assert_eq!(report.matched(), 2);
        assert!(report.mismatches().is_empty());
    }

    #[test]
    fn test_partial_convergence() {
        let comparisons = vec![
            PackageComparison {
                package: "gcc".into(),
                hash_a: "aaa".into(),
                hash_b: "aaa".into(),
            },
            PackageComparison {
                package: "python".into(),
                hash_a: "bbb".into(),
                hash_b: "ccc".into(),
            },
        ];
        let report = ConvergenceReport::from_comparisons(comparisons);
        assert!(!report.is_fully_converged());
        assert_eq!(report.matched(), 1);
        assert_eq!(report.mismatched(), 1);
        assert_eq!(report.mismatches().len(), 1);
        assert_eq!(report.mismatches()[0].package, "python");
    }

    #[test]
    fn test_empty_convergence() {
        let report = ConvergenceReport::from_comparisons(vec![]);
        assert!(report.is_fully_converged());
        assert_eq!(report.convergence_pct(), 100.0);
    }

    #[test]
    fn test_package_comparison_matches() {
        let m = PackageComparison {
            package: "x".into(),
            hash_a: "same".into(),
            hash_b: "same".into(),
        };
        assert!(m.matches());
        let n = PackageComparison {
            package: "y".into(),
            hash_a: "a".into(),
            hash_b: "b".into(),
        };
        assert!(!n.matches());
    }

    // --- DB-backed integration tests ---

    #[test]
    fn compare_seeds_full_convergence() {
        let conn = setup();
        insert_build(&conn, "gcc", "hash_gcc", "seed_a");
        insert_build(&conn, "bash", "hash_bash", "seed_a");
        insert_build(&conn, "gcc", "hash_gcc", "seed_b");
        insert_build(&conn, "bash", "hash_bash", "seed_b");

        let report = compare_seed_builds(&conn, "seed_a", "seed_b").unwrap();
        assert!(report.is_fully_converged());
        assert_eq!(report.total(), 2);
        assert_eq!(report.matched(), 2);
    }

    #[test]
    fn compare_seeds_partial_mismatch() {
        let conn = setup();
        insert_build(&conn, "gcc", "hash_gcc", "seed_a");
        insert_build(&conn, "python", "hash_py_a", "seed_a");
        insert_build(&conn, "gcc", "hash_gcc", "seed_b");
        insert_build(&conn, "python", "hash_py_b", "seed_b"); // different

        let report = compare_seed_builds(&conn, "seed_a", "seed_b").unwrap();
        assert!(!report.is_fully_converged());
        assert_eq!(report.total(), 2);
        assert_eq!(report.matched(), 1);
        assert_eq!(report.mismatched(), 1);
        assert_eq!(report.mismatches()[0].package, "python");
    }

    #[test]
    fn compare_seeds_skips_packages_only_in_one_seed() {
        let conn = setup();
        insert_build(&conn, "shared", "hash_shared", "seed_a");
        insert_build(&conn, "only_in_a", "hash_only_a", "seed_a");
        insert_build(&conn, "shared", "hash_shared", "seed_b");
        // "only_in_a" is absent from seed_b

        let report = compare_seed_builds(&conn, "seed_a", "seed_b").unwrap();
        // Only "shared" should be compared
        assert_eq!(report.total(), 1);
        assert!(report.is_fully_converged());
        assert_eq!(report.comparisons()[0].package, "shared");
    }

    #[test]
    fn compare_seeds_empty_when_no_builds() {
        let conn = setup();
        let report = compare_seed_builds(&conn, "nonexistent_a", "nonexistent_b").unwrap();
        assert_eq!(report.total(), 0);
        assert!(report.is_fully_converged());
        assert_eq!(report.convergence_pct(), 100.0);
    }
}
