// apps/remi/src/server/handlers/index.rs
//! Repository index endpoints - metadata serving

use crate::server::ServerState;
use crate::server::conversion::ScriptletPackageMetadata;
use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
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
    /// Native package release identity, when published by Remi native CCS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Native package architecture from upstream repository metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    /// Whether this package has been converted to CCS
    pub converted: bool,
    /// Dependency names (from native repo metadata)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    /// Additional native metadata, including provides for capability resolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// GET /v1/:distro/metadata
///
/// Returns repository metadata index. Cached by Cloudflare.
pub async fn get_metadata(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || build_metadata(&db_path, &distro)).await;

    match result {
        Ok(Ok(metadata)) => {
            let json = match super::serialize_json(&metadata, "repository metadata") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to build metadata: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build metadata",
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in get_metadata: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Build repository metadata from database
fn build_metadata(
    db_path: &std::path::Path,
    distro: &str,
) -> Result<RepositoryMetadata, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find all repositories for this distro (e.g. arch-core + arch-extra)
    let repositories = find_repositories_for_distro(&conn, distro)?;

    if repositories.is_empty() {
        return Ok(RepositoryMetadata {
            id: format!("conary-{}", distro),
            distro: distro.to_string(),
            last_sync: None,
            package_count: 0,
            converted_count: 0,
            packages: vec![],
        });
    }

    // Use the most recent last_sync across all matching repos
    let last_sync = repositories
        .iter()
        .filter_map(|r| r.last_sync.as_ref())
        .max()
        .cloned();

    // Aggregate packages from all matching repos
    let mut repo_packages = Vec::new();
    for repo in &repositories {
        if let Some(id) = repo.id {
            repo_packages.extend(RepositoryPackage::find_by_repository(&conn, id)?);
        }
    }
    repo_packages.retain(|pkg| pkg.size > 0);

    // Query converted packages once so we can both mark repo-backed entries as
    // converted and surface packages that exist only in Remi's CCS store.
    let converted_packages = load_converted_metadata_rows(&conn, distro)?;
    let converted_set: HashSet<PackageKey> = converted_packages
        .iter()
        .map(|pkg| package_key(&pkg.name, &pkg.version, None, pkg.architecture.as_deref()))
        .collect();
    let converted_scriptlets_by_key: HashMap<PackageKey, ScriptletPackageMetadata> =
        converted_packages
            .iter()
            .map(|pkg| {
                (
                    package_key(&pkg.name, &pkg.version, None, pkg.architecture.as_deref()),
                    pkg.scriptlets.clone(),
                )
            })
            .collect();

    // Build package entries
    let mut packages: Vec<PackageEntry> = repo_packages
        .iter()
        .map(|pkg| {
            let release = non_empty_release(&pkg.package_release);
            let key = package_key(
                &pkg.name,
                &pkg.version,
                release.as_deref(),
                pkg.architecture.as_deref(),
            );
            let dependencies = pkg
                .dependencies
                .as_ref()
                .and_then(|deps_json| serde_json::from_str::<Vec<String>>(deps_json).ok());
            let metadata = pkg.metadata.as_ref().and_then(|metadata_json| {
                serde_json::from_str::<serde_json::Value>(metadata_json).ok()
            });
            let scriptlets = converted_scriptlets_by_key.get(&key);
            PackageEntry {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                release,
                architecture: pkg.architecture.clone(),
                converted: converted_set.contains(&key),
                dependencies,
                metadata: metadata_with_scriptlets(metadata, scriptlets),
            }
        })
        .collect();

    let existing_keys: HashSet<PackageKey> = packages
        .iter()
        .map(|pkg| {
            package_key(
                &pkg.name,
                &pkg.version,
                pkg.release.as_deref(),
                pkg.architecture.as_deref(),
            )
        })
        .collect();
    for converted in converted_packages {
        let key = package_key(
            &converted.name,
            &converted.version,
            None,
            converted.architecture.as_deref(),
        );
        if !existing_keys.contains(&key) {
            packages.push(PackageEntry {
                name: converted.name,
                version: converted.version,
                release: None,
                architecture: converted.architecture,
                converted: true,
                dependencies: None,
                metadata: metadata_with_scriptlets(None, Some(&converted.scriptlets)),
            });
        }
    }

    // Sort by name, then version
    packages.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.release.cmp(&b.release))
    });

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

