// conary-server/src/server/canonical_job.rs

//! Scheduled job that builds the canonical package mapping from all
//! indexed distros. Runs after mirror sync or on demand.

use std::path::Path;

use anyhow::Result;
use conary_core::canonical::rules::RulesEngine;
use conary_core::canonical::sync::{RepoPackageInfo, ingest_canonical_mappings};
use rusqlite::Connection;
use tracing::{debug, info, warn};

/// Rebuild the canonical map from all enabled repositories.
///
/// Opens the database at `db_path`, loads curated YAML rules from `rules_dir`
/// (if the directory exists), builds a package list from all enabled repos,
/// and runs the canonical mapping pipeline (curated rules first, then
/// auto-discovery for unmatched packages).
///
/// Returns the count of newly created canonical package entries.
pub fn rebuild_canonical_map(db_path: &Path, rules_dir: &Path) -> Result<u64> {
    let conn = conary_core::db::open(db_path)?;

    // Load curated rules if the directory exists.
    let rules = if rules_dir.is_dir() {
        match RulesEngine::load_from_dir(rules_dir) {
            Ok(engine) => {
                info!(
                    "Loaded {} curated canonical rules from {}",
                    engine.rule_count(),
                    rules_dir.display()
                );
                Some(engine)
            }
            Err(e) => {
                warn!(
                    "Failed to load canonical rules from {}: {}",
                    rules_dir.display(),
                    e
                );
                None
            }
        }
    } else {
        debug!(
            "No canonical rules directory at {}, skipping curated rules",
            rules_dir.display()
        );
        None
    };

    let packages = build_repo_package_list(&conn)?;
    info!(
        "Built package list: {} packages from enabled repositories",
        packages.len()
    );

    if packages.is_empty() {
        return Ok(0);
    }

    let new_count = ingest_canonical_mappings(&conn, &packages, rules.as_ref())?;
    info!("Canonical map rebuild complete: {} new mappings", new_count);

    Ok(new_count as u64)
}

/// Query all packages from all enabled repositories and build a list of
/// `RepoPackageInfo` for canonical mapping.
///
/// Uses `COALESCE(r.default_strategy_distro, r.name)` as the distro
/// identifier so that repos with an explicit distro strategy use it,
/// while others fall back to the repository name.
fn build_repo_package_list(conn: &Connection) -> Result<Vec<RepoPackageInfo>> {
    let mut stmt = conn.prepare(
        "SELECT rp.name, COALESCE(r.default_strategy_distro, r.name) AS distro
         FROM repository_packages rp
         JOIN repositories r ON rp.repository_id = r.id
         WHERE r.enabled = 1",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(RepoPackageInfo {
            name: row.get(0)?,
            distro: row.get(1)?,
            provides: Vec::new(),
            files: Vec::new(),
        })
    })?;

    let mut packages = Vec::new();
    for row in rows {
        packages.push(row?);
    }

    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn create_test_db(dir: &TempDir) -> std::path::PathBuf {
        let db_path = dir.path().join("conary.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Insert a test repository
        conn.execute(
            "INSERT INTO repositories (name, url, repo_type, enabled, default_strategy_distro)
             VALUES ('fedora-43', 'https://example.com/fedora', 'rpm', 1, 'fedora')",
            [],
        )
        .unwrap();

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE name = 'fedora-43'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Insert test packages
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'curl', '8.5.0', 'abc123', 1000, 'https://example.com/curl.rpm')",
            [repo_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'wget', '1.21', 'def456', 2000, 'https://example.com/wget.rpm')",
            [repo_id],
        )
        .unwrap();

        db_path
    }

    #[test]
    fn test_build_repo_package_list() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);
        let conn = conary_core::db::open(&db_path).unwrap();

        let packages = build_repo_package_list(&conn).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].distro, "fedora");
        assert!(packages.iter().any(|p| p.name == "curl"));
        assert!(packages.iter().any(|p| p.name == "wget"));
    }

    #[test]
    fn test_build_repo_package_list_skips_disabled() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);
        let conn = conary_core::db::open(&db_path).unwrap();

        // Disable the repository
        conn.execute(
            "UPDATE repositories SET enabled = 0 WHERE name = 'fedora-43'",
            [],
        )
        .unwrap();

        let packages = build_repo_package_list(&conn).unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_rebuild_canonical_map_empty_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("conary.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        drop(conn);

        let rules_dir = dir.path().join("rules");
        let count = rebuild_canonical_map(&db_path, &rules_dir).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_rebuild_canonical_map_with_packages() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);
        let rules_dir = dir.path().join("rules");

        let count = rebuild_canonical_map(&db_path, &rules_dir).unwrap();
        // curl and wget should each produce at least one canonical mapping
        assert!(count > 0);
    }

    #[test]
    fn test_rebuild_canonical_map_with_rules() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);

        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(
            rules_dir.join("01-rename.yaml"),
            "rules:\n  - name: curl\n    setname: curl-tools\n",
        )
        .unwrap();

        let count = rebuild_canonical_map(&db_path, &rules_dir).unwrap();
        assert!(count > 0);

        // Verify the curated rule took effect
        let conn = conary_core::db::open(&db_path).unwrap();
        let pkg =
            conary_core::db::models::CanonicalPackage::find_by_name(&conn, "curl-tools").unwrap();
        assert!(
            pkg.is_some(),
            "curated rule should create 'curl-tools' canonical entry"
        );
    }
}
