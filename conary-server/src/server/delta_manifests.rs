// conary-server/src/server/delta_manifests.rs
//! Delta manifests for efficient package updates
//!
//! Pre-computes the set difference in chunks between package versions so
//! clients only download new chunks when upgrading. Results are persisted
//! in the `delta_manifests` table for fast lookup.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::{debug, info, warn};

/// A pre-computed delta between two versions of a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaManifest {
    pub id: Option<i64>,
    pub distro: String,
    pub package_name: String,
    pub from_version: String,
    pub to_version: String,
    /// JSON array of chunk hashes present in to_version but not from_version
    pub new_chunks: Vec<String>,
    /// JSON array of chunk hashes present in from_version but not to_version
    pub removed_chunks: Vec<String>,
    /// Total download size of new chunks in bytes
    pub download_size: u64,
    /// Full package size of to_version in bytes
    pub full_size: u64,
    pub computed_at: Option<String>,
}

/// API response for a delta query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaResponse {
    pub from_version: String,
    pub to_version: String,
    pub new_chunks: Vec<String>,
    pub removed_chunks: Vec<String>,
    pub download_size: u64,
    pub full_size: u64,
    pub savings_percent: f64,
}

impl DeltaManifest {
    /// Convert to an API response
    pub fn to_response(&self) -> DeltaResponse {
        let savings_percent = if self.full_size > 0 {
            let saved = self.full_size.saturating_sub(self.download_size);
            (saved as f64 / self.full_size as f64) * 100.0
        } else {
            0.0
        };

        DeltaResponse {
            from_version: self.from_version.clone(),
            to_version: self.to_version.clone(),
            new_chunks: self.new_chunks.clone(),
            removed_chunks: self.removed_chunks.clone(),
            download_size: self.download_size,
            full_size: self.full_size,
            savings_percent,
        }
    }
}

/// Get chunk hashes and their sizes for a specific converted package version.
///
/// Returns `(chunk_hashes, total_size)` from the `converted_packages` table
/// and `chunk_access` for individual chunk sizes.
fn get_version_chunks(
    conn: &Connection,
    distro: &str,
    package_name: &str,
    version: &str,
) -> Result<(Vec<String>, u64)> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT chunk_hashes_json, COALESCE(total_size, 0)
             FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3
             LIMIT 1",
            params![distro, package_name, version],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;

    match row {
        Some((json, total_size)) => {
            let hashes: Vec<String> = serde_json::from_str(&json).with_context(|| {
                format!(
                    "Failed to parse chunk_hashes_json for {}/{}/{}",
                    distro, package_name, version
                )
            })?;
            Ok((hashes, total_size as u64))
        }
        None => Ok((Vec::new(), 0)),
    }
}

/// Look up chunk sizes from the chunk_access table.
///
/// Returns a map of hash -> size_bytes for all requested hashes found in the DB.
fn get_chunk_sizes(conn: &Connection, hashes: &[String]) -> Result<HashMap<String, u64>> {
    let mut sizes = HashMap::new();
    if hashes.is_empty() {
        return Ok(sizes);
    }

    let mut stmt = conn.prepare("SELECT hash, size_bytes FROM chunk_access WHERE hash = ?1")?;

    for hash in hashes {
        if let Some(size) = stmt
            .query_row([hash], |row| row.get::<_, i64>(1))
            .optional()?
        {
            sizes.insert(hash.clone(), size as u64);
        }
    }

    Ok(sizes)
}

