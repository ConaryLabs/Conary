// conary-core/src/ccs/builder/package_writer.rs

//! Package emission helpers for the CCS builder.
//!
//! `builder.rs` focuses on scanning and assembling build state; this module
//! owns the final archive-writing and manifest serialization steps.

use super::{BuildResult, PayloadPreparationMetrics, PreparedPayloadObjectSet};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Exact phase and work evidence from one streamed CCS v3 package emission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CcsPackageWriteMetrics {
    pub payload_derivation_and_object_staging: Duration,
    pub control_projection_and_signing: Duration,
    pub archive_assembly_and_gzip: Duration,
    pub payload_files_examined: u64,
    pub payload_chunks_derived: u64,
    pub unique_payload_chunks_derived: u64,
    pub payload_source_files_opened: u64,
    pub payload_source_bytes_read: u64,
    pub payload_source_files_reopened: u64,
    pub payload_source_bytes_reread: u64,
    pub payload_chunk_identity_bytes_hashed: u64,
    pub payload_whole_content_bytes_hashed: u64,
    pub payload_crypto_bytes_hashed: u64,
    pub staged_object_bytes_written: u64,
    pub staged_object_deduplicated_bytes: u64,
    pub staged_object_deduplications: u64,
    pub staged_unique_objects: u64,
    pub staged_object_canonical_bytes_reread: u64,
    pub staged_object_file_syncs: u64,
    pub staged_object_shard_syncs: u64,
    pub archive_members_traversed: u64,
    pub archive_input_bytes: u64,
    pub ccs_output_sha256: String,
    pub ccs_output_bytes: u64,
    pub ccs_output_bytes_hashed: u64,
    pub maximum_retained_staging_bytes: u64,
}

/// Explicit ceiling for test-only in-memory fixture input.
///
/// Native authoring and foreign conversion do not use this adapter. It exists
/// only for tests that already constructed small byte fixtures, immediately
/// spools those bytes to reopenable files, and delegates to the sole streamed
/// package writer.
pub(super) const BOUNDED_MEMORY_FIXTURE_BYTES: u64 = 16 * 1024 * 1024;

#[doc(hidden)]
pub fn write_v3_ccs_package_from_bounded_memory_for_tests(
    authority: &crate::ccs::v3::AuthorityDocumentV3,
    payloads_by_path: &std::collections::BTreeMap<String, Vec<u8>>,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    use crate::ccs::v3::schema::PackageKindV3;
    use crate::packages::payload::{PackagePayloadFile, ReopenablePayload};

    let total = payloads_by_path.values().try_fold(0_u64, |total, bytes| {
        total
            .checked_add(bytes.len() as u64)
            .context("bounded in-memory CCS fixture size overflow")
    })?;
    if total > BOUNDED_MEMORY_FIXTURE_BYTES {
        anyhow::bail!(
            "bounded in-memory CCS fixture input is {} bytes; limit is {} bytes",
            total,
            BOUNDED_MEMORY_FIXTURE_BYTES
        );
    }
    let output_parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let spool = Arc::new(
        tempfile::Builder::new()
            .prefix(".conary-ccs-memory-fixture-")
            .tempdir_in(output_parent)?,
    );
    let PackageKindV3::Package(package) = &authority.kind else {
        anyhow::bail!("v3 package writer only writes package payloads");
    };
    let mut payloads = Vec::with_capacity(package.files.len());
    let mut expected_paths = std::collections::HashSet::new();
    for (index, file) in package.files.iter().enumerate() {
        let source = if file.node.kind.is_regular() {
            let bytes = payloads_by_path
                .get(&file.path)
                .with_context(|| format!("missing payload for {}", file.path))?;
            let path = spool.path().join(format!("{index:08}"));
            fs::write(&path, bytes)?;
            expected_paths.insert(file.path.as_str());
            Some(ReopenablePayload::from_spooled_path(
                path,
                Arc::clone(&spool),
            ))
        } else {
            None
        };
        payloads.push(
            PackagePayloadFile::new(
                file.path.clone(),
                file.node.clone(),
                file.content.clone(),
                source,
            )
            .map_err(anyhow::Error::from)?,
        );
    }
    if let Some(extra) = payloads_by_path
        .keys()
        .find(|path| !expected_paths.contains(path.as_str()))
    {
        anyhow::bail!("in-memory fixture carries unsigned payload path {extra}");
    }
    write_v3_ccs_package_from_sources(
        authority,
        &payloads,
        output_path,
        signing_key,
        debug_toml,
        build_attestation,
        foreign_conversion_boundary,
    )
}

/// Emit a signed CCS archive from independently reopenable payload sources.
pub fn write_v3_ccs_package_from_sources(
    authority: &crate::ccs::v3::AuthorityDocumentV3,
    payloads: &[crate::packages::payload::PackagePayloadFile],
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    write_v3_ccs_package_from_sources_with_metrics(
        authority,
        payloads,
        output_path,
        signing_key,
        debug_toml,
        build_attestation,
        foreign_conversion_boundary,
    )
    .map(|_metrics| ())
}

