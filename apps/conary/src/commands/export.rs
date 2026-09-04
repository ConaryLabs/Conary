// apps/conary/src/commands/export.rs
//! Export Conary generations as OCI container images.
//!
//! Produces a standards-compliant OCI Image Layout directory from a
//! generation's EROFS image and CAS objects.  The resulting directory
//! can be loaded directly by `podman load` or `docker load` (after
//! converting with `skopeo copy oci:dir docker-archive:file.tar`).
//!
//! OCI Image Layout Specification:
//!   https://github.com/opencontainers/image-spec/blob/main/image-layout.md

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use conary_core::filesystem::object_path;
use conary_core::generation::artifact::{CasObjectRef, GenerationArtifact};
use conary_core::generation::metadata::{
    EROFS_IMAGE_NAME, GENERATION_METADATA_FILE, GenerationMetadata,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use tracing::{info, warn};

/// OCI media types used in the manifest and config.
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
const INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

/// Map the host architecture to the OCI platform architecture string.
fn oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "riscv64" => "riscv64",
        other => other,
    }
}

/// Export a generation as an OCI image directory layout.
///
/// The generation is identified by number.  Pass `None` to use the
/// currently active generation.  The `objects_dir` points to the CAS
/// store requested by the CLI, but the loaded generation artifact is
/// authoritative and may redirect the export to its persisted CAS base.
pub fn export_oci(generation: Option<i64>, objects_dir: &Path, output_dir: &Path) -> Result<()> {
    let artifact = load_oci_generation_artifact(generation)?;

    export_oci_artifact(&artifact, objects_dir, output_dir)
}

fn load_oci_generation_artifact(generation: Option<i64>) -> Result<GenerationArtifact> {
    let artifact = match generation {
        Some(n) => conary_core::generation::artifact::load_installed_generation_artifact(n)
            .with_context(|| format!("Failed to load artifact for generation {n}"))?,
        None => load_current_oci_generation_artifact(Path::new("/conary/current"))?,
    };

    Ok(artifact)
}

fn load_current_oci_generation_artifact(current_path: &Path) -> Result<GenerationArtifact> {
    conary_core::generation::artifact::load_generation_artifact(current_path)
        .context("Failed to load current generation artifact")
}

fn export_oci_artifact(
    artifact: &GenerationArtifact,
    objects_dir: &Path,
    output_dir: &Path,
) -> Result<()> {
    let gen_number = artifact.generation;
    let gen_dir = &artifact.generation_dir;
    let erofs_path = &artifact.erofs_path;
    let metadata = &artifact.metadata;
    let cas_dir = &artifact.cas_dir;
    let cas_objects = &artifact.cas_objects;

    if cas_dir.as_path() != objects_dir {
        warn!(
            requested = %objects_dir.display(),
            artifact = %cas_dir.display(),
            "Using CAS object directory from generation artifact"
        );
    }

    info!(
        "Exporting generation {gen_number} as OCI image to {}",
        output_dir.display()
    );
    info!(
        "Generation {gen_number} references {} CAS objects",
        cas_objects.len()
    );

    // Create OCI directory structure
    let blobs_dir = output_dir.join("blobs/sha256");
    fs::create_dir_all(&blobs_dir).with_context(|| {
        format!(
            "Failed to create blobs directory at {}",
            blobs_dir.display()
        )
    })?;

    // Step 1: Build the layer tar.gz
    let (layer_digest, layer_size, diff_id) =
        build_layer_tar_gz(erofs_path, cas_dir, gen_dir, &blobs_dir, cas_objects)?;
    info!(
        "Layer: sha256:{} ({} bytes, diffID: sha256:{})",
        layer_digest, layer_size, diff_id
    );

    // Step 2: Build the config JSON
    let config_json = build_config_json(metadata, gen_number, &diff_id);
    let (config_digest, config_size) = write_blob(&blobs_dir, config_json.as_bytes())?;
    info!("Config: sha256:{} ({} bytes)", config_digest, config_size);

    // Step 3: Build the manifest JSON
    let manifest_json = build_manifest_json(&config_digest, config_size, &layer_digest, layer_size);
    let (manifest_digest, manifest_size) = write_blob(&blobs_dir, manifest_json.as_bytes())?;
    info!(
        "Manifest: sha256:{} ({} bytes)",
        manifest_digest, manifest_size
    );

    // Step 4: Write index.json
    let index_json = build_index_json(&manifest_digest, manifest_size);
    fs::write(output_dir.join("index.json"), &index_json).context("Failed to write index.json")?;

    // Step 5: Write oci-layout
    let oci_layout = r#"{"imageLayoutVersion":"1.0.0"}"#;
    fs::write(output_dir.join("oci-layout"), oci_layout).context("Failed to write oci-layout")?;

    println!(
        "Exported generation {gen_number} to {}",
        output_dir.display()
    );
    println!("  Manifest: sha256:{manifest_digest}");
    println!("  Layer:    sha256:{layer_digest} ({layer_size} bytes)");
    println!();
    println!("Load with:");
    println!(
        "  podman load < $(skopeo copy oci:{} docker-archive:/dev/stdout)",
        output_dir.display()
    );
    println!(
        "  skopeo copy oci:{} docker://registry.example.com/conary-gen:{}",
        output_dir.display(),
        gen_number
    );

    Ok(())
}

