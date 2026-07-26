// conary-core/src/ccs/export/oci.rs
//! OCI image export for CCS packages
//!
//! Generates OCI-compatible container images from CCS packages.
//! The resulting images can be loaded with podman, docker, or other
//! OCI-compatible runtimes.

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;
use tar::Builder;

use crate::ccs::package::CcsPackage;
use crate::ccs::verify::{TrustPolicy, verify_package};
use crate::hash;
use crate::packages::payload::PackagePayloadFile;
use crate::payload::{PayloadIdentity, PayloadNode, PayloadNodeKind};

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

/// One exact CCS payload node projected into an OCI layer.
struct OciFileEntry {
    payload: PackagePayloadFile,
}

/// Export CCS packages to OCI image format
pub fn export_oci(packages: &[String], output: &Path, trust_policy: &TrustPolicy) -> Result<()> {
    if packages.is_empty() {
        anyhow::bail!("No packages specified for export");
    }

    // Create temp directory for OCI layout
    let temp_dir = tempfile::tempdir()?;
    let blobs_dir = temp_dir.path().join("blobs/sha256");
    fs::create_dir_all(&blobs_dir)?;

    // Parse packages and collect files
    let mut all_files: Vec<OciFileEntry> = Vec::new();
    let mut package_names: Vec<String> = Vec::new();
    let mut container_config = ContainerConfig::default();

    for pkg_path in packages {
        let package_path = Path::new(pkg_path);
        let verified = verify_package(package_path, trust_policy)
            .with_context(|| format!("Failed to verify CCS package: {pkg_path}"))?;
        let pkg = CcsPackage::from_verified_archive(pkg_path, &verified)
            .with_context(|| format!("Failed to open verified CCS package: {pkg_path}"))?;

        package_names.push(pkg.manifest().package.name.clone());

        // Extract container config from first package (if any)
        // TODO: Add container field to CcsManifest
        if container_config.entrypoint.is_empty() && container_config.cmd.is_empty() {
            // Default to /bin/sh if no entrypoint specified
            container_config.cmd = vec!["/bin/sh".to_string()];
        }

        for payload in pkg.payload().files() {
            all_files.push(OciFileEntry {
                payload: payload.clone(),
            });
        }
    }

    // Sort files for deterministic layer
    all_files.sort_by(|a, b| a.payload.path.cmp(&b.payload.path));

    // Create and hash the layer through bounded-memory files.
    let compressed_layer = temp_dir.path().join(".layer.tar.gz");
    write_layer_tarball(&all_files, &compressed_layer)?;
    let layer_digest = format!(
        "sha256:{}",
        hash_file(&compressed_layer, hash::HashAlgorithm::Sha256)?
    );
    let layer_size = fs::metadata(&compressed_layer)?.len();
    let uncompressed_layer = temp_dir.path().join(".layer.tar");
    write_uncompressed_layer(&all_files, &uncompressed_layer)?;
    let layer_diff_id = format!(
        "sha256:{}",
        hash_file(&uncompressed_layer, hash::HashAlgorithm::Sha256)?
    );
    fs::remove_file(&uncompressed_layer)?;
    fs::rename(&compressed_layer, blobs_dir.join(&layer_digest[7..]))?;

    // Create config
    let config = create_config(&container_config, &layer_diff_id, &package_names);
    let config_json = serde_json::to_string_pretty(&config)?;
    let config_digest = hash::sha256_prefixed(config_json.as_bytes());
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
            size: layer_size,
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
    let manifest_digest = hash::sha256_prefixed(manifest_json.as_bytes());
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
                    format!(
                        "{}:latest",
                        package_names.first().unwrap_or(&"image".to_string())
                    ),
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
    println!("  Layer size: {layer_size} bytes");
    println!();
    println!("To load the image:");
    println!("  podman load < {}", output.display());
    println!("  # or");
    println!(
        "  skopeo copy oci-archive:{} containers-storage:localhost/{}:latest",
        output.display(),
        package_names.first().unwrap_or(&"image".to_string())
    );

    Ok(())
}

/// Write file entries into a tar archive, creating parent directories as needed
fn write_tar_entries<W: std::io::Write>(
    archive: &mut Builder<W>,
    files: &[OciFileEntry],
) -> Result<()> {
    for entry in files {
        let clean_path = entry.payload.path.trim_start_matches('/');
        if clean_path.is_empty() {
            anyhow::bail!("OCI payload node path must not resolve to archive root");
        }
        let mut header = exact_tar_header(&entry.payload.node)?;
        append_node_pax_extensions(archive, &entry.payload.node)?;
        match &entry.payload.node.kind {
            PayloadNodeKind::Regular { .. } => {
                let authority = entry.payload.content_authority.as_ref().with_context(|| {
                    format!(
                        "regular OCI payload {} has no content authority",
                        entry.payload.path
                    )
                })?;
                let mut content = entry.payload.open_content()?;
                header.set_size(authority.size);
                header.set_cksum();
                archive.append_data(&mut header, clean_path, content.as_mut())?;
            }
            PayloadNodeKind::Symlink { target } => {
                header.set_cksum();
                archive.append_link(&mut header, clean_path, target)?;
            }
            PayloadNodeKind::Hardlink { target, .. } => {
                header.set_cksum();
                archive.append_link(&mut header, clean_path, target.trim_start_matches('/'))?;
            }
            PayloadNodeKind::Socket => {
                anyhow::bail!(
                    "OCI tar layers cannot encode socket payload node {}",
                    entry.payload.path
                );
            }
            _ => {
                header.set_cksum();
                archive.append_data(&mut header, clean_path, std::io::empty())?;
            }
        }
    }

    Ok(())
}

