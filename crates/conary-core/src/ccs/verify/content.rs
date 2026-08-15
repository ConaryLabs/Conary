// conary-core/src/ccs/verify/content.rs

//! Signed CCS object-set and reconstructed payload authority.
//!
//! Archive streaming and repository transport both terminate here so there is
//! one contract for object identities, ordered chunk reconstruction, and the
//! final whole-file digest and size.

use super::{VerifiedCcsArchive, VerifyError};
use crate::ccs::archive_layout::is_lower_hex;
use crate::ccs::v3::schema::{AuthorityDocumentV3, FileContentLayoutV3, PackageKindV3};
use crate::packages::payload::{PackagePayload, PackagePayloadFile, ReopenablePayload};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};

pub(crate) fn expected_objects(
    authority: &AuthorityDocumentV3,
) -> Result<(BTreeMap<String, u64>, usize)> {
    let PackageKindV3::Package(package) = &authority.kind else {
        return Ok((BTreeMap::new(), 0));
    };
    let mut objects = BTreeMap::new();
    for file in &package.files {
        if let Some(content) = &file.content
            && (content.sha256.len() != 64 || !is_lower_hex(&content.sha256))
        {
            return Err(VerifyError::PayloadInvalid(format!(
                "signed object digest {:?} is not canonical lowercase SHA-256",
                content.sha256
            ))
            .into());
        }
        file.node
            .validate_content(file.content.as_ref())
            .map_err(|error| {
                VerifyError::PayloadInvalid(format!(
                    "v3 payload authority for {} is invalid: {error}",
                    file.path
                ))
            })?;
        let declared = match &file.content_layout {
            FileContentLayoutV3::NoContent => Vec::new(),
            FileContentLayoutV3::WholeObject => file
                .content
                .as_ref()
                .map(|content| vec![(content.sha256.as_str(), content.size)])
                .unwrap_or_default(),
            FileContentLayoutV3::FastCdcV2020 { chunks, .. } => chunks
                .iter()
                .map(|chunk| (chunk.sha256.as_str(), u64::from(chunk.size)))
                .collect(),
        };
        for (sha256, size) in declared {
            if let Some(previous) = objects.insert(sha256.to_string(), size)
                && previous != size
            {
                return Err(VerifyError::PayloadInvalid(format!(
                    "signed object {sha256} carries conflicting sizes {previous} and {size}"
                ))
                .into());
            }
        }
    }
    Ok((objects, package.files.len()))
}

pub(crate) fn payload_from_authority(
    authority: &AuthorityDocumentV3,
    objects: &HashMap<String, ReopenablePayload>,
) -> Result<PackagePayload> {
    let PackageKindV3::Package(package) = &authority.kind else {
        return Ok(PackagePayload::default());
    };
    package
        .files
        .iter()
        .map(|file| {
            let source = payload_source(file, objects)?;
            PackagePayloadFile::new(
                file.path.clone(),
                file.node.clone(),
                file.content.clone(),
                source,
            )
            .map_err(anyhow::Error::from)
        })
        .collect::<Result<Vec<_>>>()
        .map(PackagePayload::new)
}

fn payload_source(
    file: &crate::ccs::v3::schema::FileAuthorityV3,
    objects: &HashMap<String, ReopenablePayload>,
) -> Result<Option<ReopenablePayload>> {
    let source = match &file.content_layout {
        FileContentLayoutV3::NoContent => return Ok(None),
        FileContentLayoutV3::WholeObject => {
            let content = file
                .content
                .as_ref()
                .context("whole-object layout has no content")?;
            objects.get(&content.sha256).cloned().ok_or_else(|| {
                VerifyError::PayloadInvalid(format!(
                    "missing authenticated object {} for {}",
                    content.sha256, file.path
                ))
            })?
        }
        FileContentLayoutV3::FastCdcV2020 { chunks, .. } => {
            let parts = chunks
                .iter()
                .map(|chunk| -> Result<ReopenablePayload> {
                    objects.get(&chunk.sha256).cloned().ok_or_else(|| {
                        VerifyError::PayloadInvalid(format!(
                            "missing authenticated chunk {} for {}",
                            chunk.sha256, file.path
                        ))
                        .into()
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            ReopenablePayload::from_concatenated_objects(parts)?
        }
    };
    verify_reconstructed_file(file, &source)?;
    Ok(Some(source))
}

fn verify_reconstructed_file(
    file: &crate::ccs::v3::schema::FileAuthorityV3,
    source: &ReopenablePayload,
) -> Result<()> {
    let FileContentLayoutV3::FastCdcV2020 {
        min_size,
        average_size,
        max_size,
        chunks,
    } = &file.content_layout
    else {
        return Ok(());
    };
    let content = file
        .content
        .as_ref()
        .context("chunked layout has no content")?;
    let chunker = crate::ccs::chunking::Chunker::with_sizes(*min_size, *average_size, *max_size);
    let mut whole_hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
    let mut index = 0_usize;
    let processed = chunker.visit_reader_chunks(source.open()?, |chunk| {
        let signed = chunks.get(index).with_context(|| {
            format!(
                "reconstructed {} produces an unsigned chunk at index {index}",
                file.path
            )
        })?;
        let actual = chunk.reference();
        if &actual != signed {
            anyhow::bail!(
                "reconstructed {} chunk {index} is {}/{}, signed authority requires {}/{}",
                file.path,
                actual.sha256,
                actual.size,
                signed.sha256,
                signed.size
            );
        }
        whole_hasher.update(&chunk.data);
        index += 1;
        Ok(())
    })?;
    if index != chunks.len() {
        return Err(VerifyError::PayloadInvalid(format!(
            "reconstructed {} produces {index} chunks, signed authority requires {}",
            file.path,
            chunks.len()
        ))
        .into());
    }
    let actual = whole_hasher.finalize().value;
    if processed != content.size || actual != content.sha256 {
        return Err(VerifyError::PayloadInvalid(format!(
            "reconstructed {} is {actual}/{processed}, signed whole-file authority requires {}/{}",
            file.path, content.sha256, content.size
        ))
        .into());
    }
    Ok(())
}

pub(crate) struct VerifiedArchiveParts {
    pub authority: AuthorityDocumentV3,
    pub signature: super::PackageSignature,
    pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    pub debug_toml: Option<Vec<u8>>,
    pub object_sources: HashMap<String, ReopenablePayload>,
    pub verified_object_metrics: Option<crate::filesystem::VerifiedObjectBatchMetrics>,
    pub files_checked: usize,
    pub raw: super::RawControlDocuments,
}

pub(crate) fn archive_from_parts(parts: VerifiedArchiveParts) -> Result<VerifiedCcsArchive> {
    let VerifiedArchiveParts {
        authority,
        signature,
        build_attestation,
        foreign_conversion_boundary,
        debug_toml,
        object_sources,
        verified_object_metrics,
        files_checked,
        raw,
    } = parts;
    let payload = payload_from_authority(&authority, &object_sources)?;
    let components = crate::ccs::v3::component_view::components(&authority);
    Ok(VerifiedCcsArchive {
        authority,
        signature,
        build_attestation,
        foreign_conversion_boundary,
        debug_toml,
        components,
        payload,
        object_sources,
        verified_object_metrics,
        files_checked,
        raw,
    })
}
