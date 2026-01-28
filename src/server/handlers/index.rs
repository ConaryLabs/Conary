// src/server/handlers/index.rs
//! Repository index endpoints - metadata serving

use crate::db::models::{Repository, RepositoryPackage};
use crate::server::ServerState;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Repository metadata response
#[derive(Serialize)]
pub struct RepositoryMetadata {
    /// Repository identifier
    pub id: String,
    /// Distribution name
    pub distro: String,
    /// Last sync timestamp (ISO 8601)
    pub last_sync: Option<String>,
    /// Number of packages available
    pub package_count: usize,
    /// Number of packages already converted to CCS
    pub converted_count: usize,
    /// List of available packages (names only for index)
    pub packages: Vec<PackageEntry>,
}

#[derive(Serialize)]
pub struct PackageEntry {
    pub name: String,
    pub version: String,
    /// Whether this package has been converted to CCS
    pub converted: bool,
}

/// GET /v1/:distro/metadata
///
/// Returns repository metadata index. Cached by Cloudflare.
pub async fn get_metadata(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    // Validate distro
    if !["arch", "fedora", "ubuntu", "debian"].contains(&distro.as_str()) {
        return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response();
    }

    let state = state.read().await;
    let db_path = &state.config.db_path;

    // Query repository metadata
    match build_metadata(db_path, &distro) {
        Ok(metadata) => {
            // Cache for 5 minutes (Cloudflare will cache this)
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(axum::body::Body::from(
                    serde_json::to_string(&metadata).unwrap(),
                ))
                .unwrap()
        }
        Err(e) => {
            tracing::error!("Failed to build metadata for {}: {}", distro, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build metadata").into_response()
        }
    }
}

/// Build repository metadata from database
fn build_metadata(
    db_path: &std::path::Path,
    distro: &str,
) -> Result<RepositoryMetadata, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find repository for this distro
    // Try by default_strategy_distro first, then by name pattern
    let repository = find_repository_for_distro(&conn, distro)?;

    let (repo_id, last_sync) = match repository {
        Some(repo) => (repo.id, repo.last_sync),
        None => {
            // No repository configured for this distro - return empty metadata
            return Ok(RepositoryMetadata {
                id: format!("conary-{}", distro),
                distro: distro.to_string(),
                last_sync: None,
                package_count: 0,
                converted_count: 0,
                packages: vec![],
            });
        }
    };

    // Get all packages from the repository
    let repo_packages = if let Some(id) = repo_id {
        RepositoryPackage::find_by_repository(&conn, id)?
    } else {
        vec![]
    };

    // Build a set of converted package identities for fast lookup
    let converted_set = build_converted_set(&conn, distro)?;

    // Build package entries
    let mut packages: Vec<PackageEntry> = repo_packages
        .iter()
        .map(|pkg| {
            let key = format!("{}:{}", pkg.name, pkg.version);
            PackageEntry {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                converted: converted_set.contains(&key),
            }
        })
        .collect();

    // Sort by name, then version
    packages.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));

    let converted_count = packages.iter().filter(|p| p.converted).count();

    Ok(RepositoryMetadata {
        id: format!("conary-{}", distro),
        distro: distro.to_string(),
        last_sync,
        package_count: packages.len(),
        converted_count,
        packages,
    })
}

/// Find a repository configured for the given distro
fn find_repository_for_distro(
    conn: &Connection,
    distro: &str,
) -> Result<Option<Repository>, anyhow::Error> {
    // First, try to find by default_strategy_distro
    let repos = Repository::list_enabled(conn)?;

    for repo in &repos {
        if repo.default_strategy_distro.as_deref() == Some(distro) {
            return Ok(Some(repo.clone()));
        }
    }

    // Fall back to name-based matching (e.g., "fedora", "fedora-updates")
    for repo in &repos {
        if repo.name.starts_with(distro) || repo.name.contains(distro) {
            return Ok(Some(repo.clone()));
        }
    }

    Ok(None)
}

/// Build a set of "name:version" keys for converted packages
fn build_converted_set(
    conn: &Connection,
    distro: &str,
) -> Result<HashSet<String>, anyhow::Error> {
    // Query converted_packages for this distro
    let mut stmt = conn.prepare(
        "SELECT package_name, package_version FROM converted_packages
         WHERE distro = ?1 AND package_name IS NOT NULL AND package_version IS NOT NULL",
    )?;

    let mut set = HashSet::new();
    let mut rows = stmt.query([distro])?;

    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let version: String = row.get(1)?;
        set.insert(format!("{}:{}", name, version));
    }

    Ok(set)
}