fn exact_tar_header(node: &PayloadNode) -> Result<tar::Header> {
    node.validate()?;
    let mut header = tar::Header::new_gnu();
    header.set_mode(node.mode & 0o7777);
    let PayloadIdentity::Numeric { id: uid } = &node.user else {
        anyhow::bail!("OCI export requires payload user identity to be resolved");
    };
    let PayloadIdentity::Numeric { id: gid } = &node.group else {
        anyhow::bail!("OCI export requires payload group identity to be resolved");
    };
    header.set_uid(*uid);
    header.set_gid(*gid);
    header.set_mtime(u64::try_from(node.mtime.seconds).unwrap_or(0));
    header.set_size(0);
    let entry_type = match &node.kind {
        PayloadNodeKind::Regular { .. } => tar::EntryType::Regular,
        PayloadNodeKind::Directory => tar::EntryType::Directory,
        PayloadNodeKind::Symlink { .. } => tar::EntryType::Symlink,
        PayloadNodeKind::Hardlink { .. } => tar::EntryType::Link,
        PayloadNodeKind::BlockDevice { major, minor } => {
            header.set_device_major(u32::try_from(*major)?)?;
            header.set_device_minor(u32::try_from(*minor)?)?;
            tar::EntryType::Block
        }
        PayloadNodeKind::CharacterDevice { major, minor } => {
            header.set_device_major(u32::try_from(*major)?)?;
            header.set_device_minor(u32::try_from(*minor)?)?;
            tar::EntryType::Char
        }
        PayloadNodeKind::Fifo => tar::EntryType::Fifo,
        PayloadNodeKind::Socket => tar::EntryType::new(b's'),
    };
    header.set_entry_type(entry_type);
    Ok(header)
}

fn append_node_pax_extensions<W: std::io::Write>(
    archive: &mut Builder<W>,
    node: &PayloadNode,
) -> Result<()> {
    let mut extensions = Vec::new();
    let mtime = exact_pax_timestamp(node.mtime.seconds, node.mtime.nanoseconds);
    if node.mtime.nanoseconds != 0 || node.mtime.seconds < 0 {
        extensions.push(("mtime".to_string(), mtime.into_bytes()));
    }
    for (name, value) in &node.xattrs {
        extensions.push((format!("SCHILY.xattr.{name}"), value.clone()));
    }
    archive.append_pax_extensions(
        extensions
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_slice())),
    )?;
    Ok(())
}

fn exact_pax_timestamp(seconds: i64, nanoseconds: u32) -> String {
    if nanoseconds == 0 {
        return seconds.to_string();
    }
    if seconds >= 0 {
        return format!("{seconds}.{nanoseconds:09}");
    }
    let whole = seconds.unsigned_abs() - 1;
    let fractional = 1_000_000_000 - nanoseconds;
    format!("-{whole}.{fractional:09}")
}

fn write_layer_tarball(files: &[OciFileEntry], output: &Path) -> Result<()> {
    let encoder = GzEncoder::new(File::create(output)?, Compression::default());
    let mut archive = Builder::new(encoder);
    write_tar_entries(&mut archive, files)?;
    let encoder = archive.into_inner()?;
    encoder.finish()?;
    Ok(())
}

fn write_uncompressed_layer(files: &[OciFileEntry], output: &Path) -> Result<()> {
    let mut archive = Builder::new(File::create(output)?);
    write_tar_entries(&mut archive, files)?;
    archive.finish()?;
    Ok(())
}

fn hash_file(path: &Path, algorithm: hash::HashAlgorithm) -> Result<String> {
    let mut file = File::open(path)?;
    Ok(hash::hash_reader(algorithm, &mut file)?.value)
}
/// Create OCI image config
fn create_config(container: &ContainerConfig, diff_id: &str, packages: &[String]) -> OciConfig {
    let now = chrono::Utc::now().to_rfc3339();

    // Convert env map to KEY=VALUE format
    let env: Vec<String> = container
        .env
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .chain(std::iter::once(
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ))
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

    fn signed_test_package(
        temp: &tempfile::TempDir,
    ) -> (std::path::PathBuf, crate::ccs::signing::SigningKeyPair) {
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("usr/bin")).unwrap();
        fs::write(source.join("usr/bin/demo"), b"demo\n").unwrap();
        let manifest = crate::ccs::manifest::CcsManifest::new_minimal("demo", "1.0.0");
        let result = crate::ccs::builder::CcsBuilder::new(manifest, &source)
            .unwrap()
            .build()
            .unwrap();
        let package_path = temp.path().join("demo.ccs");
        let signer = crate::ccs::signing::SigningKeyPair::generate().with_key_id("export-test");
        crate::ccs::builder::write_signed_current_ccs_package(
            &result,
            &package_path,
            &signer,
            false,
        )
        .unwrap();
        (package_path, signer)
    }

    #[test]
    fn test_sha256_integration() {
        let data = b"hello world";
        let digest = hash::sha256(data);
        assert_eq!(
            digest,
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

    #[test]
    fn export_requires_trusted_signed_v2_authority() {
        let temp = tempfile::tempdir().unwrap();
        let (package_path, signer) = signed_test_package(&temp);
        let output = temp.path().join("demo.oci.tar");
        let trusted = TrustPolicy::strict(vec![signer.public_key_base64()]);

        export_oci(
            &[package_path.to_string_lossy().into_owned()],
            &output,
            &trusted,
        )
        .unwrap();
        assert!(output.exists());

        let untrusted = crate::ccs::signing::SigningKeyPair::generate();
        let error = export_oci(
            &[package_path.to_string_lossy().into_owned()],
            &temp.path().join("untrusted.oci.tar"),
            &TrustPolicy::strict(vec![untrusted.public_key_base64()]),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not trusted"));
    }
}
