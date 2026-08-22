// apps/remi/src/server/index_gen.rs
//! Repository index generation for Remi
//!
//! Generates a JSON index of all converted packages for a distribution.
//! The index is used by clients to discover available packages without
//! querying the server for each package individually.

use crate::server::conversion::ScriptletPackageMetadata;
use crate::server::handlers::human_bytes;
use anyhow::{Context, Result};
use chrono::Utc;
use conary_core::db::models::{ConvertedPackage, RemiActiveProfileRevision};
use conary_core::repository::catalog::CatalogPackageRecordV1;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::catalog_authority::{CatalogAuthority, PinnedProfileCatalog};
use super::database_writer::DatabaseWriter;
use super::profile_catalog::ProfileCatalog;

/// Repository index containing all converted packages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    /// Index format version
    pub version: u32,
    /// Exact public source profile.
    pub source_profile: String,
    /// When the index was generated (ISO 8601)
    pub generated_at: String,
    /// Total number of packages
    pub package_count: usize,
    /// Total number of chunks in the store
    pub chunk_count: usize,
    /// Total size of all chunks in bytes
    pub total_chunk_bytes: u64,
    /// Human-readable total size
    pub total_size_human: String,
    /// Packages indexed by name
    pub packages: HashMap<String, PackageIndexEntry>,
}

/// Entry for a single package in the index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageIndexEntry {
    /// Package name
    pub name: String,
    /// Available versions (newest first)
    pub versions: Vec<VersionEntry>,
}

/// Entry for a single version of a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    /// Version string
    pub version: String,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Typed aggregate scriptlet fidelity.
    pub scriptlet_fidelity: String,
    /// When this version was converted
    pub converted_at: Option<String>,
    /// SHA-256 checksum of original package
    pub original_checksum: String,
    /// Passive native lifecycle metadata for converted package versions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scriptlets: Option<ScriptletPackageMetadata>,
}

/// Index generation configuration
pub struct IndexGenConfig {
    /// Path to the database
    pub db_path: String,
    /// Path to the chunk store
    pub chunk_dir: String,
    /// Root containing immutable activated source and profile catalogs.
    pub catalog_dir: String,
    /// Output directory for index files
    pub output_dir: String,
    /// Exact public source profile to generate (None = all)
    pub source_profile: Option<String>,
    /// Path to signing key (optional)
    pub sign_key: Option<String>,
}

/// Result of index generation
#[derive(Debug)]
pub struct IndexGenResult {
    /// Exact public source profile.
    pub source_profile: String,
    /// Path to generated index file
    pub index_path: String,
    /// Number of packages in index
    pub package_count: usize,
    /// Number of versions across all packages
    pub version_count: usize,
    /// Whether the index was signed
    pub signed: bool,
}

/// Generate repository indices
pub fn generate_indices(config: &IndexGenConfig) -> Result<Vec<IndexGenResult>> {
    let source_profiles = match &config.source_profile {
        Some(source_profile) => {
            let profile =
                conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
                    .with_context(|| {
                    format!(
                        "unsupported exact source profile '{source_profile}' for index generation"
                    )
                })?;
            vec![profile.id().to_string()]
        }
        None => {
            let conn = conary_core::db::open(&config.db_path)?;
            RemiActiveProfileRevision::list(&conn)?
                .into_iter()
                .map(|active| active.source_profile)
                .collect()
        }
    };
    let catalog_authority = CatalogAuthority::from_paths(
        &config.db_path,
        &config.catalog_dir,
        DatabaseWriter::default(),
    );

    let mut results = Vec::new();
    for source_profile in source_profiles {
        results.push(
            generate_index_for_profile(config, &catalog_authority, &source_profile)
                .with_context(|| format!("generate repository index for {source_profile}"))?,
        );
    }

    Ok(results)
}

