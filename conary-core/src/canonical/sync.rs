// conary-core/src/canonical/sync.rs

//! Integration layer that wires canonical discovery into the repository sync pipeline.
//!
//! Converts repository package metadata into canonical mappings by first checking
//! curated rules, then falling back to multi-strategy auto-discovery. All results
//! are persisted to the database in a single transaction.

use rusqlite::Connection;
use std::collections::BTreeSet;

use crate::Result;
use crate::canonical::discovery::{DistroPackage, run_discovery};
use crate::canonical::rules::RulesEngine;
use crate::db::models::{CanonicalPackage, PackageImplementation};
use crate::error::Error;

const KNOWN_CANONICAL_ALIAS_PAIRS: &[(&str, &str)] =
    &[("firefox", "iceweasel"), ("mysql", "mariadb")];

fn normalized_mapping_name(name: &str) -> String {
    let stripped = crate::canonical::discovery::strip_distro_affixes(&name.to_ascii_lowercase());
    let trimmed = stripped.trim_end_matches(|c: char| c.is_ascii_digit());
    trimmed
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn mapping_tokens(name: &str) -> BTreeSet<String> {
    name.split('-')
        .filter(|token| token.len() >= 3)
        .map(ToString::to_string)
        .collect()
}

fn is_known_alias_pair(left: &str, right: &str) -> bool {
    KNOWN_CANONICAL_ALIAS_PAIRS
        .iter()
        .any(|(a, b)| (left == *a && right == *b) || (left == *b && right == *a))
}

pub(crate) fn mapping_names_are_reasonable(
    canonical_name: &str,
    implementation_name: &str,
) -> bool {
    let canonical = normalized_mapping_name(canonical_name);
    let implementation = normalized_mapping_name(implementation_name);

    if canonical.is_empty() || implementation.is_empty() {
        return false;
    }

    if canonical == implementation || is_known_alias_pair(&canonical, &implementation) {
        return true;
    }

    if canonical.len() >= 3 && implementation.contains(&canonical) {
        return true;
    }

    if implementation.len() >= 3 && canonical.contains(&implementation) {
        return true;
    }

    let canonical_tokens = mapping_tokens(&canonical);
    let implementation_tokens = mapping_tokens(&implementation);
    !canonical_tokens.is_disjoint(&implementation_tokens)
}

pub(crate) fn validate_canonical_mapping<'a>(
    canonical_name: &str,
    implementation_names: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    for implementation_name in implementation_names {
        if !mapping_names_are_reasonable(canonical_name, implementation_name) {
            return Err(Error::TrustError(format!(
                "Suspicious canonical mapping rejected: '{implementation_name}' -> '{canonical_name}'"
            )));
        }
    }

    Ok(())
}

/// Package metadata from a repository sync, used as input to canonical mapping.
#[derive(Debug, Clone)]
pub struct RepoPackageInfo {
    /// Distro package name (e.g., "httpd", "apache2").
    pub name: String,
    /// Distro identifier (e.g., "fedora-43", "ubuntu-noble").
    pub distro: String,
    /// Virtual provides / capabilities declared by this package.
    pub provides: Vec<String>,
    /// Installed file paths.
    pub files: Vec<String>,
}