/// Build a gzipped tar layer containing the EROFS image and referenced CAS objects.
///
/// Layout inside the tar:
///   root.erofs          -- the generation's EROFS image
///   objects/<prefix>/<suffix>  -- CAS objects referenced by the generation
///
/// Returns (compressed_digest, compressed_size, uncompressed_digest).
fn build_layer_tar_gz(
    erofs_path: &Path,
    objects_dir: &Path,
    gen_dir: &Path,
    blobs_dir: &Path,
    cas_objects: &[CasObjectRef],
) -> Result<(String, u64, String)> {
    // We need both compressed digest (for the manifest) and uncompressed
    // digest (for the config's diff_id).  Build the tar in memory first
    // to compute the uncompressed hash, then gzip-compress and hash again.

    // Build uncompressed tar
    let tar_bytes = build_layer_tar(erofs_path, objects_dir, gen_dir, cas_objects)?;

    // Compute uncompressed (diffID) digest
    let diff_id = hex_digest(&tar_bytes);

    // Compress and write to a temp file, then compute compressed digest
    let temp_path = blobs_dir.join("layer.tmp");
    {
        let file = fs::File::create(&temp_path).context("Failed to create temporary layer file")?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder
            .write_all(&tar_bytes)
            .context("Failed to write compressed layer")?;
        encoder
            .finish()
            .context("Failed to finish gzip compression")?;
    }

    // Read back compressed bytes to compute digest
    let compressed_bytes = fs::read(&temp_path).context("Failed to read compressed layer")?;
    let compressed_digest = hex_digest(&compressed_bytes);
    let compressed_size = compressed_bytes.len() as u64;

    // Move to final location
    let final_path = blobs_dir.join(&compressed_digest);
    fs::rename(&temp_path, &final_path).context("Failed to move layer blob to final location")?;

    Ok((compressed_digest, compressed_size, diff_id))
}

