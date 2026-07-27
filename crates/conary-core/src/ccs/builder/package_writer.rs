// conary-core/src/ccs/builder/package_writer.rs

//! Package emission helpers for the CCS builder.
//!
//! `builder.rs` focuses on scanning and assembling build state; this module
//! owns the final archive-writing and manifest serialization steps.

use super::BuildResult;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn write_v2_ccs_package(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    payloads_by_path: &std::collections::BTreeMap<String, Vec<u8>>,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    write_v2_ccs_package_with_open(
        authority,
        output_path,
        signing_key,
        debug_toml,
        build_attestation,
        foreign_conversion_boundary,
        |path| {
            let payload = payloads_by_path
                .get(path)
                .with_context(|| format!("missing payload for {path}"))?;
            Ok(Box::new(std::io::Cursor::new(payload.as_slice())) as Box<dyn std::io::Read>)
        },
    )
}

/// Emit a signed CCS archive from independently reopenable payload sources.
pub fn write_v2_ccs_package_from_sources(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    payloads: &[crate::packages::payload::PackagePayloadFile],
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    let mut by_path = std::collections::HashMap::with_capacity(payloads.len());
    for file in payloads {
        if by_path.insert(file.path.as_str(), file).is_some() {
            anyhow::bail!("payload source path {} appears more than once", file.path);
        }
    }
    write_v2_ccs_package_with_open(
        authority,
        output_path,
        signing_key,
        debug_toml,
        build_attestation,
        foreign_conversion_boundary,
        |path| {
            by_path
                .get(path)
                .with_context(|| format!("missing payload source for {path}"))?
                .open_content()
                .map(|reader| reader as Box<dyn std::io::Read>)
                .map_err(anyhow::Error::from)
        },
    )
}

fn write_v2_ccs_package_with_open<'a>(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
    mut open_payload: impl FnMut(&str) -> Result<Box<dyn std::io::Read + 'a>>,
) -> Result<()> {
    use crate::ccs::budget::CCS_BUDGET;
    use crate::ccs::v2::schema::PackageKindV2;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;

    // Authoring preflight and verification share one structural-budget owner,
    // so a package this writer emits is admissible to the reader by
    // construction rather than by coincidence.
    let census =
        crate::ccs::v2::authority_census(authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    let temp_dir = tempfile::tempdir()?;
    let manifest_cbor = authority.to_cbor()?;
    CCS_BUDGET.admit_encoded_authority(&census, manifest_cbor.len() as u64)?;
    fs::write(temp_dir.path().join("MANIFEST"), &manifest_cbor)?;
    if let Some(debug_toml) = debug_toml {
        CCS_BUDGET.admit_control_bytes(
            crate::ccs::budget::BudgetDimension::DebugProjectionBytes,
            "MANIFEST.toml",
            debug_toml.len() as u64,
            CCS_BUDGET.debug_projection_bytes_ceiling(&census)?,
        )?;
        fs::write(temp_dir.path().join("MANIFEST.toml"), debug_toml)?;
    }
    if let Some(build_attestation) = build_attestation {
        let encoded = serde_json::to_string_pretty(build_attestation)?;
        admit_attestation_document(&encoded, "MANIFEST.attestation.json")?;
        fs::write(temp_dir.path().join("MANIFEST.attestation.json"), encoded)?;
    }
    if let Some(foreign_conversion_boundary) = foreign_conversion_boundary {
        let encoded = serde_json::to_string_pretty(foreign_conversion_boundary)?;
        admit_attestation_document(&encoded, "MANIFEST.conversion-boundary.json")?;
        fs::write(
            temp_dir.path().join("MANIFEST.conversion-boundary.json"),
            encoded,
        )?;
    }
    let signature = signing_key.sign(&manifest_cbor);
    let signature_document = serde_json::to_string_pretty(&signature)?;
    CCS_BUDGET.admit_control_bytes(
        crate::ccs::budget::BudgetDimension::SignatureBytes,
        "MANIFEST.sig",
        signature_document.len() as u64,
        CCS_BUDGET.signature_bytes_ceiling(),
    )?;
    fs::write(temp_dir.path().join("MANIFEST.sig"), signature_document)?;

    let PackageKindV2::Package(data) = &authority.kind else {
        anyhow::bail!("M4a v2 writer only writes package payloads");
    };

    let objects_dir = temp_dir.path().join("objects");
    let object_store = crate::filesystem::CasStore::new(&objects_dir)?;
    for file in &data.files {
        if !authority.components.contains_key(&file.component) {
            anyhow::bail!("missing component {}", file.component);
        }
        if file.node.kind.is_regular() {
            let content = file.content.as_ref().with_context(|| {
                format!(
                    "regular payload node {} has no content authority",
                    file.path
                )
            })?;
            let mut reader = open_payload(&file.path)?;
            object_store
                .store_reader_expected(reader.as_mut(), content.size, &content.sha256)
                .with_context(|| {
                    format!(
                        "payload for {} does not match signed v2 authority",
                        file.path
                    )
                })?;
        }
    }

    let output_file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);
    let timestamp = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_704_067_200);
    append_dir_with_mtime(&mut archive, temp_dir.path(), "", timestamp)?;
    archive.into_inner()?.finish()?;
    Ok(())
}

/// Admit one attestation-class control document against the shared budget.
fn admit_attestation_document(encoded: &str, label: &str) -> Result<()> {
    use crate::ccs::budget::{BudgetDimension, CCS_BUDGET};

    CCS_BUDGET.admit_control_bytes(
        BudgetDimension::AttestationBytes,
        label,
        encoded.len() as u64,
        CCS_BUDGET.attestation_bytes_ceiling(),
    )?;
    Ok(())
}