/// Compute the delta manifest between two versions of a package.
///
/// Queries chunk hashes for both versions from `converted_packages`,
/// computes the set difference (new_chunks = in to_version but not from_version,
/// removed_chunks = in from_version but not to_version), calculates download_size
/// from chunk sizes in `chunk_access`, and inserts the result into `delta_manifests`.
pub fn compute_delta(
    conn: &Connection,
    distro: &str,
    package_name: &str,
    from_version: &str,
    to_version: &str,
) -> Result<DeltaManifest> {
    debug!(
        "Computing delta for {}/{}: {} -> {}",
        distro, package_name, from_version, to_version
    );

    // Get chunks for both versions
    let (from_chunks, _from_size) = get_version_chunks(conn, distro, package_name, from_version)?;
    let (to_chunks, to_size) = get_version_chunks(conn, distro, package_name, to_version)?;

    let from_set: HashSet<&str> = from_chunks.iter().map(String::as_str).collect();
    let to_set: HashSet<&str> = to_chunks.iter().map(String::as_str).collect();

    // New chunks: in to_version but not from_version
    let new_chunks: Vec<String> = to_set
        .difference(&from_set)
        .map(|s| (*s).to_string())
        .collect();

    // Removed chunks: in from_version but not to_version
    let removed_chunks: Vec<String> = from_set
        .difference(&to_set)
        .map(|s| (*s).to_string())
        .collect();

    // Calculate download size from new chunk sizes
    let chunk_sizes = get_chunk_sizes(conn, &new_chunks)?;
    let download_size: u64 = new_chunks
        .iter()
        .map(|h| chunk_sizes.get(h).copied().unwrap_or(0))
        .sum();

    let new_chunks_json = serde_json::to_string(&new_chunks)?;
    let removed_chunks_json = serde_json::to_string(&removed_chunks)?;

    // Insert or replace into delta_manifests
    conn.execute(
        "INSERT OR REPLACE INTO delta_manifests
         (distro, package_name, from_version, to_version, new_chunks, removed_chunks, download_size, full_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            distro,
            package_name,
            from_version,
            to_version,
            &new_chunks_json,
            &removed_chunks_json,
            download_size as i64,
            to_size as i64,
        ],
    )?;

    let id = conn.last_insert_rowid();

    info!(
        "Delta computed for {}/{}: {} -> {} ({} new, {} removed, {} bytes to download vs {} full)",
        distro,
        package_name,
        from_version,
        to_version,
        new_chunks.len(),
        removed_chunks.len(),
        download_size,
        to_size
    );

    Ok(DeltaManifest {
        id: Some(id),
        distro: distro.to_string(),
        package_name: package_name.to_string(),
        from_version: from_version.to_string(),
        to_version: to_version.to_string(),
        new_chunks,
        removed_chunks,
        download_size,
        full_size: to_size,
        computed_at: None,
    })
}