/// Alias to shared implementation in handlers/mod.rs
use super::find_repositories_for_distro;

type PackageKey = (String, String, Option<String>, Option<String>);

fn package_key(
    name: &str,
    version: &str,
    release: Option<&str>,
    architecture: Option<&str>,
) -> PackageKey {
    (
        name.to_string(),
        version.to_string(),
        release.map(str::to_string),
        architecture.map(str::to_string),
    )
}

fn non_empty_release(release: &str) -> Option<String> {
    (!release.is_empty()).then(|| release.to_string())
}

#[derive(Debug, Clone)]
struct ConvertedMetadataRow {
    name: String,
    version: String,
    architecture: Option<String>,
    scriptlets: ScriptletPackageMetadata,
}

/// Build converted package entries for this distro.
#[cfg(test)]
fn build_converted_packages(
    conn: &Connection,
    distro: &str,
) -> Result<Vec<PackageEntry>, anyhow::Error> {
    Ok(load_converted_metadata_rows(conn, distro)?
        .into_iter()
        .map(|row| PackageEntry {
            name: row.name,
            version: row.version,
            release: None,
            architecture: row.architecture,
            converted: true,
            dependencies: None,
            metadata: metadata_with_scriptlets(None, Some(&row.scriptlets)),
        })
        .collect())
}

fn load_converted_metadata_rows(
    conn: &Connection,
    distro: &str,
) -> Result<Vec<ConvertedMetadataRow>, anyhow::Error> {
    let mut packages = Vec::new();
    for converted in ConvertedPackage::find_publication_candidates(conn, distro, None)? {
        let Some(name) = converted.package_name.clone() else {
            continue;
        };
        let Some(version) = converted.package_version.clone() else {
            continue;
        };
        let architecture = converted.package_architecture.clone();

        // Pre-architecture Remi conversion records cannot be addressed safely
        // once native metadata has multilib packages and epoch-aware versions.
        // Keep uploaded CCS fixtures visible, but do not advertise ambiguous
        // repo-derived conversions as installable repository metadata.
        if architecture.is_none() && converted.original_format != "ccs" {
            continue;
        }

        if !converted.is_scriptlet_public_ready() {
            continue;
        }

        packages.push(ConvertedMetadataRow {
            name,
            version,
            architecture,
            scriptlets: ScriptletPackageMetadata::from(&converted.scriptlet_summary()),
        });
    }

    Ok(packages)
}

fn metadata_with_scriptlets(
    metadata: Option<serde_json::Value>,
    scriptlets: Option<&ScriptletPackageMetadata>,
) -> Option<serde_json::Value> {
    let Some(scriptlets) = scriptlets else {
        return metadata;
    };
    let scriptlets = serde_json::to_value(scriptlets).ok()?;
    match metadata {
        Some(serde_json::Value::Object(mut object)) => {
            object.insert("scriptlets".to_string(), scriptlets);
            Some(serde_json::Value::Object(object))
        }
        Some(existing) => {
            let mut object = serde_json::Map::new();
            object.insert("native".to_string(), existing);
            object.insert("scriptlets".to_string(), scriptlets);
            Some(serde_json::Value::Object(object))
        }
        None => {
            let mut object = serde_json::Map::new();
            object.insert("scriptlets".to_string(), scriptlets);
            Some(serde_json::Value::Object(object))
        }
    }
}

