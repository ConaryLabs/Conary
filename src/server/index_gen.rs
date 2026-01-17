// src/server/index_gen.rs
//! Repository index generation for Remi
//!
//! Generates a JSON index of all converted packages for a distribution.
//! The index is used by clients to discover available packages without
//! querying the server for each package individually.

use crate::db::models::ConvertedPackage;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info};

/// Repository index containing all converted packages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    /// Index format version
    pub version: u32,
    /// Distribution name
    pub distro: String,
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
    /// Conversion fidelity (full, high, partial, low)
    pub fidelity: String,
    /// When this version was converted
    pub converted_at: Option<String>,
    /// SHA-256 checksum of original package
    pub original_checksum: String,
}

/// Index generation configuration
pub struct IndexGenConfig {
    /// Path to the database
    pub db_path: String,
    /// Path to the chunk store
    pub chunk_dir: String,
    /// Output directory for index files
    pub output_dir: String,
    /// Specific distribution to generate (None = all)
    pub distro: Option<String>,
    /// Path to signing key (optional)
    pub sign_key: Option<String>,
}

/// Result of index generation
#[derive(Debug)]
pub struct IndexGenResult {
    /// Distribution name
    pub distro: String,
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
    let distros = match &config.distro {
        Some(d) => vec![d.clone()],
        None => vec![
            "arch".to_string(),
            "fedora".to_string(),
            "ubuntu".to_string(),
            "debian".to_string(),
        ],
    };

    let mut results = Vec::new();
    for distro in distros {
        match generate_index_for_distro(config, &distro) {
            Ok(result) => results.push(result),
            Err(e) => {
                tracing::warn!("Failed to generate index for {}: {}", distro, e);
            }
        }
    }

    Ok(results)
}

/// Generate index for a specific distribution
fn generate_index_for_distro(config: &IndexGenConfig, distro: &str) -> Result<IndexGenResult> {
    info!("Generating index for distribution: {}", distro);

    // Open database
    let conn = crate::db::open(&config.db_path)?;

    // Get all converted packages
    let converted = ConvertedPackage::list_all(&conn)?;
    debug!("Found {} converted packages total", converted.len());

    // Get package metadata from repository_packages table
    let packages = get_packages_for_distro(&conn, distro, &converted)?;
    let package_count = packages.len();
    let version_count: usize = packages.values().map(|p| p.versions.len()).sum();

    // Scan chunk store for statistics
    let (chunk_count, total_bytes) = scan_chunk_store(&config.chunk_dir)?;

    // Build the index
    let index = RepositoryIndex {
        version: 1,
        distro: distro.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        package_count,
        chunk_count,
        total_chunk_bytes: total_bytes,
        total_size_human: human_bytes(total_bytes),
        packages,
    };

    // Ensure output directory exists
    let output_dir = Path::new(&config.output_dir).join(distro);
    fs::create_dir_all(&output_dir).context("Failed to create output directory")?;

    // Write index file
    let index_path = output_dir.join("index.json");
    let index_json = serde_json::to_string_pretty(&index)?;
    fs::write(&index_path, &index_json).context("Failed to write index file")?;
    info!("Wrote index to {:?}", index_path);

    // Sign if key provided
    let signed = if let Some(key_path) = &config.sign_key {
        match sign_index(&index_path, key_path) {
            Ok(()) => {
                info!("Signed index with key from {:?}", key_path);
                true
            }
            Err(e) => {
                tracing::warn!("Failed to sign index: {}", e);
                false
            }
        }
    } else {
        false
    };

    Ok(IndexGenResult {
        distro: distro.to_string(),
        index_path: index_path.to_string_lossy().to_string(),
        package_count,
        version_count,
        signed,
    })
}