/// Emit the sole signed CCS archive while retaining exact pass, byte, object,
/// traversal, and durability evidence for performance attribution.
pub fn write_v3_ccs_package_from_sources_with_metrics(
    authority: &crate::ccs::v3::AuthorityDocumentV3,
    payloads: &[crate::packages::payload::PackagePayloadFile],
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<CcsPackageWriteMetrics> {
    let output_parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let prepared =
        PreparedPayloadObjectSet::prepare_for_authority(payloads, authority, output_parent)?;
    write_v3_ccs_package_from_prepared_with_metrics(
        authority,
        &prepared,
        output_path,
        signing_key,
        debug_toml,
        build_attestation,
        foreign_conversion_boundary,
    )
}

/// Emit final controls and objects from one already authenticated preparation.
pub(crate) fn write_v3_ccs_package_from_prepared_with_metrics(
    authority: &crate::ccs::v3::AuthorityDocumentV3,
    prepared: &PreparedPayloadObjectSet,
    output_path: &Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<CcsPackageWriteMetrics> {
    use crate::ccs::budget::CCS_BUDGET;
    use crate::ccs::v3::schema::PackageKindV3;
    use std::collections::BTreeMap;

    let mut metrics = metrics_from_preparation(prepared.metrics());
    let control_started = Instant::now();
    // Authoring preflight and verification share one structural-budget owner,
    // so a package this writer emits is admissible to the reader by
    // construction rather than by coincidence.
    let census =
        crate::ccs::v3::authority_census(authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    prepared.reconcile_authority(authority)?;
    let PackageKindV3::Package(data) = &authority.kind else {
        anyhow::bail!("v3 writer only writes package payloads");
    };
    for file in &data.files {
        if !authority.components.contains_key(&file.component) {
            anyhow::bail!("missing component {}", file.component);
        }
    }
    let manifest_cbor = authority.to_cbor()?;
    CCS_BUDGET.admit_encoded_authority(&census, manifest_cbor.len() as u64)?;
    if let Some(debug_toml) = debug_toml {
        CCS_BUDGET.admit_control_bytes(
            crate::ccs::budget::BudgetDimension::DebugProjectionBytes,
            "MANIFEST.toml",
            debug_toml.len() as u64,
            CCS_BUDGET.debug_projection_bytes_ceiling(&census)?,
        )?;
    }
    let build_attestation_document = build_attestation
        .map(serde_json::to_string_pretty)
        .transpose()?;
    if let Some(encoded) = build_attestation_document.as_deref() {
        admit_attestation_document(encoded, "MANIFEST.attestation.json")?;
    }
    let foreign_conversion_boundary_document = foreign_conversion_boundary
        .map(serde_json::to_string_pretty)
        .transpose()?;
    if let Some(encoded) = foreign_conversion_boundary_document.as_deref() {
        admit_attestation_document(encoded, "MANIFEST.conversion-boundary.json")?;
    }
    let signature = signing_key.sign(&manifest_cbor);
    let signature_document = serde_json::to_string_pretty(&signature)?;
    CCS_BUDGET.admit_control_bytes(
        crate::ccs::budget::BudgetDimension::SignatureBytes,
        "MANIFEST.sig",
        signature_document.len() as u64,
        CCS_BUDGET.signature_bytes_ceiling(),
    )?;
    let mut controls = BTreeMap::new();
    controls.insert("MANIFEST", manifest_cbor.as_slice());
    if let Some(encoded) = build_attestation_document.as_deref() {
        controls.insert("MANIFEST.attestation.json", encoded.as_bytes());
    }
    if let Some(encoded) = foreign_conversion_boundary_document.as_deref() {
        controls.insert("MANIFEST.conversion-boundary.json", encoded.as_bytes());
    }
    controls.insert("MANIFEST.sig", signature_document.as_bytes());
    if let Some(debug_toml) = debug_toml {
        controls.insert("MANIFEST.toml", debug_toml.as_bytes());
    }
    metrics.control_projection_and_signing = control_started.elapsed();

    let archive_started = Instant::now();
    let timestamp = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_704_067_200);
    let emitted = crate::ccs::archive_emitter::write_exact_archive(
        output_path,
        timestamp,
        &controls,
        prepared.inventory(),
        |sha256| prepared.open_object(sha256),
    )?;
    metrics.archive_assembly_and_gzip = archive_started.elapsed();
    metrics.archive_members_traversed = emitted.members;
    metrics.archive_input_bytes = emitted.input_bytes;
    metrics.ccs_output_sha256 = emitted.output_sha256;
    metrics.ccs_output_bytes = emitted.output_bytes;
    metrics.ccs_output_bytes_hashed = emitted.output_bytes;
    metrics.maximum_retained_staging_bytes = checked_add(
        metrics.archive_input_bytes,
        metrics.ccs_output_bytes,
        "maximum retained CCS staging bytes",
    )?;
    Ok(metrics)
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64> {
    left.checked_add(right)
        .with_context(|| format!("{label} overflow"))
}

fn metrics_from_preparation(value: &PayloadPreparationMetrics) -> CcsPackageWriteMetrics {
    CcsPackageWriteMetrics {
        payload_derivation_and_object_staging: value.duration,
        payload_files_examined: value.files_examined,
        payload_chunks_derived: value.chunks_derived,
        unique_payload_chunks_derived: value.unique_chunks_derived,
        payload_source_files_opened: value.source_files_opened,
        payload_source_bytes_read: value.source_bytes_read,
        payload_source_files_reopened: value.source_files_reopened,
        payload_source_bytes_reread: value.source_bytes_reread,
        payload_chunk_identity_bytes_hashed: value.chunk_identity_bytes_hashed,
        payload_whole_content_bytes_hashed: value.whole_content_bytes_hashed,
        payload_crypto_bytes_hashed: value.crypto_bytes_hashed,
        staged_object_bytes_written: value.staged_object_bytes_written,
        staged_object_deduplicated_bytes: value.staged_object_deduplicated_bytes,
        staged_object_deduplications: value.staged_object_deduplications,
        staged_unique_objects: value.staged_unique_objects,
        staged_object_canonical_bytes_reread: value.staged_object_canonical_bytes_reread,
        ..Default::default()
    }
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
    let projected = crate::ccs::v3::project_build_result_to_v3(crate::ccs::v3::V3AuthoringInput {
        build: result,
        local_dev,
        debug_toml: Some(debug_toml),
    })
    .context("project current CCS v3 authority")?;
    let provenance = result.manifest.provenance.as_ref();
    write_v3_ccs_package_from_sources(
        &projected.authority,
        &projected.payloads,
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
    println!(
        "Payload sources: {} regular files",
        result
            .payloads
            .iter()
            .filter(|payload| payload.node.kind.is_regular())
            .count()
    );

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_v3_package_preserves_signed_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hello-v3.ccs");
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("hello-v3");
        let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
        let key = crate::ccs::signing::SigningKeyPair::generate();

        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority,
            &payloads,
            &path,
            &key,
            Some(
                "[package]\nname = \"hello-v3\"\nversion = \"1.0.0\"\nversion_scheme = \"conary\"\ndescription = \"debug\"\n",
            ),
            None,
            None,
        )
        .unwrap();

        let contents = crate::ccs::archive_reader::inspect_untrusted_ccs_archive(
            std::fs::File::open(path).unwrap(),
        )
        .unwrap();
        assert_eq!(contents.v3_authority.identity.name, "hello-v3");
        assert_eq!(contents.components["main"].files.len(), 1);
        assert_eq!(contents.census.files, 1);
        assert_eq!(contents.census.payload_objects, 1);
    }

    #[test]
    fn writer_emits_no_duplicated_component_projection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("no-projection.ccs");
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("nodup");
        let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
        let key = crate::ccs::signing::SigningKeyPair::generate();
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority, &payloads, &path, &key, None, None, None,
        )
        .unwrap();

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

    #[test]
    fn invalid_payload_never_replaces_an_existing_complete_archive() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("atomic.ccs");
        fs::write(&path, b"previous complete archive").unwrap();
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("atomic");
        let payloads = std::collections::BTreeMap::from([(
            "/usr/bin/hello".to_string(),
            b"wrong payload".to_vec(),
        )]);
        let key = crate::ccs::signing::SigningKeyPair::generate();

        assert!(
            write_v3_ccs_package_from_bounded_memory_for_tests(
                &authority, &payloads, &path, &key, None, None, None,
            )
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), b"previous complete archive");
    }

    #[test]
    fn generic_authoring_preserves_explicit_large_whole_object_layout() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large-whole.ccs");
        let bytes = vec![0x6d; crate::ccs::chunking::MIN_CHUNK_SIZE as usize * 2];
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("large-whole");
        let crate::ccs::v3::schema::PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().size = bytes.len() as u64;
        package.files[0].content.as_mut().unwrap().sha256 = crate::hash::sha256(&bytes);
        package.files[0].content_layout = crate::ccs::v3::schema::FileContentLayoutV3::WholeObject;
        authority.components.get_mut("main").unwrap().total_size = bytes.len() as u64;
        let payloads = std::collections::BTreeMap::from([("/usr/bin/hello".to_string(), bytes)]);
        let key = crate::ccs::signing::SigningKeyPair::generate();

        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority, &payloads, &path, &key, None, None, None,
        )
        .unwrap();
        let inspected = crate::ccs::archive_reader::inspect_untrusted_ccs_archive(
            fs::File::open(path).unwrap(),
        )
        .unwrap();
        let crate::ccs::v3::schema::PackageKindV3::Package(package) = &inspected.v3_authority.kind
        else {
            unreachable!()
        };
        assert_eq!(
            package.files[0].content_layout,
            crate::ccs::v3::schema::FileContentLayoutV3::WholeObject
        );
    }
}