/// Generate an index for one exact source profile.
fn generate_index_for_profile(
    config: &IndexGenConfig,
    catalog_authority: &CatalogAuthority,
    source_profile: &str,
) -> Result<IndexGenResult> {
    info!("Generating index for source profile: {}", source_profile);

    // Open database
    let conn = conary_core::db::open(&config.db_path)?;

    let pinned = catalog_authority
        .open_active_profile(source_profile)
        .with_context(|| format!("open active immutable catalog for '{source_profile}'"))?;

    // Conversion state is exact-revision state and is valid only with its
    // durable conversion pin.
    let converted =
        ConvertedPackage::find_current_conversions(&conn, pinned.profile_revision_sha256(), None)?;
    for candidate in &converted {
        let converted_id = candidate
            .id
            .context("persisted repository conversion has no row identity")?;
        ConvertedPackage::require_conversion_pin(&conn, converted_id)?;
        candidate.repository_artifact()?;
        candidate.scriptlet_summary()?;
    }
    debug!("Found {} converted packages total", converted.len());

    let packages = get_packages_for_profile(&pinned, source_profile, &converted)?;
    let package_count = packages.len();
    let version_count: usize = packages.values().map(|p| p.versions.len()).sum();

    // Scan chunk store for statistics
    let (chunk_count, total_bytes) = scan_chunk_store(&config.chunk_dir)?;

    // Build the index
    let index = RepositoryIndex {
        version: 2,
        source_profile: source_profile.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        package_count,
        chunk_count,
        total_chunk_bytes: total_bytes,
        total_size_human: human_bytes(total_bytes),
        packages,
    };

    // Ensure output directory exists
    let output_dir = Path::new(&config.output_dir).join(source_profile);
    fs::create_dir_all(&output_dir).context("Failed to create output directory")?;

    // Write index file
    let index_path = output_dir.join("index.json");
    let index_json = serde_json::to_string_pretty(&index)?;
    fs::write(&index_path, &index_json).context("Failed to write index file")?;
    info!("Wrote index to {:?}", index_path);

    // Sign if key provided
    let signed = if let Some(key_path) = &config.sign_key {
        sign_index(&index_path, key_path)
            .with_context(|| format!("sign repository index {}", index_path.display()))?;
        info!("Signed index with key from {:?}", key_path);
        true
    } else {
        false
    };

    Ok(IndexGenResult {
        source_profile: source_profile.to_string(),
        index_path: index_path.to_string_lossy().to_string(),
        package_count,
        version_count,
        signed,
    })
}

/// Get packages for one exact source profile, matching converted artifacts.
fn get_packages_for_profile(
    pinned: &PinnedProfileCatalog,
    source_profile: &str,
    converted: &[ConvertedPackage],
) -> Result<HashMap<String, PackageIndexEntry>> {
    let mut packages: HashMap<String, PackageIndexEntry> = HashMap::new();
    let package_format =
        conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
            .with_context(|| format!("unsupported exact source profile '{source_profile}'"))?
            .package_format()
            .as_str()
            .to_string();

    for package in ProfileCatalog::new(pinned).downloadable_package_records(1)? {
        require_catalog_profile(&package, source_profile)?;
        let entry = packages
            .entry(package.name.clone())
            .or_insert_with(|| PackageIndexEntry {
                name: package.name.clone(),
                versions: Vec::new(),
            });

        let mut converted_info = None;
        for candidate in converted
            .iter()
            .filter(|candidate| !candidate.needs_reconversion())
        {
            let artifact = candidate.repository_artifact()?;
            if artifact.package_name == package.name
                && artifact.package_version == package.version
                && artifact.source_profile == source_profile
                && Some(artifact.package_architecture) == package.architecture.as_deref()
                && candidate.original_checksum == package.checksum
            {
                converted_info = Some(candidate);
                break;
            }
        }

        let version_entry = if let Some(conv) = converted_info {
            let scriptlet_summary = conv.scriptlet_summary()?;
            VersionEntry {
                version: package.version.clone(),
                original_format: conv.original_format.clone(),
                scriptlet_fidelity: conv.scriptlet_fidelity.clone(),
                converted_at: conv.converted_at.clone(),
                original_checksum: conv.original_checksum.clone(),
                scriptlets: Some(ScriptletPackageMetadata::from(&scriptlet_summary)),
            }
        } else {
            VersionEntry {
                version: package.version,
                original_format: package_format.clone(),
                scriptlet_fidelity: "pending".to_string(),
                converted_at: None,
                original_checksum: package.checksum,
                scriptlets: None,
            }
        };

        entry.versions.push(version_entry);
    }

    for entry in packages.values_mut() {
        entry
            .versions
            .sort_by(|left, right| right.version.cmp(&left.version));
    }

    Ok(packages)
}