/// GET /v1/:distro/metadata.sig
///
/// Returns GPG signature for repository metadata.
pub async fn get_metadata_sig(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    // Validate distro
    if !["arch", "fedora", "ubuntu", "debian"].contains(&distro.as_str()) {
        return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response();
    }

    let state = state.read().await;
    let sig_path = state
        .config
        .chunk_dir
        .parent()
        .unwrap_or(&state.config.chunk_dir)
        .join("repo")
        .join(&distro)
        .join("metadata.json.sig");

    if !sig_path.exists() {
        return (StatusCode::NOT_FOUND, "Signature not found").into_response();
    }

    match tokio::fs::read(&sig_path).await {
        Ok(sig) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/pgp-signature")
            .header(header::CACHE_CONTROL, "public, max-age=300")
            .body(axum::body::Body::from(sig))
            .unwrap(),
        Err(e) => {
            tracing::error!("Failed to read signature: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read signature").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::ConvertedPackage;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_build_metadata_no_repository() {
        let (temp_file, _conn) = create_test_db();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.id, "conary-fedora");
        assert_eq!(metadata.distro, "fedora");
        assert!(metadata.last_sync.is_none());
        assert_eq!(metadata.package_count, 0);
        assert_eq!(metadata.converted_count, 0);
        assert!(metadata.packages.is_empty());
    }

    #[test]
    fn test_build_metadata_empty_repository() {
        let (temp_file, conn) = create_test_db();

        // Create a repository with default_strategy_distro
        let mut repo = Repository::new("fedora-base".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        repo.insert(&conn).unwrap();

        // Update with last_sync (not set during insert)
        repo.last_sync = Some("2026-01-21T12:00:00Z".to_string());
        repo.update(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.distro, "fedora");
        assert_eq!(metadata.last_sync.as_deref(), Some("2026-01-21T12:00:00Z"));
        assert_eq!(metadata.package_count, 0);
        assert_eq!(metadata.converted_count, 0);
    }

    #[test]
    fn test_build_metadata_with_packages() {
        let (temp_file, conn) = create_test_db();

        // Create repository
        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        // Add some packages
        let mut pkg1 = RepositoryPackage::new(
            repo_id,
            "nginx".to_string(),
            "1.24.0-1.fc43".to_string(),
            "sha256:abc".to_string(),
            1024,
            "https://example.com/nginx.rpm".to_string(),
        );
        pkg1.insert(&conn).unwrap();

        let mut pkg2 = RepositoryPackage::new(
            repo_id,
            "curl".to_string(),
            "8.5.0-1.fc43".to_string(),
            "sha256:def".to_string(),
            512,
            "https://example.com/curl.rpm".to_string(),
        );
        pkg2.insert(&conn).unwrap();

        let mut pkg3 = RepositoryPackage::new(
            repo_id,
            "zlib".to_string(),
            "1.3.1-1.fc43".to_string(),
            "sha256:ghi".to_string(),
            256,
            "https://example.com/zlib.rpm".to_string(),
        );
        pkg3.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.package_count, 3);
        assert_eq!(metadata.converted_count, 0);

        // Verify sorted by name
        assert_eq!(metadata.packages[0].name, "curl");
        assert_eq!(metadata.packages[1].name, "nginx");
        assert_eq!(metadata.packages[2].name, "zlib");
    }

    #[test]
    fn test_build_metadata_with_converted_packages() {
        let (temp_file, conn) = create_test_db();

        // Create repository
        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        // Add packages
        let mut pkg1 = RepositoryPackage::new(
            repo_id,
            "nginx".to_string(),
            "1.24.0-1.fc43".to_string(),
            "sha256:abc".to_string(),
            1024,
            "https://example.com/nginx.rpm".to_string(),
        );
        pkg1.insert(&conn).unwrap();

        let mut pkg2 = RepositoryPackage::new(
            repo_id,
            "curl".to_string(),
            "8.5.0-1.fc43".to_string(),
            "sha256:def".to_string(),
            512,
            "https://example.com/curl.rpm".to_string(),
        );
        pkg2.insert(&conn).unwrap();

        // Mark nginx as converted
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0-1.fc43".to_string(),
            "rpm".to_string(),
            "sha256:abc".to_string(),
            "high".to_string(),
            &["chunk1".to_string(), "chunk2".to_string()],
            2048,
            "sha256:content".to_string(),
            "/path/to/nginx.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.package_count, 2);
        assert_eq!(metadata.converted_count, 1);

        // curl is not converted
        let curl = metadata.packages.iter().find(|p| p.name == "curl").unwrap();
        assert!(!curl.converted);

        // nginx is converted
        let nginx = metadata.packages.iter().find(|p| p.name == "nginx").unwrap();
        assert!(nginx.converted);
    }

    #[test]
    fn test_find_repository_by_strategy_distro() {
        let (_temp_file, conn) = create_test_db();

        // Create repo with default_strategy_distro
        let mut repo = Repository::new("my-fedora-repo".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        repo.insert(&conn).unwrap();

        let found = find_repository_for_distro(&conn, "fedora").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "my-fedora-repo");
    }

    #[test]
    fn test_find_repository_by_name_pattern() {
        let (_temp_file, conn) = create_test_db();

        // Create repo without default_strategy_distro but with matching name
        let mut repo = Repository::new("arch-linux".to_string(), "https://example.com".to_string());
        repo.insert(&conn).unwrap();

        let found = find_repository_for_distro(&conn, "arch").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "arch-linux");
    }

    #[test]
    fn test_find_repository_prefers_strategy_distro() {
        let (_temp_file, conn) = create_test_db();

        // Create two repos - one with matching name, one with matching strategy_distro
        let mut repo1 = Repository::new("debian-old".to_string(), "https://old.example.com".to_string());
        repo1.insert(&conn).unwrap();

        let mut repo2 = Repository::new("my-deb-repo".to_string(), "https://new.example.com".to_string());
        repo2.default_strategy_distro = Some("debian".to_string());
        repo2.insert(&conn).unwrap();

        // Should prefer the one with matching strategy_distro
        let found = find_repository_for_distro(&conn, "debian").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "my-deb-repo");
    }

    #[test]
    fn test_find_repository_not_found() {
        let (_temp_file, conn) = create_test_db();

        // Create unrelated repo
        let mut repo = Repository::new("centos".to_string(), "https://example.com".to_string());
        repo.insert(&conn).unwrap();

        let found = find_repository_for_distro(&conn, "fedora").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_build_converted_set() {
        let (_temp_file, conn) = create_test_db();

        // Add converted packages for different distros
        let mut fedora_pkg = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0".to_string(),
            "rpm".to_string(),
            "sha256:fed1".to_string(),
            "high".to_string(),
            &[],
            1024,
            "sha256:c1".to_string(),
            "/path/1.ccs".to_string(),
        );
        fedora_pkg.insert(&conn).unwrap();

        let mut arch_pkg = ConvertedPackage::new_server(
            "arch".to_string(),
            "nginx".to_string(),
            "1.24.0".to_string(),
            "arch".to_string(),
            "sha256:arch1".to_string(),
            "high".to_string(),
            &[],
            1024,
            "sha256:c2".to_string(),
            "/path/2.ccs".to_string(),
        );
        arch_pkg.insert(&conn).unwrap();

        // Query for fedora - should only get fedora packages
        let fedora_set = build_converted_set(&conn, "fedora").unwrap();
        assert_eq!(fedora_set.len(), 1);
        assert!(fedora_set.contains("nginx:1.24.0"));

        // Query for arch - should only get arch packages
        let arch_set = build_converted_set(&conn, "arch").unwrap();
        assert_eq!(arch_set.len(), 1);
        assert!(arch_set.contains("nginx:1.24.0"));

        // Query for ubuntu - should be empty
        let ubuntu_set = build_converted_set(&conn, "ubuntu").unwrap();
        assert!(ubuntu_set.is_empty());
    }

    #[test]
    fn test_build_converted_set_ignores_null_fields() {
        let (_temp_file, conn) = create_test_db();

        // Add a client-side converted package (no package_name/version/distro)
        let mut client_pkg = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:client".to_string(),
            "high".to_string(),
        );
        client_pkg.insert(&conn).unwrap();

        // Add a server-side converted package
        let mut server_pkg = ConvertedPackage::new_server(
            "fedora".to_string(),
            "curl".to_string(),
            "8.5.0".to_string(),
            "rpm".to_string(),
            "sha256:server".to_string(),
            "high".to_string(),
            &[],
            512,
            "sha256:c".to_string(),
            "/path/curl.ccs".to_string(),
        );
        server_pkg.insert(&conn).unwrap();

        let set = build_converted_set(&conn, "fedora").unwrap();

        // Should only include the server-side package with non-null fields
        assert_eq!(set.len(), 1);
        assert!(set.contains("curl:8.5.0"));
    }

    #[test]
    fn test_metadata_package_sorting() {
        let (temp_file, conn) = create_test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        // Add packages in non-alphabetical order
        for (name, version) in [
            ("zlib", "1.3.0"),
            ("acl", "2.3.2"),
            ("zlib", "1.2.0"),  // older version
            ("bash", "5.2.0"),
        ] {
            let mut pkg = RepositoryPackage::new(
                repo_id,
                name.to_string(),
                version.to_string(),
                format!("sha256:{name}{version}"),
                100,
                format!("https://example.com/{name}.rpm"),
            );
            pkg.insert(&conn).unwrap();
        }

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        // Verify sorted by name, then version
        assert_eq!(metadata.packages[0].name, "acl");
        assert_eq!(metadata.packages[1].name, "bash");
        assert_eq!(metadata.packages[2].name, "zlib");
        assert_eq!(metadata.packages[2].version, "1.2.0"); // earlier version first (lexicographic)
        assert_eq!(metadata.packages[3].name, "zlib");
        assert_eq!(metadata.packages[3].version, "1.3.0");
    }
}