/// Compute deltas for all adjacent version pairs of a package.
///
/// Finds all converted versions, sorts them with scheme-aware comparison
/// (RPM/Debian/Arch), then computes deltas between each adjacent pair
/// (v1->v2, v2->v3, ...).
pub fn compute_deltas_for_package(
    conn: &Connection,
    distro: &str,
    package_name: &str,
) -> Result<Vec<DeltaManifest>> {
    use conary_core::repository::versioning::{VersionScheme, compare_repo_versions};

    // Determine the version comparison scheme from the distro.
    let scheme = match distro {
        "arch" => VersionScheme::Arch,
        "fedora" | "centos" | "rhel" => VersionScheme::Rpm,
        "ubuntu" | "debian" => VersionScheme::Debian,
        _ => {
            warn!("Unknown distro '{}' for delta computation, using RPM ordering", distro);
            VersionScheme::Rpm
        }
    };

    // Get all converted versions for this package (unordered from DB).
    let mut stmt = conn.prepare(
        "SELECT DISTINCT package_version FROM converted_packages
         WHERE distro = ?1 AND package_name = ?2 AND package_version IS NOT NULL",
    )?;

    let mut versions: Vec<String> = stmt
        .query_map(params![distro, package_name], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Sort with scheme-aware comparison instead of lexicographic ordering.
    versions.sort_by(|a, b| {
        compare_repo_versions(scheme, a, b).unwrap_or(std::cmp::Ordering::Equal)
    });

    if versions.len() < 2 {
        debug!(
            "Package {}/{} has {} versions, need at least 2 for deltas",
            distro,
            package_name,
            versions.len()
        );
        return Ok(Vec::new());
    }

    let mut deltas = Vec::new();
    for pair in versions.windows(2) {
        let from_version = &pair[0];
        let to_version = &pair[1];

        match compute_delta(conn, distro, package_name, from_version, to_version) {
            Ok(delta) => deltas.push(delta),
            Err(e) => {
                warn!(
                    "Failed to compute delta {}/{}: {} -> {}: {}",
                    distro, package_name, from_version, to_version, e
                );
            }
        }
    }

    info!(
        "Computed {} deltas for {}/{}",
        deltas.len(),
        distro,
        package_name
    );
    Ok(deltas)
}

/// Look up a pre-computed delta manifest.
pub fn get_delta(
    conn: &Connection,
    distro: &str,
    package_name: &str,
    from_version: &str,
    to_version: &str,
) -> Result<Option<DeltaManifest>> {
    let row = conn
        .query_row(
            "SELECT id, distro, package_name, from_version, to_version,
                    new_chunks, removed_chunks, download_size, full_size, computed_at
             FROM delta_manifests
             WHERE distro = ?1 AND package_name = ?2
               AND from_version = ?3 AND to_version = ?4",
            params![distro, package_name, from_version, to_version],
            |row| {
                let new_chunks_json: String = row.get(5)?;
                let removed_chunks_json: String = row.get(6)?;

                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    new_chunks_json,
                    removed_chunks_json,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?;

    match row {
        Some((id, distro, pkg, from_v, to_v, new_json, rem_json, dl_size, full_size, computed)) => {
            let new_chunks: Vec<String> = serde_json::from_str(&new_json).unwrap_or_default();
            let removed_chunks: Vec<String> = serde_json::from_str(&rem_json).unwrap_or_default();

            Ok(Some(DeltaManifest {
                id: Some(id),
                distro,
                package_name: pkg,
                from_version: from_v,
                to_version: to_v,
                new_chunks,
                removed_chunks,
                download_size: dl_size as u64,
                full_size: full_size as u64,
                computed_at: computed,
            }))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::ConvertedPackage;
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn insert_converted(
        conn: &Connection,
        distro: &str,
        name: &str,
        version: &str,
        chunks: &[&str],
        total_size: i64,
    ) {
        let chunk_strings: Vec<String> = chunks.iter().map(|s| (*s).to_string()).collect();
        let mut pkg = ConvertedPackage::new_server(
            distro.to_string(),
            name.to_string(),
            version.to_string(),
            "rpm".to_string(),
            format!("sha256:{name}-{version}"),
            "high".to_string(),
            &chunk_strings,
            total_size,
            format!("sha256:content-{name}-{version}"),
            format!("/data/{name}-{version}.ccs"),
        );
        pkg.insert(conn).unwrap();
    }

    fn insert_chunk(conn: &Connection, hash: &str, size: i64) {
        use conary_core::db::models::ChunkAccess;
        let chunk = ChunkAccess::new(hash.to_string(), size);
        chunk.upsert(conn).unwrap();
    }

    #[test]
    fn test_compute_delta_basic() {
        let (_temp, conn) = create_test_db();

        // Version 1 has chunks A, B, C
        insert_converted(
            &conn,
            "fedora",
            "nginx",
            "1.0",
            &["chunkA", "chunkB", "chunkC"],
            3000,
        );
        // Version 2 has chunks B, C, D (A removed, D added)
        insert_converted(
            &conn,
            "fedora",
            "nginx",
            "2.0",
            &["chunkB", "chunkC", "chunkD"],
            3500,
        );

        insert_chunk(&conn, "chunkA", 1000);
        insert_chunk(&conn, "chunkB", 1000);
        insert_chunk(&conn, "chunkC", 1000);
        insert_chunk(&conn, "chunkD", 1500);

        let delta = compute_delta(&conn, "fedora", "nginx", "1.0", "2.0").unwrap();

        assert_eq!(delta.distro, "fedora");
        assert_eq!(delta.package_name, "nginx");
        assert_eq!(delta.from_version, "1.0");
        assert_eq!(delta.to_version, "2.0");
        assert_eq!(delta.new_chunks.len(), 1);
        assert!(delta.new_chunks.contains(&"chunkD".to_string()));
        assert_eq!(delta.removed_chunks.len(), 1);
        assert!(delta.removed_chunks.contains(&"chunkA".to_string()));
        assert_eq!(delta.download_size, 1500); // size of chunkD
        assert_eq!(delta.full_size, 3500);
    }

    #[test]
    fn test_compute_delta_identical_versions() {
        let (_temp, conn) = create_test_db();

        insert_converted(&conn, "fedora", "curl", "1.0", &["chunkX", "chunkY"], 2000);
        insert_converted(&conn, "fedora", "curl", "1.1", &["chunkX", "chunkY"], 2000);

        let delta = compute_delta(&conn, "fedora", "curl", "1.0", "1.1").unwrap();

        assert!(delta.new_chunks.is_empty());
        assert!(delta.removed_chunks.is_empty());
        assert_eq!(delta.download_size, 0);
    }

    #[test]
    fn test_compute_delta_no_overlap() {
        let (_temp, conn) = create_test_db();

        insert_converted(&conn, "arch", "vim", "1.0", &["chunkA", "chunkB"], 2000);
        insert_converted(&conn, "arch", "vim", "2.0", &["chunkC", "chunkD"], 2500);

        insert_chunk(&conn, "chunkC", 1200);
        insert_chunk(&conn, "chunkD", 1300);

        let delta = compute_delta(&conn, "arch", "vim", "1.0", "2.0").unwrap();

        assert_eq!(delta.new_chunks.len(), 2);
        assert_eq!(delta.removed_chunks.len(), 2);
        assert_eq!(delta.download_size, 2500); // chunkC(1200) + chunkD(1300)
    }

    #[test]
    fn test_get_delta_not_found() {
        let (_temp, conn) = create_test_db();

        let result = get_delta(&conn, "fedora", "nonexistent", "1.0", "2.0").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_delta_after_compute() {
        let (_temp, conn) = create_test_db();

        insert_converted(&conn, "fedora", "nginx", "1.0", &["chunkA", "chunkB"], 2000);
        insert_converted(&conn, "fedora", "nginx", "2.0", &["chunkB", "chunkC"], 2500);

        compute_delta(&conn, "fedora", "nginx", "1.0", "2.0").unwrap();

        let cached = get_delta(&conn, "fedora", "nginx", "1.0", "2.0")
            .unwrap()
            .unwrap();

        assert_eq!(cached.from_version, "1.0");
        assert_eq!(cached.to_version, "2.0");
        assert!(cached.new_chunks.contains(&"chunkC".to_string()));
        assert!(cached.removed_chunks.contains(&"chunkA".to_string()));
    }

    #[test]
    fn test_compute_deltas_for_package() {
        let (_temp, conn) = create_test_db();

        insert_converted(&conn, "fedora", "nginx", "1.0", &["chunkA"], 1000);
        insert_converted(&conn, "fedora", "nginx", "2.0", &["chunkA", "chunkB"], 2000);
        insert_converted(&conn, "fedora", "nginx", "3.0", &["chunkB", "chunkC"], 2500);

        let deltas = compute_deltas_for_package(&conn, "fedora", "nginx").unwrap();

        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0].from_version, "1.0");
        assert_eq!(deltas[0].to_version, "2.0");
        assert_eq!(deltas[1].from_version, "2.0");
        assert_eq!(deltas[1].to_version, "3.0");
    }

    #[test]
    fn test_compute_deltas_single_version() {
        let (_temp, conn) = create_test_db();

        insert_converted(&conn, "fedora", "curl", "1.0", &["chunkA"], 1000);

        let deltas = compute_deltas_for_package(&conn, "fedora", "curl").unwrap();
        assert!(deltas.is_empty());
    }

    #[test]
    fn test_delta_response_savings() {
        let delta = DeltaManifest {
            id: Some(1),
            distro: "fedora".to_string(),
            package_name: "nginx".to_string(),
            from_version: "1.0".to_string(),
            to_version: "2.0".to_string(),
            new_chunks: vec!["chunkD".to_string()],
            removed_chunks: vec!["chunkA".to_string()],
            download_size: 1500,
            full_size: 3500,
            computed_at: None,
        };

        let response = delta.to_response();
        assert_eq!(response.from_version, "1.0");
        assert_eq!(response.to_version, "2.0");
        // Savings: (3500 - 1500) / 3500 * 100 = 57.14%
        assert!((response.savings_percent - 57.14).abs() < 0.1);
    }

    #[test]
    fn test_delta_response_zero_full_size() {
        let delta = DeltaManifest {
            id: None,
            distro: "fedora".to_string(),
            package_name: "empty".to_string(),
            from_version: "1.0".to_string(),
            to_version: "2.0".to_string(),
            new_chunks: vec![],
            removed_chunks: vec![],
            download_size: 0,
            full_size: 0,
            computed_at: None,
        };

        let response = delta.to_response();
        assert_eq!(response.savings_percent, 0.0);
    }

    #[test]
    fn test_compute_delta_missing_version() {
        let (_temp, conn) = create_test_db();

        // Only one version exists
        insert_converted(&conn, "fedora", "nginx", "1.0", &["chunkA"], 1000);

        // Compute delta with nonexistent from_version - should succeed with empty from set
        let delta = compute_delta(&conn, "fedora", "nginx", "0.9", "1.0").unwrap();

        assert_eq!(delta.new_chunks.len(), 1);
        assert!(delta.new_chunks.contains(&"chunkA".to_string()));
        assert!(delta.removed_chunks.is_empty());
    }
}
