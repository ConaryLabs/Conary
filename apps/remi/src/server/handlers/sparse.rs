// apps/remi/src/server/handlers/sparse.rs
//! Sparse HTTP index - crates.io-style per-package JSON documents
//!
//! Each package gets a single CDN-cacheable JSON document containing all
//! available versions, conversion status, and metadata. This enables
//! efficient incremental client sync without downloading full indices.

use crate::server::ServerState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::db::models::RepositoryPackage;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// A sparse index entry for a single package across all versions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseIndexEntry {
    pub name: String,
    pub distro: String,
    pub versions: Vec<SparseVersionEntry>,
}

/// Version-level metadata within a sparse index entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseVersionEntry {
    pub version: String,
    pub dependencies: Option<String>,
    pub provides: Option<String>,
    pub architecture: Option<String>,
    pub size: i64,
    pub converted: bool,
    pub content_hash: Option<String>,
}

/// Query parameters for the package list endpoint
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

/// Paginated package list response
#[derive(Debug, Serialize)]
pub struct PackageListResponse {
    pub distro: String,
    pub packages: Vec<String>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

/// GET /v1/index/{distro}/{name}
///
/// Returns a sparse index entry for a single package, including all versions
/// and their conversion status. Designed to be CDN-cacheable.
///
/// When federation is enabled, merges entries from upstream Remi peers.
pub async fn get_sparse_entry(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Response {
    // Validate path parameters against traversal and injection
    if let Err(e) = super::validate_name(&distro) {
        return e;
    }
    if let Err(e) = super::validate_name(&name) {
        return e;
    }

    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    let fed_config = state_guard.federated_config.clone();
    let fed_cache = state_guard.federated_cache.clone();
    let http_client = state_guard.http_client.clone();
    drop(state_guard);

    // Use federated builder if federation is enabled
    if let (Some(config), Some(cache)) = (fed_config, fed_cache) {
        let result = crate::server::federated_index::build_federated_sparse_entry(
            &db_path,
            &distro,
            &name,
            &config,
            &cache,
            &http_client,
        )
        .await;

        return match result {
            Ok(Some(entry)) => {
                let json = match super::serialize_json(&entry, "federated sparse entry") {
                    Ok(j) => j,
                    Err(e) => return e,
                };
                super::json_response(json, 60)
            }
            Ok(None) => (StatusCode::NOT_FOUND, "Package not found").into_response(),
            Err(e) => {
                tracing::error!("Failed to build federated sparse entry: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
            }
        };
    }

    // Non-federated path: local only
    let result =
        tokio::task::spawn_blocking(move || build_sparse_entry(&db_path, &distro, &name)).await;

    match result {
        Ok(Ok(Some(entry))) => {
            let json = match super::serialize_json(&entry, "sparse index entry") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 60)
        }
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "Package not found").into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to build sparse entry: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// GET /v1/index/{distro}?page=1&per_page=100
///
/// Returns a paginated list of package names for a distribution.
pub async fn list_packages(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
    Query(query): Query<ListQuery>,
) -> Response {
    // Validate path parameter against traversal and injection
    if let Err(e) = super::validate_name(&distro) {
        return e;
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(100).clamp(1, 1000);

    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    drop(state_guard);

    let result =
        tokio::task::spawn_blocking(move || build_package_list(&db_path, &distro, page, per_page))
            .await;

    match result {
        Ok(Ok(list)) => {
            let json = match super::serialize_json(&list, "package list") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 60)
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to list packages: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Build a sparse index entry for a specific package, aggregating across all
/// repos for the distro (e.g. arch-core + arch-extra).
fn build_sparse_entry(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> Result<Option<SparseIndexEntry>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find all repositories for this distro
    let repositories = find_repositories_for_distro(&conn, distro)?;
    let repo_ids: Vec<i64> = repositories.into_iter().filter_map(|r| r.id).collect();
    if repo_ids.is_empty() {
        return Ok(None);
    }

    // Get all versions of this package across all matching repositories
    let placeholders: String = (1..=repo_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let name_idx = repo_ids.len() + 1;
    let sql = format!(
        "SELECT id, repository_id, name, version, architecture, description,
                checksum, size, download_url, dependencies, metadata, synced_at,
                is_security_update, severity, cve_ids, advisory_id, advisory_url
         FROM repository_packages
         WHERE repository_id IN ({placeholders}) AND name = ?{name_idx}
         ORDER BY version"
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = repo_ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    params.push(Box::new(name.to_string()));

    let mut stmt = conn.prepare(&sql)?;
    let packages: Vec<RepositoryPackage> = stmt
        .query_map(rusqlite::params_from_iter(&params), |row| {
            Ok(RepositoryPackage {
                id: Some(row.get(0)?),
                repository_id: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                architecture: row.get(4)?,
                description: row.get(5)?,
                checksum: row.get(6)?,
                size: row.get(7)?,
                download_url: row.get(8)?,
                dependencies: row.get(9)?,
                metadata: row.get(10)?,
                synced_at: row.get(11)?,
                is_security_update: row.get::<_, i32>(12)? != 0,
                severity: row.get(13)?,
                cve_ids: row.get(14)?,
                advisory_id: row.get(15)?,
                advisory_url: row.get(16)?,
                distro: None,
                version_scheme: None,
                canonical_id: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if packages.is_empty() {
        return Ok(None);
    }

    // Build converted lookup: version -> (content_hash)
    let mut converted_stmt = conn.prepare(
        "SELECT package_version, content_hash FROM converted_packages
         WHERE distro = ?1 AND package_name = ?2
         AND package_version IS NOT NULL",
    )?;

    let mut converted_map = std::collections::HashMap::new();
    let mut rows = converted_stmt.query(rusqlite::params![distro, name])?;
    while let Some(row) = rows.next()? {
        let version: String = row.get(0)?;
        let content_hash: Option<String> = row.get(1)?;
        converted_map.insert(version, content_hash);
    }

    // Build version entries
    let versions = packages
        .into_iter()
        .map(|pkg| {
            let converted_info = converted_map.get(&pkg.version);
            SparseVersionEntry {
                version: pkg.version,
                dependencies: pkg.dependencies,
                provides: pkg.metadata,
                architecture: pkg.architecture,
                size: pkg.size,
                converted: converted_info.is_some(),
                content_hash: converted_info.and_then(Clone::clone),
            }
        })
        .collect();

    Ok(Some(SparseIndexEntry {
        name: name.to_string(),
        distro: distro.to_string(),
        versions,
    }))
}

/// Build a paginated list of unique package names for a distro, aggregating
/// across all matching repos (e.g. arch-core + arch-extra).
fn build_package_list(
    db_path: &std::path::Path,
    distro: &str,
    page: usize,
    per_page: usize,
) -> Result<PackageListResponse, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find all repositories for this distro
    let repositories = find_repositories_for_distro(&conn, distro)?;
    let repo_ids: Vec<i64> = repositories.into_iter().filter_map(|r| r.id).collect();
    if repo_ids.is_empty() {
        return Ok(PackageListResponse {
            distro: distro.to_string(),
            packages: vec![],
            total: 0,
            page,
            per_page,
        });
    }

    let placeholders: String = (1..=repo_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Count distinct package names across all repos
    let count_sql = format!(
        "SELECT COUNT(DISTINCT name) FROM repository_packages WHERE repository_id IN ({placeholders})"
    );
    let count_params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    let total: usize = conn.query_row(
        &count_sql,
        rusqlite::params_from_iter(&count_params),
        |row| row.get::<_, i64>(0).map(|v| v as usize),
    )?;

    // Get paginated distinct names
    let offset = (page - 1) * per_page;
    let limit_idx = repo_ids.len() + 1;
    let offset_idx = repo_ids.len() + 2;
    let list_sql = format!(
        "SELECT DISTINCT name FROM repository_packages
         WHERE repository_id IN ({placeholders})
         ORDER BY name
         LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );

    let mut list_params: Vec<Box<dyn rusqlite::types::ToSql>> =
        repo_ids.iter().map(|id| Box::new(*id) as _).collect();
    list_params.push(Box::new(per_page as i64));
    list_params.push(Box::new(offset as i64));

    let mut stmt = conn.prepare(&list_sql)?;
    let packages: Vec<String> = stmt
        .query_map(rusqlite::params_from_iter(&list_params), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(PackageListResponse {
        distro: distro.to_string(),
        packages,
        total,
        page,
        per_page,
    })
}

/// Alias to shared implementations in handlers/mod.rs
use super::find_repositories_for_distro;
#[cfg(test)]
use super::find_repository_for_distro;

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{ConvertedPackage, Repository};
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn insert_repo(conn: &Connection, name: &str, distro: &str) -> i64 {
        let mut repo = Repository::new(name.to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some(distro.to_string());
        repo.insert(conn).unwrap()
    }

    fn insert_package(conn: &Connection, repo_id: i64, name: &str, version: &str, size: i64) {
        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            version.to_string(),
            format!("sha256:{name}-{version}"),
            size,
            format!("https://example.com/{name}-{version}.rpm"),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.dependencies = Some(r#"["glibc","openssl"]"#.to_string());
        pkg.insert(conn).unwrap();
    }

    #[test]
    fn test_sparse_entry_not_found_no_repo() {
        let (temp_file, _conn) = create_test_db();
        let result = build_sparse_entry(temp_file.path(), "fedora", "nginx").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_sparse_entry_not_found_no_package() {
        let (temp_file, conn) = create_test_db();
        insert_repo(&conn, "fedora-base", "fedora");

        let result = build_sparse_entry(temp_file.path(), "fedora", "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_sparse_entry_single_version() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0-1.fc43", 1024);

        let entry = build_sparse_entry(temp_file.path(), "fedora", "nginx")
            .unwrap()
            .unwrap();

        assert_eq!(entry.name, "nginx");
        assert_eq!(entry.distro, "fedora");
        assert_eq!(entry.versions.len(), 1);
        assert_eq!(entry.versions[0].version, "1.24.0-1.fc43");
        assert_eq!(entry.versions[0].size, 1024);
        assert_eq!(entry.versions[0].architecture.as_deref(), Some("x86_64"));
        assert!(!entry.versions[0].converted);
        assert!(entry.versions[0].content_hash.is_none());
        assert!(entry.versions[0].dependencies.is_some());
    }

    #[test]
    fn test_sparse_entry_multiple_versions() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.22.0-1.fc43", 900);
        insert_package(&conn, repo_id, "nginx", "1.24.0-1.fc43", 1024);
        insert_package(&conn, repo_id, "nginx", "1.25.0-1.fc43", 1100);

        let entry = build_sparse_entry(temp_file.path(), "fedora", "nginx")
            .unwrap()
            .unwrap();

        assert_eq!(entry.versions.len(), 3);
        // Versions should be sorted by version string
        assert_eq!(entry.versions[0].version, "1.22.0-1.fc43");
        assert_eq!(entry.versions[1].version, "1.24.0-1.fc43");
        assert_eq!(entry.versions[2].version, "1.25.0-1.fc43");
    }

    #[test]
    fn test_sparse_entry_with_conversion() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0-1.fc43", 1024);
        insert_package(&conn, repo_id, "nginx", "1.25.0-1.fc43", 1100);

        // Mark one version as converted
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0-1.fc43".to_string(),
            "rpm".to_string(),
            "sha256:nginx-1.24.0-1.fc43".to_string(),
            "high".to_string(),
            &["chunk1".to_string(), "chunk2".to_string()],
            2048,
            "sha256:content_abc".to_string(),
            "/data/nginx.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let entry = build_sparse_entry(temp_file.path(), "fedora", "nginx")
            .unwrap()
            .unwrap();

        let v1240 = entry
            .versions
            .iter()
            .find(|v| v.version == "1.24.0-1.fc43")
            .unwrap();
        assert!(v1240.converted);
        assert_eq!(v1240.content_hash.as_deref(), Some("sha256:content_abc"));

        let v1250 = entry
            .versions
            .iter()
            .find(|v| v.version == "1.25.0-1.fc43")
            .unwrap();
        assert!(!v1250.converted);
        assert!(v1250.content_hash.is_none());
    }

    #[test]
    fn test_package_list_empty() {
        let (temp_file, _conn) = create_test_db();
        let list = build_package_list(temp_file.path(), "fedora", 1, 100).unwrap();

        assert_eq!(list.distro, "fedora");
        assert!(list.packages.is_empty());
        assert_eq!(list.total, 0);
        assert_eq!(list.page, 1);
        assert_eq!(list.per_page, 100);
    }

    #[test]
    fn test_package_list_with_packages() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);
        insert_package(&conn, repo_id, "curl", "8.5.0", 512);
        insert_package(&conn, repo_id, "zlib", "1.3.0", 256);
        // Add a second version of nginx (should not duplicate in name list)
        insert_package(&conn, repo_id, "nginx", "1.25.0", 1100);

        let list = build_package_list(temp_file.path(), "fedora", 1, 100).unwrap();

        assert_eq!(list.total, 3); // 3 distinct names
        assert_eq!(list.packages.len(), 3);
        assert_eq!(list.packages, vec!["curl", "nginx", "zlib"]);
    }

    #[test]
    fn test_package_list_pagination() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");

        // Insert 5 packages
        for name in &["alpha", "bravo", "charlie", "delta", "echo"] {
            insert_package(&conn, repo_id, name, "1.0.0", 100);
        }

        // Page 1 with per_page=2
        let page1 = build_package_list(temp_file.path(), "fedora", 1, 2).unwrap();
        assert_eq!(page1.total, 5);
        assert_eq!(page1.packages.len(), 2);
        assert_eq!(page1.packages, vec!["alpha", "bravo"]);
        assert_eq!(page1.page, 1);
        assert_eq!(page1.per_page, 2);

        // Page 2
        let page2 = build_package_list(temp_file.path(), "fedora", 2, 2).unwrap();
        assert_eq!(page2.total, 5);
        assert_eq!(page2.packages.len(), 2);
        assert_eq!(page2.packages, vec!["charlie", "delta"]);

        // Page 3 (partial)
        let page3 = build_package_list(temp_file.path(), "fedora", 3, 2).unwrap();
        assert_eq!(page3.total, 5);
        assert_eq!(page3.packages.len(), 1);
        assert_eq!(page3.packages, vec!["echo"]);

        // Page 4 (empty)
        let page4 = build_package_list(temp_file.path(), "fedora", 4, 2).unwrap();
        assert_eq!(page4.total, 5);
        assert!(page4.packages.is_empty());
    }

    #[test]
    fn test_package_list_different_distros() {
        let (temp_file, conn) = create_test_db();
        let fedora_id = insert_repo(&conn, "fedora-base", "fedora");
        let arch_id = insert_repo(&conn, "arch-base", "arch");

        insert_package(&conn, fedora_id, "nginx", "1.24.0", 1024);
        insert_package(&conn, fedora_id, "curl", "8.5.0", 512);
        insert_package(&conn, arch_id, "nginx", "1.25.0", 1100);
        insert_package(&conn, arch_id, "pacman", "6.0.0", 800);

        let fedora_list = build_package_list(temp_file.path(), "fedora", 1, 100).unwrap();
        assert_eq!(fedora_list.total, 2);
        assert_eq!(fedora_list.packages, vec!["curl", "nginx"]);

        let arch_list = build_package_list(temp_file.path(), "arch", 1, 100).unwrap();
        assert_eq!(arch_list.total, 2);
        assert_eq!(arch_list.packages, vec!["nginx", "pacman"]);
    }

    #[test]
    fn test_find_repository_for_distro_by_strategy() {
        let (_temp_file, conn) = create_test_db();
        let mut repo = Repository::new("my-repo".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        repo.insert(&conn).unwrap();

        let found = find_repository_for_distro(&conn, "fedora").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "my-repo");
    }

    #[test]
    fn test_find_repository_for_distro_by_name() {
        let (_temp_file, conn) = create_test_db();
        let mut repo = Repository::new("arch-linux".to_string(), "https://example.com".to_string());
        repo.insert(&conn).unwrap();

        let found = find_repository_for_distro(&conn, "arch").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "arch-linux");
    }

    #[test]
    fn test_find_repository_for_distro_not_found() {
        let (_temp_file, conn) = create_test_db();
        let found = find_repository_for_distro(&conn, "gentoo").unwrap();
        assert!(found.is_none());
    }
}
