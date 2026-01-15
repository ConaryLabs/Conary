// src/ccs/export/oci.rs
//! OCI image export for CCS packages
//!
//! Generates OCI-compatible container images from CCS packages.
//! The resulting images can be loaded with podman, docker, or other
//! OCI-compatible runtimes.

use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;
use tar::Builder;

use crate::ccs::package::CcsPackage;
use crate::packages::traits::PackageFormat;

/// OCI image layout version
const OCI_LAYOUT_VERSION: &str = "1.0.0";

/// OCI image manifest media type
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// OCI image config media type
const CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// OCI layer media type (gzipped tar)
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// OCI image index media type
const INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

/// Container configuration from ccs.toml
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerConfig {
    /// Container entrypoint
    #[serde(default)]
    pub entrypoint: Vec<String>,

    /// Default command arguments
    #[serde(default)]
    pub cmd: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Working directory
    #[serde(default)]
    pub working_dir: Option<String>,

    /// Exposed ports (e.g., "8080/tcp")
    #[serde(default)]
    pub exposed_ports: Vec<String>,

    /// User to run as
    #[serde(default)]
    pub user: Option<String>,
}

/// OCI image configuration
#[derive(Debug, Serialize)]
struct OciConfig {
    created: String,
    architecture: String,
    os: String,
    config: OciConfigRuntime,
    rootfs: OciRootfs,
    history: Vec<OciHistory>,
}

#[derive(Debug, Serialize)]
struct OciConfigRuntime {
    #[serde(rename = "Entrypoint", skip_serializing_if = "Option::is_none")]
    entrypoint: Option<Vec<String>>,
    #[serde(rename = "Cmd", skip_serializing_if = "Option::is_none")]
    cmd: Option<Vec<String>>,
    #[serde(rename = "Env", skip_serializing_if = "Vec::is_empty")]
    env: Vec<String>,
    #[serde(rename = "WorkingDir", skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
    #[serde(rename = "ExposedPorts", skip_serializing_if = "HashMap::is_empty")]
    exposed_ports: HashMap<String, serde_json::Value>,
    #[serde(rename = "User", skip_serializing_if = "Option::is_none")]
    user: Option<String>,
}

#[derive(Debug, Serialize)]
struct OciRootfs {
    #[serde(rename = "type")]
    rootfs_type: String,
    diff_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct OciHistory {
    created: String,
    created_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
}

/// OCI image manifest
#[derive(Debug, Serialize)]
struct OciManifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType")]
    media_type: String,
    config: OciDescriptor,
    layers: Vec<OciDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<HashMap<String, String>>,
}

/// OCI image index
#[derive(Debug, Serialize)]
struct OciIndex {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType")]
    media_type: String,
    manifests: Vec<OciDescriptor>,
}

/// OCI content descriptor
#[derive(Debug, Serialize)]
struct OciDescriptor {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<OciPlatform>,
}

/// OCI platform specification
#[derive(Debug, Serialize)]
struct OciPlatform {
    architecture: String,
    os: String,
}

/// OCI layout file content
#[derive(Debug, Serialize)]
struct OciLayout {
    #[serde(rename = "imageLayoutVersion")]
    image_layout_version: String,
}