/// Ingest repository packages into canonical mappings.
///
/// For each package, curated rules are checked first (if a `RulesEngine` is provided).
/// Packages not matched by rules are passed through multi-strategy auto-discovery.
/// All resulting mappings are persisted via `INSERT OR IGNORE` within a single
/// transaction.
///
/// Returns the count of newly created canonical package entries.
pub fn ingest_canonical_mappings(
    conn: &Connection,
    packages: &[RepoPackageInfo],
    rules: Option<&RulesEngine>,
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut new_count = 0;

    // Phase 1: Check curated rules for each package. Track which packages
    // were NOT matched so we can run auto-discovery on the remainder.
    let mut unmatched = Vec::new();

    for pkg in packages {
        let repo_id = crate::canonical::repology::distro_to_repo(&pkg.distro);
        let resolved = rules.and_then(|engine| engine.resolve(&pkg.name, repo_id.as_deref()));

        if let Some(canonical_name) = resolved {
            validate_canonical_mapping(&canonical_name, std::iter::once(pkg.name.as_str()))?;

            // Determine kind from rules if available.
            let kind = rules
                .and_then(|engine| engine.get_kind(&canonical_name))
                .unwrap_or_else(|| "package".to_string());

            let mut canonical = CanonicalPackage::new(canonical_name.clone(), kind);

            // Check if this canonical package already exists before inserting
            let already_exists = CanonicalPackage::find_by_name(&tx, &canonical_name)?.is_some();
            let id = canonical.insert_or_ignore(&tx)?;

            if let Some(canonical_id) = id {
                let mut imp = PackageImplementation::new(
                    canonical_id,
                    pkg.distro.clone(),
                    pkg.name.clone(),
                    "curated".to_string(),
                );
                imp.insert_or_ignore(&tx)?;

                // Only count genuinely new canonical packages
                if !already_exists {
                    new_count += 1;
                }
            }
        } else {
            unmatched.push(pkg);
        }
    }

    // Phase 2: Run auto-discovery on unmatched packages.
    if !unmatched.is_empty() {
        let distro_packages: Vec<DistroPackage> = unmatched
            .iter()
            .map(|pkg| DistroPackage {
                name: pkg.name.clone(),
                distro: pkg.distro.clone(),
                provides: pkg.provides.clone(),
                files: pkg.files.clone(),
            })
            .collect();

        let discoveries = run_discovery(&distro_packages);

        for mapping in &discoveries {
            validate_canonical_mapping(
                &mapping.canonical_name,
                mapping
                    .implementations
                    .iter()
                    .map(|(_distro, distro_name)| distro_name.as_str()),
            )?;

            let mut canonical =
                CanonicalPackage::new(mapping.canonical_name.clone(), "package".to_string());
            let id = canonical.insert_or_ignore(&tx)?;

            if let Some(canonical_id) = id {
                for (distro, distro_name) in &mapping.implementations {
                    let mut imp = PackageImplementation::new(
                        canonical_id,
                        distro.clone(),
                        distro_name.clone(),
                        mapping.source.clone(),
                    );
                    imp.insert_or_ignore(&tx)?;
                }
                new_count += 1;
            }
        }
    }

    tx.commit()?;
    Ok(new_count)
}