/// GET /v1/:distro/metadata.sig
///
/// Returns GPG signature for repository metadata.
pub async fn get_metadata_sig(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
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
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) => {
            tracing::error!("Failed to read signature: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read signature",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::handlers::find_repository_for_distro;
    use crate::server::native_publish::test_support::seed_native_publication;
    use conary_core::ccs::convert::ScriptletBundleSummary;
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

    fn insert_converted_with_summary(
        conn: &Connection,
        distro: &str,
        package: &str,
        version: &str,
        architecture: Option<&str>,
        original_format: &str,
        summary: ScriptletBundleSummary,
    ) {
        let mut converted = ConvertedPackage::new_server(
            distro.to_string(),
            package.to_string(),
            version.to_string(),
            original_format.to_string(),
            format!("sha256:{package}-{version}-source"),
            "high".to_string(),
            &[format!("sha256:{package}-{version}-chunk")],
            42,
            format!("sha256:{package}-{version}-content"),
            format!("/tmp/{package}-{version}.ccs"),
        );
        converted.package_architecture = architecture.map(str::to_string);
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(conn).unwrap();
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
        let mut repo =
            Repository::new("fedora-base".to_string(), "https://example.com".to_string());
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
    fn metadata_includes_native_only_package_as_native_not_converted() {
        let (temp_file, conn) = create_test_db();
        seed_native_publication(
            &conn,
            "fedora",
            "hello",
            "1.0.0",
            "1",
            "noarch",
            "/tmp/hello.ccs",
        );

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
        let hello = metadata
            .packages
            .iter()
            .find(|pkg| pkg.name == "hello")
            .unwrap();

        assert_eq!(hello.version, "1.0.0");
        assert_eq!(hello.release.as_deref(), Some("1"));
        assert!(!hello.converted);
        assert_eq!(
            hello.metadata.as_ref().unwrap()["source_kind"],
            "native-ccs"
        );
    }

    #[test]
    fn native_row_not_filtered_by_conversion_publication_gate() {
        let (temp_file, conn) = create_test_db();
        seed_native_publication(
            &conn,
            "fedora",
            "hello",
            "1.0.0",
            "1",
            "noarch",
            "/tmp/hello.ccs",
        );

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
        let hello = metadata
            .packages
            .iter()
            .find(|pkg| pkg.name == "hello")
            .unwrap();

        assert!(!hello.converted);
        assert_eq!(
            hello.metadata.as_ref().unwrap()["source_kind"],
            "native-ccs"
        );
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
            "1.24.0-1.fc44".to_string(),
            "sha256:abc".to_string(),
            1024,
            "https://example.com/nginx.rpm".to_string(),
        );
        pkg1.architecture = Some("x86_64".to_string());
        pkg1.insert(&conn).unwrap();

        let mut pkg2 = RepositoryPackage::new(
            repo_id,
            "curl".to_string(),
            "8.5.0-1.fc44".to_string(),
            "sha256:def".to_string(),
            512,
            "https://example.com/curl.rpm".to_string(),
        );
        pkg2.insert(&conn).unwrap();

        let mut pkg3 = RepositoryPackage::new(
            repo_id,
            "zlib".to_string(),
            "1.3.1-1.fc44".to_string(),
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
    fn test_build_metadata_preserves_repository_architecture() {
        let (temp_file, conn) = create_test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "qemu-img".to_string(),
            "2:10.1.0-7.fc44".to_string(),
            "sha256:qemu-img".to_string(),
            4096,
            "https://example.com/qemu-img.rpm".to_string(),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
        let qemu_img = metadata
            .packages
            .iter()
            .find(|p| p.name == "qemu-img")
            .unwrap();
        let serialized = serde_json::to_value(qemu_img).unwrap();

        assert_eq!(serialized["architecture"], "x86_64");
    }

    #[test]
    fn test_build_metadata_ignores_zero_sized_repository_rows() {
        let (temp_file, conn) = create_test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut placeholder = RepositoryPackage::new(
            repo_id,
            "qemu-img".to_string(),
            "10.1.0-7.fc44".to_string(),
            "sha256:placeholder".to_string(),
            0,
            "".to_string(),
        );
        placeholder.architecture = Some("x86_64".to_string());
        placeholder.insert(&conn).unwrap();

        let mut real_package = RepositoryPackage::new(
            repo_id,
            "qemu-img".to_string(),
            "2:10.1.0-7.fc44".to_string(),
            "sha256:qemu-img".to_string(),
            4096,
            "https://example.com/qemu-img.rpm".to_string(),
        );
        real_package.architecture = Some("x86_64".to_string());
        real_package.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.package_count, 1);
        assert!(
            metadata
                .packages
                .iter()
                .any(|p| p.name == "qemu-img" && p.version == "2:10.1.0-7.fc44")
        );
        assert!(
            !metadata
                .packages
                .iter()
                .any(|p| p.name == "qemu-img" && p.version == "10.1.0-7.fc44")
        );
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
            "1.24.0-1.fc44".to_string(),
            "sha256:abc".to_string(),
            1024,
            "https://example.com/nginx.rpm".to_string(),
        );
        pkg1.architecture = Some("x86_64".to_string());
        pkg1.insert(&conn).unwrap();

        let mut pkg2 = RepositoryPackage::new(
            repo_id,
            "curl".to_string(),
            "8.5.0-1.fc44".to_string(),
            "sha256:def".to_string(),
            512,
            "https://example.com/curl.rpm".to_string(),
        );
        pkg2.insert(&conn).unwrap();

        // Mark nginx as converted
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0-1.fc44".to_string(),
            "rpm".to_string(),
            "sha256:abc".to_string(),
            "high".to_string(),
            &["chunk1".to_string(), "chunk2".to_string()],
            2048,
            "sha256:content".to_string(),
            "/path/to/nginx.ccs".to_string(),
        );
        converted.package_architecture = Some("x86_64".to_string());
        converted.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.package_count, 2);
        assert_eq!(metadata.converted_count, 1);

        // curl is not converted
        let curl = metadata.packages.iter().find(|p| p.name == "curl").unwrap();
        assert!(!curl.converted);

        // nginx is converted
        let nginx = metadata
            .packages
            .iter()
            .find(|p| p.name == "nginx")
            .unwrap();
        assert!(nginx.converted);
    }

    #[test]
    fn test_build_metadata_with_converted_only_package() {
        let (temp_file, conn) = create_test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        repo.insert(&conn).unwrap();

        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "conary-test-fixture".to_string(),
            "1.0.0".to_string(),
            "ccs".to_string(),
            "upload:fedora:fixture".to_string(),
            "full".to_string(),
            &["fixture".to_string()],
            1277,
            "fixture-hash".to_string(),
            "/conary/cache/packages/conary-test-fixture-1.0.0.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert_eq!(metadata.package_count, 1);
        assert_eq!(metadata.converted_count, 1);

        let fixture = metadata
            .packages
            .iter()
            .find(|p| p.name == "conary-test-fixture")
            .unwrap();
        assert_eq!(fixture.version, "1.0.0");
        assert!(fixture.converted);
        assert!(fixture.dependencies.is_none());
    }

    #[test]
    fn metadata_merges_public_scriptlet_metadata_for_repo_backed_and_converted_only_rows() {
        let (temp_file, conn) = create_test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut repo_backed_pkg = RepositoryPackage::new(
            repo_id,
            "repo-backed".to_string(),
            "1.0".to_string(),
            "sha256:repo-backed".to_string(),
            2048,
            "https://example.com/repo-backed.rpm".to_string(),
        );
        repo_backed_pkg.architecture = Some("x86_64".to_string());
        repo_backed_pkg.insert(&conn).unwrap();

        let mut unconverted_pkg = RepositoryPackage::new(
            repo_id,
            "unconverted".to_string(),
            "1.0".to_string(),
            "sha256:unconverted".to_string(),
            1024,
            "https://example.com/unconverted.rpm".to_string(),
        );
        unconverted_pkg.insert(&conn).unwrap();

        let mut repo_backed = ConvertedPackage::new_server(
            "fedora".to_string(),
            "repo-backed".to_string(),
            "1.0".to_string(),
            "rpm".to_string(),
            "sha256:repo-backed".to_string(),
            "high".to_string(),
            &["sha256:repo-backed-chunk".to_string()],
            2048,
            "sha256:repo-backed-content".to_string(),
            "/cache/repo-backed.ccs".to_string(),
        );
        repo_backed.package_architecture = Some("x86_64".to_string());
        repo_backed
            .set_scriptlet_metadata(&ScriptletBundleSummary {
                scriptlet_fidelity: "fully-replaced".to_string(),
                target_compatibility: "fully-compatible".to_string(),
                publication_status: "public".to_string(),
                review_artifact_path: Some("/tmp/private-review-secret".to_string()),
                ..ScriptletBundleSummary::default()
            })
            .unwrap();
        repo_backed.insert(&conn).unwrap();

        let mut converted_only = ConvertedPackage::new_server(
            "fedora".to_string(),
            "converted-only".to_string(),
            "2.0".to_string(),
            "ccs".to_string(),
            "upload:fedora:converted-only".to_string(),
            "full".to_string(),
            &["sha256:converted-only-chunk".to_string()],
            4096,
            "sha256:converted-only-content".to_string(),
            "/cache/converted-only.ccs".to_string(),
        );
        converted_only
            .set_scriptlet_metadata(&ScriptletBundleSummary {
                scriptlet_fidelity: "fully-replaced".to_string(),
                target_compatibility: "fully-compatible".to_string(),
                publication_status: "public".to_string(),
                review_artifact_path: Some("/tmp/private-review-secret".to_string()),
                ..ScriptletBundleSummary::default()
            })
            .unwrap();
        converted_only.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        let repo_backed = metadata
            .packages
            .iter()
            .find(|pkg| pkg.name == "repo-backed")
            .unwrap();
        assert!(repo_backed.converted);
        let repo_backed_scriptlets = repo_backed
            .metadata
            .as_ref()
            .unwrap()
            .get("scriptlets")
            .unwrap();
        assert_eq!(
            repo_backed_scriptlets
                .get("scriptlet_fidelity")
                .and_then(serde_json::Value::as_str),
            Some("fully-replaced")
        );
        let repo_backed_json = serde_json::to_string(repo_backed).unwrap();
        assert!(!repo_backed_json.contains("review_artifact_path"));
        assert!(!repo_backed_json.contains("private-review-secret"));

        let converted_only = metadata
            .packages
            .iter()
            .find(|pkg| pkg.name == "converted-only")
            .unwrap();
        assert!(converted_only.converted);
        let converted_only_scriptlets = converted_only
            .metadata
            .as_ref()
            .unwrap()
            .get("scriptlets")
            .unwrap();
        assert_eq!(
            converted_only_scriptlets
                .get("scriptlet_fidelity")
                .and_then(serde_json::Value::as_str),
            Some("fully-replaced")
        );
        let converted_only_json = serde_json::to_string(converted_only).unwrap();
        assert!(!converted_only_json.contains("review_artifact_path"));
        assert!(!converted_only_json.contains("private-review-secret"));

        let unconverted = metadata
            .packages
            .iter()
            .find(|pkg| pkg.name == "unconverted")
            .unwrap();
        assert!(!unconverted.converted);
        assert!(unconverted.metadata.is_none());
    }

    #[test]
    fn metadata_hides_non_public_scriptlet_rows() {
        let (temp_file, conn) = create_test_db();
        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "gtk3".to_string(),
            "3.24.0".to_string(),
            "sha256:repo".to_string(),
            1024,
            "https://example.com/gtk3.rpm".to_string(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        repo_pkg.insert(&conn).unwrap();

        insert_converted_with_summary(
            &conn,
            "fedora",
            "gtk3",
            "3.24.0",
            Some("x86_64"),
            "rpm",
            ScriptletBundleSummary {
                publication_status: "private-review".to_string(),
                scriptlet_fidelity: "review-required".to_string(),
                target_compatibility: "review-required".to_string(),
                review_reason_codes: vec!["review-class-debconf".to_string()],
                ..Default::default()
            },
        );

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
        let pkg = metadata
            .packages
            .iter()
            .find(|pkg| pkg.name == "gtk3")
            .unwrap();

        assert!(!pkg.converted);
        assert_eq!(metadata.converted_count, 0);
        assert!(
            pkg.metadata
                .as_ref()
                .and_then(|value| value.get("scriptlets"))
                .is_none()
        );
    }

    #[test]
    fn metadata_omits_converted_only_non_public_rows() {
        let (temp_file, conn) = create_test_db();
        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        repo.insert(&conn).unwrap();

        insert_converted_with_summary(
            &conn,
            "fedora",
            "private-only",
            "1.0",
            Some("x86_64"),
            "ccs",
            ScriptletBundleSummary {
                publication_status: "blocked".to_string(),
                scriptlet_fidelity: "blocked".to_string(),
                target_compatibility: "blocked".to_string(),
                blocked_reason_codes: vec!["blocked-class-network".to_string()],
                ..Default::default()
            },
        );

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert!(
            metadata
                .packages
                .iter()
                .all(|pkg| pkg.name != "private-only")
        );
        assert_eq!(metadata.converted_count, 0);
    }

    #[test]
    fn test_build_metadata_omits_legacy_repo_converted_only_without_architecture() {
        let (temp_file, conn) = create_test_db();

        let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some("fedora".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "qemu-img".to_string(),
            "2:10.1.0-7.fc44".to_string(),
            "sha256:qemu-img".to_string(),
            4096,
            "https://example.com/qemu-img.rpm".to_string(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        repo_pkg.insert(&conn).unwrap();

        let mut stale_converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "qemu-img".to_string(),
            "10.1.0-7.fc44".to_string(),
            "rpm".to_string(),
            "sha256:old-qemu-img".to_string(),
            "high".to_string(),
            &["chunk".to_string()],
            2048,
            "sha256:content".to_string(),
            "/cache/qemu-img-10.1.0-7.fc44.ccs".to_string(),
        );
        stale_converted.insert(&conn).unwrap();

        let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

        assert!(
            metadata
                .packages
                .iter()
                .any(|p| p.name == "qemu-img" && p.version == "2:10.1.0-7.fc44")
        );
        assert!(
            !metadata
                .packages
                .iter()
                .any(|p| p.name == "qemu-img" && p.version == "10.1.0-7.fc44")
        );
    }

    #[test]
    fn test_find_repository_by_strategy_distro() {
        let (_temp_file, conn) = create_test_db();

        // Create repo with default_strategy_distro
        let mut repo = Repository::new(
            "my-fedora-repo".to_string(),
            "https://example.com".to_string(),
        );
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
        let mut repo1 = Repository::new(
            "debian-old".to_string(),
            "https://old.example.com".to_string(),
        );
        repo1.insert(&conn).unwrap();

        let mut repo2 = Repository::new(
            "my-deb-repo".to_string(),
            "https://new.example.com".to_string(),
        );
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
    fn test_build_converted_packages() {
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
        fedora_pkg.package_architecture = Some("x86_64".to_string());
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
        arch_pkg.package_architecture = Some("x86_64".to_string());
        arch_pkg.insert(&conn).unwrap();

        // Query for fedora - should only get fedora packages
        let fedora_set = build_converted_packages(&conn, "fedora").unwrap();
        assert_eq!(fedora_set.len(), 1);
        assert_eq!(fedora_set[0].name, "nginx");
        assert_eq!(fedora_set[0].version, "1.24.0");
        assert!(fedora_set[0].converted);

        // Query for arch - should only get arch packages
        let arch_set = build_converted_packages(&conn, "arch").unwrap();
        assert_eq!(arch_set.len(), 1);
        assert_eq!(arch_set[0].name, "nginx");
        assert_eq!(arch_set[0].version, "1.24.0");

        // Query for ubuntu - should be empty
        let ubuntu_set = build_converted_packages(&conn, "ubuntu").unwrap();
        assert!(ubuntu_set.is_empty());
    }

    #[test]
    fn test_build_converted_packages_ignores_null_fields() {
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
        server_pkg.package_architecture = Some("x86_64".to_string());
        server_pkg.insert(&conn).unwrap();

        let set = build_converted_packages(&conn, "fedora").unwrap();

        // Should only include the server-side package with non-null fields
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].name, "curl");
        assert_eq!(set[0].version, "8.5.0");
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
            ("zlib", "1.2.0"), // older version
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
