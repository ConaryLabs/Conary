// conary-core/src/ccs/convert/payload_preparation.rs

//! Conversion projection built from the sole prepared payload pass.

use super::converter::ConversionError;
use crate::ccs::builder::{
    BuildResult, ChunkStats, ComponentData, FileEntry, PreparedPayloadObjectSet,
};
use crate::ccs::manifest::CcsManifest;
use crate::packages::payload::PackagePayloadFile;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(super) fn prepare_streaming_result(
    manifest: CcsManifest,
    payloads: &[PackagePayloadFile],
    output_parent: &Path,
) -> Result<(BuildResult, PreparedPayloadObjectSet), ConversionError> {
    let prepared = PreparedPayloadObjectSet::prepare(payloads, output_parent).map_err(|error| {
        ConversionError::BuildError(format!(
            "failed to derive and stage conversion payload: {error:#}"
        ))
    })?;
    let mut files = Vec::with_capacity(payloads.len());
    let mut total_size = 0_u64;
    let mut chunk_stats = ChunkStats::default();
    let mut unique_chunks = HashSet::new();
    for payload in payloads {
        if let Some(content) = &payload.content_authority {
            total_size = total_size.checked_add(content.size).ok_or_else(|| {
                ConversionError::BuildError("CCS payload size arithmetic overflow".to_string())
            })?;
        }
        let chunks = prepared.chunks_for(&payload.path).map_err(|error| {
            ConversionError::BuildError(format!(
                "prepared conversion payload {} lost layout evidence: {error}",
                payload.path
            ))
        })?;
        if let Some(chunks) = &chunks {
            chunk_stats.chunked_files += 1;
            chunk_stats.total_chunks = chunk_stats
                .total_chunks
                .checked_add(chunks.len())
                .ok_or_else(|| {
                    ConversionError::BuildError(
                        "conversion chunk count arithmetic overflow".to_string(),
                    )
                })?;
            for chunk in chunks {
                if !unique_chunks.insert(chunk.sha256.clone()) {
                    chunk_stats.dedup_savings = chunk_stats
                        .dedup_savings
                        .checked_add(u64::from(chunk.size))
                        .ok_or_else(|| {
                            ConversionError::BuildError(
                                "conversion chunk dedup savings overflow".to_string(),
                            )
                        })?;
                }
            }
        } else if payload.node.kind.is_regular() {
            chunk_stats.whole_files += 1;
        }
        files.push(FileEntry {
            path: payload.path.clone(),
            node: payload.node.clone(),
            content: payload.content_authority.clone(),
            component: "runtime".to_string(),
            chunks,
        });
    }
    chunk_stats.unique_chunks = unique_chunks.len();
    let component_hash = crate::hash::sha256_prefixed(
        &crate::ccs::attestation::canonical_json_bytes(&files)
            .map_err(|error| ConversionError::BuildError(error.to_string()))?,
    );
    let components = if files.is_empty() {
        HashMap::new()
    } else {
        HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: component_hash,
                size: total_size,
            },
        )])
    };
    Ok((
        BuildResult {
            manifest,
            components,
            files,
            payloads: payloads.to_vec(),
            total_size,
            chunked: chunk_stats.chunked_files > 0,
            chunk_stats: Some(chunk_stats),
        },
        prepared,
    ))
}