fn require_catalog_profile(package: &CatalogPackageRecordV1, source_profile: &str) -> Result<()> {
    if package.source_profile != source_profile {
        anyhow::bail!(
            "immutable catalog for '{}' contains package '{}' from profile '{}'",
            source_profile,
            package.name,
            package.source_profile
        );
    }
    Ok(())
}

/// Scan chunk store directory for statistics
fn scan_chunk_store(chunk_dir: &str) -> Result<(usize, u64)> {
    let objects_dir = Path::new(chunk_dir).join("objects");
    if !objects_dir.exists() {
        return Ok((0, 0));
    }

    let mut count = 0usize;
    let mut total_bytes = 0u64;

    // Walk the objects directory
    for entry in walkdir::WalkDir::new(&objects_dir) {
        let entry = entry.with_context(|| {
            format!(
                "Failed to traverse chunk store directory {}",
                objects_dir.display()
            )
        })?;
        if entry.file_type().is_file() {
            // Skip temp files
            if entry.path().extension().is_some_and(|ext| ext == "tmp") {
                continue;
            }
            let metadata = entry
                .metadata()
                .with_context(|| format!("Failed to stat chunk {}", entry.path().display()))?;
            count += 1;
            total_bytes += metadata.len();
        }
    }

    debug!("Chunk store: {} chunks, {} bytes", count, total_bytes);
    Ok((count, total_bytes))
}