/// Export CCS packages to OCI image format
pub fn export_oci(packages: &[String], output: &Path, _db_path: Option<&Path>) -> Result<()> {
    if packages.is_empty() {
        anyhow::bail!("No packages specified for export");
    }

    // Create temp directory for OCI layout
    let temp_dir = tempfile::tempdir()?;
    let blobs_dir = temp_dir.path().join("blobs/sha256");
    fs::create_dir_all(&blobs_dir)?;

    // Parse packages and collect files
    let mut all_files: Vec<(String, Vec<u8>, u32)> = Vec::new(); // (path, content, mode)
    let mut package_names: Vec<String> = Vec::new();
    let mut container_config = ContainerConfig::default();

    for pkg_path in packages {
        let pkg = CcsPackage::parse(pkg_path)
            .with_context(|| format!("Failed to parse package: {}", pkg_path))?;

        package_names.push(pkg.manifest().package.name.clone());

        // Extract container config from first package (if any)
        // TODO: Add container field to CcsManifest
        if container_config.entrypoint.is_empty() && container_config.cmd.is_empty() {
            // Default to /bin/sh if no entrypoint specified
            container_config.cmd = vec!["/bin/sh".to_string()];
        }

        // Extract file contents
        let blobs = pkg.extract_all_content()?;
        let files = pkg.file_entries();

        for file in files {
            if let Some(target) = &file.target {
                // Symlink - store as special content
                all_files.push((file.path.clone(), format!("symlink:{}", target).into_bytes(), file.mode));
            } else if let Some(content) = blobs.get(&file.hash) {
                all_files.push((file.path.clone(), content.clone(), file.mode));
            }
        }
    }

    // Sort files for deterministic layer
    all_files.sort_by(|a, b| a.0.cmp(&b.0));

    // Create layer tarball
    let layer_data = create_layer_tarball(&all_files)?;
    let layer_digest = format!("sha256:{}", sha256_hex(&layer_data));
    let layer_diff_id = format!("sha256:{}", sha256_hex_uncompressed(&all_files)?);

    // Write layer blob
    fs::write(blobs_dir.join(&layer_digest[7..]), &layer_data)?;

    // Create config
    let config = create_config(&container_config, &layer_diff_id, &package_names);
    let config_json = serde_json::to_string_pretty(&config)?;
    let config_digest = format!("sha256:{}", sha256_hex(config_json.as_bytes()));
    fs::write(blobs_dir.join(&config_digest[7..]), &config_json)?;

    // Create manifest
    let manifest = OciManifest {
        schema_version: 2,
        media_type: MANIFEST_MEDIA_TYPE.to_string(),
        config: OciDescriptor {
            media_type: CONFIG_MEDIA_TYPE.to_string(),
            digest: config_digest.clone(),
            size: config_json.len() as u64,
            annotations: None,
            platform: None,
        },
        layers: vec![OciDescriptor {
            media_type: LAYER_MEDIA_TYPE.to_string(),
            digest: layer_digest.clone(),
            size: layer_data.len() as u64,
            annotations: None,
            platform: None,
        }],
        annotations: Some({
            let mut ann = HashMap::new();
            ann.insert(
                "org.opencontainers.image.title".to_string(),
                package_names.join(", "),
            );
            ann.insert(
                "org.opencontainers.image.source".to_string(),
                "CCS Package Export".to_string(),
            );
            ann
        }),
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let manifest_digest = format!("sha256:{}", sha256_hex(manifest_json.as_bytes()));
    fs::write(blobs_dir.join(&manifest_digest[7..]), &manifest_json)?;

    // Create index
    let index = OciIndex {
        schema_version: 2,
        media_type: INDEX_MEDIA_TYPE.to_string(),
        manifests: vec![OciDescriptor {
            media_type: MANIFEST_MEDIA_TYPE.to_string(),
            digest: manifest_digest,
            size: manifest_json.len() as u64,
            annotations: Some({
                let mut ann = HashMap::new();
                ann.insert(
                    "org.opencontainers.image.ref.name".to_string(),
                    format!("{}:latest", package_names.first().unwrap_or(&"image".to_string())),
                );
                ann
            }),
            platform: Some(OciPlatform {
                architecture: std::env::consts::ARCH.to_string(),
                os: "linux".to_string(),
            }),
        }],
    };

    let index_json = serde_json::to_string_pretty(&index)?;
    fs::write(temp_dir.path().join("index.json"), &index_json)?;

    // Create oci-layout file
    let layout = OciLayout {
        image_layout_version: OCI_LAYOUT_VERSION.to_string(),
    };
    let layout_json = serde_json::to_string_pretty(&layout)?;
    fs::write(temp_dir.path().join("oci-layout"), &layout_json)?;

    // Create output tar archive
    let output_file = File::create(output)?;
    let mut archive = Builder::new(output_file);
    archive.append_dir_all(".", temp_dir.path())?;
    archive.finish()?;

    println!("Exported OCI image: {}", output.display());
    println!("  Packages: {}", package_names.join(", "));
    println!("  Layer size: {} bytes", layer_data.len());
    println!();
    println!("To load the image:");
    println!("  podman load < {}", output.display());
    println!("  # or");
    println!("  skopeo copy oci-archive:{} containers-storage:localhost/{}:latest",
             output.display(), package_names.first().unwrap_or(&"image".to_string()));

    Ok(())
}

/// Create a gzipped tar layer from files
fn create_layer_tarball(files: &[(String, Vec<u8>, u32)]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    {
        let encoder = GzEncoder::new(&mut output, Compression::default());
        let mut archive = Builder::new(encoder);

        // Track directories we've created
        let mut created_dirs = std::collections::HashSet::new();

        for (path, content, mode) in files {
            // Clean the path (remove leading slashes)
            let clean_path = path.trim_start_matches('/');

            // Create parent directories
            let path_obj = Path::new(clean_path);
            if let Some(parent) = path_obj.parent() {
                let mut current = std::path::PathBuf::new();
                for component in parent.components() {
                    match component {
                        std::path::Component::Normal(c) => current.push(c),
                        _ => continue, // Skip RootDir, CurDir, ParentDir
                    }
                    let dir_path = current.to_string_lossy().to_string();
                    if !dir_path.is_empty() && !created_dirs.contains(&dir_path) {
                        let mut header = tar::Header::new_gnu();
                        header.set_entry_type(tar::EntryType::Directory);
                        header.set_mode(0o755);
                        header.set_size(0);
                        header.set_mtime(1704067200); // 2024-01-01
                        header.set_cksum();
                        archive.append_data(&mut header, &dir_path, std::io::empty())?;
                        created_dirs.insert(dir_path);
                    }
                }
            }

            // Handle symlinks
            if content.starts_with(b"symlink:") {
                let target = String::from_utf8_lossy(&content[8..]);
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_mtime(1704067200);
                header.set_cksum();

                archive.append_link(&mut header, clean_path, target.as_ref())?;
            } else {
                // Regular file
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_size(content.len() as u64);
                header.set_mtime(1704067200);
                header.set_cksum();

                archive.append_data(&mut header, clean_path, content.as_slice())?;
            }
        }

        let encoder = archive.into_inner()?;
        encoder.finish()?;
    }

    Ok(output)
}

/// Calculate SHA256 of uncompressed layer content (for diff_id)
fn sha256_hex_uncompressed(files: &[(String, Vec<u8>, u32)]) -> Result<String> {
    // Create uncompressed tar for diff_id calculation
    let mut output = Vec::new();
    {
        let mut archive = Builder::new(&mut output);
        let mut created_dirs = std::collections::HashSet::new();

        for (path, content, mode) in files {
            // Clean the path (remove leading slashes)
            let clean_path = path.trim_start_matches('/');
            let path_obj = Path::new(clean_path);

            if let Some(parent) = path_obj.parent() {
                let mut current = std::path::PathBuf::new();
                for component in parent.components() {
                    match component {
                        std::path::Component::Normal(c) => current.push(c),
                        _ => continue,
                    }
                    let dir_path = current.to_string_lossy().to_string();
                    if !dir_path.is_empty() && !created_dirs.contains(&dir_path) {
                        let mut header = tar::Header::new_gnu();
                        header.set_entry_type(tar::EntryType::Directory);
                        header.set_mode(0o755);
                        header.set_size(0);
                        header.set_mtime(1704067200);
                        header.set_cksum();
                        archive.append_data(&mut header, &dir_path, std::io::empty())?;
                        created_dirs.insert(dir_path);
                    }
                }
            }

            if content.starts_with(b"symlink:") {
                let target = String::from_utf8_lossy(&content[8..]);
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_mtime(1704067200);
                header.set_cksum();
                archive.append_link(&mut header, clean_path, target.as_ref())?;
            } else {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_size(content.len() as u64);
                header.set_mtime(1704067200);
                header.set_cksum();
                archive.append_data(&mut header, clean_path, content.as_slice())?;
            }
        }

        archive.finish()?;
    }

    Ok(sha256_hex(&output))
}

/// Calculate SHA256 hex digest
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Create OCI image config
fn create_config(container: &ContainerConfig, diff_id: &str, packages: &[String]) -> OciConfig {
    let now = chrono::Utc::now().to_rfc3339();

    // Convert env map to KEY=VALUE format
    let env: Vec<String> = container
        .env
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .chain(std::iter::once("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()))
        .collect();

    // Convert exposed ports to OCI format
    let exposed_ports: HashMap<String, serde_json::Value> = container
        .exposed_ports
        .iter()
        .map(|p| (p.clone(), serde_json::json!({})))
        .collect();

    OciConfig {
        created: now.clone(),
        architecture: std::env::consts::ARCH.to_string(),
        os: "linux".to_string(),
        config: OciConfigRuntime {
            entrypoint: if container.entrypoint.is_empty() {
                None
            } else {
                Some(container.entrypoint.clone())
            },
            cmd: if container.cmd.is_empty() {
                None
            } else {
                Some(container.cmd.clone())
            },
            env,
            working_dir: container.working_dir.clone(),
            exposed_ports,
            user: container.user.clone(),
        },
        rootfs: OciRootfs {
            rootfs_type: "layers".to_string(),
            diff_ids: vec![diff_id.to_string()],
        },
        history: vec![OciHistory {
            created: now,
            created_by: format!("conary ccs-export: {}", packages.join(", ")),
            comment: Some("Created by Conary CCS package export".to_string()),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hex() {
        let data = b"hello world";
        let hash = sha256_hex(data);
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_container_config_default() {
        let config = ContainerConfig::default();
        assert!(config.entrypoint.is_empty());
        assert!(config.cmd.is_empty());
        assert!(config.env.is_empty());
    }
}