/// Get packages for a specific distribution, matching with converted packages
fn get_packages_for_distro(
    conn: &rusqlite::Connection,
    distro: &str,
    converted: &[ConvertedPackage],
) -> Result<HashMap<String, PackageIndexEntry>> {
    // Query repository_packages for this distribution
    let mut stmt = conn.prepare(
        "SELECT rp.name, rp.version, r.name as repo_name
         FROM repository_packages rp
         JOIN repositories r ON rp.repository_id = r.id
         WHERE r.name LIKE ?1 OR r.url LIKE ?2
         ORDER BY rp.name, rp.version DESC",
    )?;

    let distro_pattern = format!("%{}%", distro);
    let rows = stmt.query_map([&distro_pattern, &distro_pattern], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    // Build checksum lookup for converted packages (for future use)
    let _checksum_map: HashMap<&str, &ConvertedPackage> = converted
        .iter()
        .map(|c| (c.original_checksum.as_str(), c))
        .collect();

    let mut packages: HashMap<String, PackageIndexEntry> = HashMap::new();

    for row_result in rows {
        let (name, version, _repo_name) = row_result?;

        // For now, include all packages from the distro
        // In a full implementation, we'd match by checksum
        let entry = packages.entry(name.clone()).or_insert_with(|| PackageIndexEntry {
            name: name.clone(),
            versions: Vec::new(),
        });

        // Check if this version is converted (simplified matching by name/version)
        let converted_info = converted.iter().find(|c| {
            // Try to match by looking at the detected_hooks JSON for package info
            // This is a simplified approach - in production you'd have proper linking
            c.detected_hooks
                .as_ref()
                .is_some_and(|h| h.contains(&name) && h.contains(&version))
        });

        let version_entry = if let Some(conv) = converted_info {
            VersionEntry {
                version: version.clone(),
                original_format: conv.original_format.clone(),
                fidelity: conv.conversion_fidelity.clone(),
                converted_at: conv.converted_at.clone(),
                original_checksum: conv.original_checksum.clone(),
            }
        } else {
            // Not yet converted - still include in index as "pending"
            VersionEntry {
                version,
                original_format: distro_to_format(distro),
                fidelity: "pending".to_string(),
                converted_at: None,
                original_checksum: String::new(),
            }
        };

        entry.versions.push(version_entry);
    }

    // Also add packages from converted_packages that match this distro format
    for conv in converted {
        let format = &conv.original_format;
        if format_matches_distro(format, distro) {
            // Try to extract name from detected_hooks or use checksum as identifier
            if let Some(hooks) = &conv.detected_hooks
                && let Ok(info) = serde_json::from_str::<serde_json::Value>(hooks)
                && let Some(name) = info.get("name").and_then(|n| n.as_str())
            {
                let version = info
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                let entry = packages.entry(name.to_string()).or_insert_with(|| {
                    PackageIndexEntry {
                        name: name.to_string(),
                        versions: Vec::new(),
                    }
                });

                // Only add if not already present
                if !entry.versions.iter().any(|v| v.version == version) {
                    entry.versions.push(VersionEntry {
                        version: version.to_string(),
                        original_format: conv.original_format.clone(),
                        fidelity: conv.conversion_fidelity.clone(),
                        converted_at: conv.converted_at.clone(),
                        original_checksum: conv.original_checksum.clone(),
                    });
                }
            }
        }
    }

    Ok(packages)
}

/// Map distribution name to package format
fn distro_to_format(distro: &str) -> String {
    match distro {
        "arch" => "arch".to_string(),
        "fedora" | "centos" | "rhel" => "rpm".to_string(),
        "ubuntu" | "debian" => "deb".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Check if a package format matches a distribution
fn format_matches_distro(format: &str, distro: &str) -> bool {
    matches!(
        (format, distro),
        ("arch", "arch") | ("rpm", "fedora" | "centos" | "rhel") | ("deb", "ubuntu" | "debian")
    )
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
    for entry in walkdir::WalkDir::new(&objects_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            // Skip temp files
            if entry.path().extension().is_some_and(|ext| ext == "tmp") {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                count += 1;
                total_bytes += metadata.len();
            }
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
        SigningKey::from_bytes(&key_bytes.try_into().map_err(|_| anyhow::anyhow!("Invalid key length"))?)
    } else if key_bytes.len() == 64 {
        SigningKey::from_keypair_bytes(&key_bytes.try_into().map_err(|_| anyhow::anyhow!("Invalid keypair length"))?)
            .map_err(|e| anyhow::anyhow!("Invalid keypair: {}", e))?
    } else {
        return Err(anyhow::anyhow!("Invalid key file: expected 32 or 64 bytes, got {}", key_bytes.len()));
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

/// Format bytes as human-readable string
fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_distro_to_format() {
        assert_eq!(distro_to_format("arch"), "arch");
        assert_eq!(distro_to_format("fedora"), "rpm");
        assert_eq!(distro_to_format("ubuntu"), "deb");
        assert_eq!(distro_to_format("debian"), "deb");
    }

    #[test]
    fn test_format_matches_distro() {
        assert!(format_matches_distro("arch", "arch"));
        assert!(format_matches_distro("rpm", "fedora"));
        assert!(format_matches_distro("deb", "ubuntu"));
        assert!(format_matches_distro("deb", "debian"));
        assert!(!format_matches_distro("rpm", "arch"));
        assert!(!format_matches_distro("deb", "fedora"));
    }

    #[test]
    fn test_repository_index_serialization() {
        let index = RepositoryIndex {
            version: 1,
            distro: "arch".to_string(),
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
                        fidelity: "high".to_string(),
                        converted_at: Some("2026-01-16T12:00:00Z".to_string()),
                        original_checksum: "sha256:abc123".to_string(),
                    }],
                },
            )]),
        };

        let json = serde_json::to_string_pretty(&index).unwrap();
        assert!(json.contains("nginx"));
        assert!(json.contains("1.24.0-1"));

        // Deserialize back
        let parsed: RepositoryIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.distro, "arch");
        assert_eq!(parsed.package_count, 1);
    }
}