/// Build an uncompressed tar archive with the generation's content.
fn build_layer_tar(
    erofs_path: &Path,
    objects_dir: &Path,
    gen_dir: &Path,
    cas_objects: &[CasObjectRef],
) -> Result<Vec<u8>> {
    let buf = Vec::new();
    let mut tar_builder = tar::Builder::new(buf);

    // Add root.erofs
    let erofs_data = fs::read(erofs_path)
        .with_context(|| format!("Failed to read EROFS image at {}", erofs_path.display()))?;
    let mut header = tar::Header::new_gnu();
    header.set_size(erofs_data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder
        .append_data(&mut header, EROFS_IMAGE_NAME, erofs_data.as_slice())
        .context("Failed to add EROFS image to tar")?;

    // The generation artifact's CAS manifest is the export scope.
    for object in cas_objects {
        let hash = &object.sha256;
        let obj_path =
            object_path(objects_dir, hash).with_context(|| format!("Invalid CAS hash {hash}"))?;

        if !obj_path.exists() {
            bail!("Generation artifact references missing CAS object {hash}");
        }

        let prefix = &hash[..2];
        let suffix = &hash[2..];

        let data =
            fs::read(&obj_path).with_context(|| format!("Failed to read CAS object {hash}"))?;
        let mut obj_header = tar::Header::new_gnu();
        obj_header.set_size(data.len() as u64);
        obj_header.set_mode(0o644);
        obj_header.set_cksum();
        let tar_path = format!("objects/{prefix}/{suffix}");
        tar_builder
            .append_data(&mut obj_header, &tar_path, data.as_slice())
            .with_context(|| format!("Failed to add CAS object {hash} to tar"))?;
    }

    // Also include the generation metadata
    let meta_path = gen_dir.join(GENERATION_METADATA_FILE);
    let meta_data = fs::read(&meta_path).with_context(|| {
        format!(
            "Failed to read generation metadata at {}",
            meta_path.display()
        )
    })?;
    let mut meta_header = tar::Header::new_gnu();
    meta_header.set_size(meta_data.len() as u64);
    meta_header.set_mode(0o644);
    meta_header.set_cksum();
    tar_builder
        .append_data(
            &mut meta_header,
            GENERATION_METADATA_FILE,
            meta_data.as_slice(),
        )
        .context("Failed to add generation metadata to tar")?;

    let bytes = tar_builder
        .into_inner()
        .context("Failed to finalize tar archive")?;

    Ok(bytes)
}

/// Write a blob to the blobs directory, returning (hex_digest, size).
fn write_blob(blobs_dir: &Path, data: &[u8]) -> Result<(String, u64)> {
    let digest = hex_digest(data);
    let path = blobs_dir.join(&digest);
    fs::write(&path, data).with_context(|| format!("Failed to write blob {digest}"))?;
    Ok((digest, data.len() as u64))
}

/// Compute the hex-encoded SHA-256 digest of the given data.
fn hex_digest(data: &[u8]) -> String {
    conary_core::hash::sha256(data)
}

/// Build the OCI image config JSON.
///
/// Follows the OCI Image Configuration spec:
///   https://github.com/opencontainers/image-spec/blob/main/config.md
fn build_config_json(metadata: &GenerationMetadata, gen_number: i64, diff_id: &str) -> String {
    let created = &metadata.created_at;
    let pkg_count = metadata.package_count;
    let kernel = metadata.kernel_version.as_deref().unwrap_or("unknown");
    let summary = &metadata.summary;

    // Build the config as a JSON string to avoid pulling in a builder crate.
    // This is intentionally simple -- no entrypoint or cmd since this is a
    // system image, not an application container.
    serde_json::json!({
        "created": created,
        "architecture": oci_arch(),
        "os": "linux",
        "config": {
            "Labels": {
                "io.conary.generation": gen_number.to_string(),
                "io.conary.package-count": pkg_count.to_string(),
                "io.conary.kernel": kernel,
                "io.conary.summary": summary,
                "io.conary.format": "composefs-native"
            }
        },
        "rootfs": {
            "type": "layers",
            "diff_ids": [
                format!("sha256:{diff_id}")
            ]
        },
        "history": [
            {
                "created": created,
                "comment": format!("Conary generation {gen_number}: {summary}")
            }
        ]
    })
    .to_string()
}

/// Build the OCI image manifest JSON.
fn build_manifest_json(
    config_digest: &str,
    config_size: u64,
    layer_digest: &str,
    layer_size: u64,
) -> String {
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "config": {
            "mediaType": CONFIG_MEDIA_TYPE,
            "digest": format!("sha256:{config_digest}"),
            "size": config_size
        },
        "layers": [
            {
                "mediaType": LAYER_MEDIA_TYPE,
                "digest": format!("sha256:{layer_digest}"),
                "size": layer_size
            }
        ]
    })
    .to_string()
}

/// Build the OCI index JSON (top-level entry point).
fn build_index_json(manifest_digest: &str, manifest_size: u64) -> String {
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": INDEX_MEDIA_TYPE,
        "manifests": [
            {
                "mediaType": MANIFEST_MEDIA_TYPE,
                "digest": format!("sha256:{manifest_digest}"),
                "size": manifest_size,
                "platform": {
                    "architecture": oci_arch(),
                    "os": "linux"
                }
            }
        ]
    })
    .to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[path = "export/tests.rs"]
mod tests;