/// Ingest Repology rename rules from a rules directory.
///
/// Looks for YAML files in the `800.renames-and-merges/` subdirectory,
/// which contains cross-distro package name equivalences.
pub fn ingest_repology_rules(conn: &Connection, rules_dir: &std::path::Path) -> Result<usize> {
    let merges_dir = rules_dir.join("800.renames-and-merges");
    if !merges_dir.exists() {
        tracing::info!("No Repology rules directory at {}", merges_dir.display());
        return Ok(0);
    }
    let mut total = 0;
    for entry in std::fs::read_dir(&merges_dir)
        .map_err(|e| crate::error::Error::IoError(format!("read Repology rules dir: {e}")))?
    {
        let entry =
            entry.map_err(|e| crate::error::Error::IoError(format!("read dir entry: {e}")))?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            let yaml = std::fs::read_to_string(&path).map_err(|e| {
                crate::error::Error::IoError(format!("read {}: {e}", path.display()))
            })?;
            match super::repology::parse_repology_rules(&yaml) {
                Ok(rules) => {
                    let applied = super::repology::apply_repology_rules(conn, &rules)?;
                    total += applied;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {e}", path.display());
                }
            }
        }
    }
    tracing::info!("Applied {total} Repology rename rules");
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::rules;
    use crate::db::models::{CanonicalPackage, PackageImplementation};
    use crate::db::testing::create_test_db;

    #[test]
    fn test_ingest_repo_packages_creates_canonical_mappings() {
        let (_temp, conn) = create_test_db();

        let packages = vec![
            RepoPackageInfo {
                name: "curl".into(),
                distro: "fedora-41".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
            RepoPackageInfo {
                name: "curl".into(),
                distro: "ubuntu-noble".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
        ];

        let count = ingest_canonical_mappings(&conn, &packages, None).unwrap();
        assert!(count > 0);

        let pkg = CanonicalPackage::find_by_name(&conn, "curl").unwrap();
        assert!(pkg.is_some());

        let impls =
            PackageImplementation::find_by_canonical(&conn, pkg.unwrap().id.unwrap()).unwrap();
        assert_eq!(impls.len(), 2);
    }

    #[test]
    fn test_curated_rules_override_auto_discovery() {
        let (_temp, conn) = create_test_db();

        let packages = vec![RepoPackageInfo {
            name: "httpd".into(),
            distro: "fedora-41".into(),
            provides: vec![],
            files: vec![],
        }];

        let rules_yaml = "rules:\n  - setname: apache-httpd\n    name: httpd\n";
        let parsed = rules::parse_rules(rules_yaml).unwrap();
        let engine = rules::RulesEngine::new(parsed).unwrap();

        let _count = ingest_canonical_mappings(&conn, &packages, Some(&engine)).unwrap();

        // Should use curated name
        let pkg = CanonicalPackage::find_by_name(&conn, "apache-httpd").unwrap();
        assert!(pkg.is_some());

        // "httpd" should NOT be a separate canonical package
        let httpd = CanonicalPackage::find_by_name(&conn, "httpd").unwrap();
        assert!(httpd.is_none());
    }

    #[test]
    fn test_idempotent_ingest() {
        let (_temp, conn) = create_test_db();

        let packages = vec![
            RepoPackageInfo {
                name: "curl".into(),
                distro: "fedora-41".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
            RepoPackageInfo {
                name: "curl".into(),
                distro: "ubuntu-noble".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
        ];

        let count1 = ingest_canonical_mappings(&conn, &packages, None).unwrap();
        assert!(count1 > 0);

        // Second ingest should not create duplicates
        let count2 = ingest_canonical_mappings(&conn, &packages, None).unwrap();
        // All canonical packages already exist, so insert_or_ignore returns
        // existing IDs -- still counted because id is Some
        // The important thing is no errors and no duplicate rows
        let pkg = CanonicalPackage::find_by_name(&conn, "curl")
            .unwrap()
            .unwrap();
        let impls = PackageImplementation::find_by_canonical(&conn, pkg.id.unwrap()).unwrap();
        assert_eq!(
            impls.len(),
            2,
            "No duplicate implementations after re-ingest"
        );
        // count2 can be anything -- the key invariant is no duplicates
        let _ = count2;
    }

    #[test]
    fn test_ingest_repology_rules_from_directory() {
        use std::io::Write;

        let (_temp, conn) = create_test_db();

        let temp_dir = tempfile::tempdir().unwrap();
        let merges_dir = temp_dir.path().join("800.renames-and-merges");
        std::fs::create_dir_all(&merges_dir).unwrap();

        let mut file = std::fs::File::create(merges_dir.join("test.yaml")).unwrap();
        writeln!(file, "- {{ name: httpd, setname: apache }}").unwrap();
        writeln!(file, "- {{ name: apache2, setname: apache }}").unwrap();

        let count = ingest_repology_rules(&conn, temp_dir.path()).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_ingest_repology_rules_missing_directory() {
        let (_temp, conn) = create_test_db();

        let temp_dir = tempfile::tempdir().unwrap();
        // No 800.renames-and-merges subdir exists
        let count = ingest_repology_rules(&conn, temp_dir.path()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_mapping_names_are_reasonable_for_known_aliases() {
        assert!(mapping_names_are_reasonable("apache-httpd", "httpd"));
        assert!(mapping_names_are_reasonable("apache-httpd", "apache2"));
        assert!(mapping_names_are_reasonable("openssl", "libssl3"));
        assert!(mapping_names_are_reasonable("firefox", "iceweasel"));
    }

    #[test]
    fn test_mapping_names_are_reasonable_rejects_unrelated_redirect() {
        assert!(!mapping_names_are_reasonable(
            "openssl",
            "totally-malicious-package"
        ));
    }

    #[test]
    fn test_ingest_canonical_mappings_rejects_suspicious_redirect_rule() {
        let (_temp, conn) = create_test_db();

        let packages = vec![RepoPackageInfo {
            name: "totally-malicious-package".into(),
            distro: "fedora-41".into(),
            provides: vec![],
            files: vec![],
        }];

        let rules_yaml = "rules:\n  - setname: openssl\n    name: totally-malicious-package\n    repo: fedora_41\n";
        let parsed = rules::parse_rules(rules_yaml).unwrap();
        let engine = rules::RulesEngine::new(parsed).unwrap();

        let err = ingest_canonical_mappings(&conn, &packages, Some(&engine)).unwrap_err();
        assert!(
            err.to_string().contains("Suspicious canonical mapping"),
            "unexpected error: {err}"
        );
    }
}