/// Project a builder result into the sole current CCS archive contract and
/// authenticate it with the supplied authority key.
pub fn write_signed_current_ccs_package(
    result: &BuildResult,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    local_dev: bool,
) -> Result<()> {
    let debug_toml = result
        .manifest
        .to_toml()
        .context("serialize CCS diagnostic projection")?;
    let projected = crate::ccs::v2::project_build_result_to_v2(crate::ccs::v2::V2AuthoringInput {
        build: result,
        local_dev,
        debug_toml: Some(debug_toml),
    })
    .context("project current CCS v2 authority")?;
    let provenance = result.manifest.provenance.as_ref();
    write_v2_ccs_package(
        &projected.authority,
        &projected.payloads_by_path,
        output_path,
        signing_key,
        projected.debug_toml.as_deref(),
        provenance.and_then(|value| value.build_attestation.as_ref()),
        provenance.and_then(|value| value.foreign_conversion_boundary.as_ref()),
    )
}

/// Print a concise build summary.
pub fn print_build_summary(result: &BuildResult) {
    println!();
    println!("Build Summary");
    println!("=============");
    println!();
    println!(
        "Package: {} v{}",
        result.manifest.package.name, result.manifest.package.version
    );
    println!("Total files: {}", result.files.len());
    println!("Total size: {} bytes", result.total_size);
    println!("Blobs: {} objects", result.blobs.len());

    if let Some(ref stats) = result.chunk_stats {
        println!();
        println!("CDC Chunking:");
        println!("  Chunked files: {} (files >16KB)", stats.chunked_files);
        println!("  Whole files: {} (files ≤16KB)", stats.whole_files);
        println!("  Total chunks: {}", stats.total_chunks);
        println!("  Unique chunks: {}", stats.unique_chunks);
        if stats.dedup_savings > 0 {
            println!("  Intra-package dedup: {} bytes saved", stats.dedup_savings);
        }
    }

    println!();
    println!("Components:");

    let mut comp_names: Vec<_> = result.components.keys().collect();
    comp_names.sort();

    for name in comp_names {
        let comp = &result.components[name];
        println!(
            "  :{} - {} files ({} bytes)",
            name,
            comp.files.len(),
            comp.size
        );
    }
}

fn append_dir_with_mtime<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    base_path: &Path,
    archive_path: &str,
    mtime: u64,
) -> Result<()> {
    let mut entries = Vec::new();
    collect_archive_entries(base_path, archive_path, &mut entries)?;
    for directories in [true, false] {
        for (path, entry_archive_path, file_type) in &entries {
            if file_type.is_dir() != directories {
                continue;
            }
            if file_type.is_dir() {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                header.set_mtime(mtime);
                header.set_cksum();

                archive.append_data(&mut header, entry_archive_path, std::io::empty())?;
            } else if file_type.is_file() {
                let metadata = fs::metadata(path)?;
                let mut content = fs::File::open(path)?;

                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(metadata.permissions().mode());
                header.set_size(metadata.len());
                header.set_mtime(mtime);
                header.set_cksum();

                archive.append_data(&mut header, entry_archive_path, &mut content)?;
            } else if file_type.is_symlink() {
                let target = fs::read_link(path)?;

                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_mtime(mtime);
                header.set_cksum();

                archive.append_link(&mut header, entry_archive_path, &target)?;
            }
        }
    }
    Ok(())
}

fn collect_archive_entries(
    base_path: &Path,
    archive_path: &str,
    collected: &mut Vec<(std::path::PathBuf, String, std::fs::FileType)>,
) -> Result<()> {
    let mut entries = fs::read_dir(base_path)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        let entry_archive_path = if archive_path.is_empty() {
            file_name_str.to_string()
        } else {
            format!("{}/{}", archive_path, file_name_str)
        };
        let path = entry.path();
        collected.push((path.clone(), entry_archive_path.clone(), file_type));
        if file_type.is_dir() {
            collect_archive_entries(&path, &entry_archive_path, collected)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_v2_package_preserves_signed_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hello-v2.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("hello-v2");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let key = crate::ccs::signing::SigningKeyPair::generate();

        write_v2_ccs_package(
            &authority,
            &payloads,
            &path,
            &key,
            Some(
                "[package]\nname = \"hello-v2\"\nversion = \"1.0.0\"\nversion_scheme = \"conary\"\ndescription = \"debug\"\n",
            ),
            None,
            None,
        )
        .unwrap();

        let contents = crate::ccs::archive_reader::inspect_untrusted_ccs_archive(
            std::fs::File::open(path).unwrap(),
        )
        .unwrap();
        assert_eq!(contents.v2_authority.identity.name, "hello-v2");
        assert_eq!(contents.components["main"].files.len(), 1);
        assert_eq!(contents.census.files, 1);
        assert_eq!(contents.census.payload_objects, 1);
    }

    #[test]
    fn writer_emits_no_duplicated_component_projection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("no-projection.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("nodup");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let key = crate::ccs::signing::SigningKeyPair::generate();
        write_v2_ccs_package(&authority, &payloads, &path, &key, None, None, None).unwrap();

        let mut archive =
            tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(&path).unwrap()));
        let entries = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().display().to_string(),
                    entry.header().entry_type().is_file(),
                )
            })
            .collect::<Vec<_>>();

        assert!(
            !entries
                .iter()
                .any(|(path, _)| path.starts_with(crate::ccs::archive_layout::COMPONENTS_DIR)),
            "{entries:?}"
        );
        let (first_file, _) = entries
            .iter()
            .find(|(_, is_file)| *is_file)
            .expect("archive carries files");
        assert_eq!(first_file, "MANIFEST", "{entries:?}");
    }
}