/// Sign an index file with Ed25519
fn sign_index(index_path: &Path, key_path: &str) -> Result<()> {
    use ed25519_dalek::{Signer, SigningKey};

    // Read the signing key
    let key_bytes = fs::read(key_path).context("Failed to read signing key")?;

    // Parse key (expecting 32-byte seed or 64-byte keypair)
    let signing_key = if key_bytes.len() == 32 {
        SigningKey::from_bytes(
            &key_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid key length"))?,
        )
    } else if key_bytes.len() == 64 {
        SigningKey::from_keypair_bytes(
            &key_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid keypair length"))?,
        )
        .map_err(|e| anyhow::anyhow!("Invalid keypair: {}", e))?
    } else {
        return Err(anyhow::anyhow!(
            "Invalid key file: expected 32 or 64 bytes, got {}",
            key_bytes.len()
        ));
    };

    // Read index content
    let content = fs::read(index_path).context("Failed to read index file")?;

    // Sign
    let signature = signing_key.sign(&content);

    // Write signature file
    let sig_path = index_path.with_extension("json.sig");
    fs::write(&sig_path, signature.to_bytes()).context("Failed to write signature")?;

    info!("Wrote signature to {:?}", sig_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use conary_core::ccs::convert::ScriptletBundleSummary;

    fn test_config(root: &Path, db_path: &Path) -> IndexGenConfig {
        IndexGenConfig {
            db_path: db_path.display().to_string(),
            chunk_dir: root.join("chunks").display().to_string(),
            catalog_dir: root.join("catalogs").display().to_string(),
            output_dir: root.join("index").display().to_string(),
            source_profile: Some("fedora-44".to_string()),
            sign_key: None,
        }
    }

    #[test]
    fn test_repository_index_serialization() {
        let index = RepositoryIndex {
            version: 2,
            source_profile: "arch".to_string(),
            generated_at: "2026-01-16T12:00:00Z".to_string(),
            package_count: 1,
            chunk_count: 10,
            total_chunk_bytes: 1024 * 1024,
            total_size_human: "1.00 MB".to_string(),
            packages: HashMap::from([(
                "nginx".to_string(),
                PackageIndexEntry {
                    name: "nginx".to_string(),
                    versions: vec![VersionEntry {
                        version: "1.24.0-1".to_string(),
                        original_format: "arch".to_string(),
                        scriptlet_fidelity: "fully-replaced".to_string(),
                        converted_at: Some("2026-01-16T12:00:00Z".to_string()),
                        original_checksum: "sha256:abc123".to_string(),
                        scriptlets: None,
                    }],
                },
            )]),
        };

        let json = serde_json::to_string_pretty(&index).unwrap();
        assert!(json.contains("nginx"));
        assert!(json.contains("1.24.0-1"));

        // Deserialize back
        let parsed: RepositoryIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source_profile, "arch");
        assert_eq!(parsed.package_count, 1);
    }

    #[test]
    fn generated_index_includes_typed_lifecycle_summary() {
        let fixture = ActiveCatalogFixture::new();
        let catalog_package = package(
            "fedora-44",
            "pkg",
            "1.0",
            "1",
            Some("x86_64"),
            3,
            "pkg-source",
        );
        let checksum = catalog_package.checksum.clone();
        let revision = fixture.activate("fedora-44", 1, vec![catalog_package]);
        let pinned = fixture
            .authority()
            .open_active_profile("fedora-44")
            .unwrap();

        let transport =
            crate::server::conversion::test_support::test_transport(&["sha256:chunk".to_string()]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            revision,
            "pkg".to_string(),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            checksum,
            &transport,
            3,
            "sha256:content".to_string(),
            "/cache/pkg.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted
            .set_scriptlet_metadata(&ScriptletBundleSummary {
                scriptlet_fidelity: "native-lifecycle".to_string(),
                ..ScriptletBundleSummary::default()
            })
            .unwrap();
        let converted = vec![converted];

        let packages = get_packages_for_profile(&pinned, "fedora-44", &converted).unwrap();
        let version = packages.get("pkg").unwrap().versions.first().unwrap();
        let scriptlets = version.scriptlets.as_ref().unwrap();

        assert_eq!(scriptlets.scriptlet_fidelity, "native-lifecycle");

        let index = RepositoryIndex {
            version: 2,
            source_profile: "fedora-44".to_string(),
            generated_at: "2026-01-16T12:00:00Z".to_string(),
            package_count: packages.len(),
            chunk_count: 1,
            total_chunk_bytes: 3,
            total_size_human: "3 B".to_string(),
            packages,
        };
        let json = serde_json::to_string(&index).unwrap();

        assert!(json.contains("\"scriptlets\""));
    }

    #[test]
    fn generated_index_omits_converted_only_stale_rows() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate("fedora-44", 1, Vec::new());
        let pinned = fixture
            .authority()
            .open_active_profile("fedora-44")
            .unwrap();

        let transport = crate::server::conversion::test_support::test_transport(&[
            "sha256:stale-only-chunk".to_string(),
        ]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            revision,
            "stale-only".to_string(),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:stale-only-source".to_string(),
            &transport,
            42,
            "sha256:stale-only-content".to_string(),
            "/tmp/stale-only.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted.conversion_version = conary_core::db::models::CONVERSION_VERSION - 1;

        let packages = get_packages_for_profile(&pinned, "fedora-44", &[converted]).unwrap();

        assert!(
            packages
                .values()
                .all(|package| package.name != "stale-only")
        );
    }

    #[test]
    fn index_generation_propagates_a_source_profile_failure() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("corrupt.sqlite");
        fs::write(&db_path, b"not a SQLite database").unwrap();
        let config = test_config(temp.path(), &db_path);

        let error = generate_indices(&config)
            .expect_err("a failed source_profile index must fail the requested generation")
            .to_string();

        assert!(
            error.contains("generate repository index for fedora-44"),
            "{error}"
        );
    }

    #[test]
    fn index_generation_rejects_route_alias_as_persisted_profile() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = test_config(temp.path(), &temp.path().join("unused.sqlite"));
        config.source_profile = Some("fedora".to_string());

        let error = generate_indices(&config).unwrap_err().to_string();

        assert!(
            error.contains("unsupported exact source profile 'fedora'"),
            "{error}"
        );
    }

    #[test]
    fn configured_index_signing_failure_is_not_downgraded_to_unsigned() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = ActiveCatalogFixture::new();
        fixture.activate("fedora-44", 1, Vec::new());
        let mut config = test_config(temp.path(), fixture.db_path());
        config.catalog_dir = fixture
            .db_path()
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("catalogs")
            .display()
            .to_string();
        config.sign_key = Some(temp.path().join("missing.private").display().to_string());

        let error = format!(
            "{:#}",
            generate_indices(&config).expect_err("configured signing must fail closed")
        );

        assert!(error.contains("sign repository index"), "{error}");
        assert!(error.contains("Failed to read signing key"), "{error}");
    }
}
