// crates/conary-core/src/ccs/builder/payload_preparation.rs

//! One-pass payload layout derivation and operation-private object staging.

use crate::ccs::budget::CCS_BUDGET;
use crate::ccs::chunking::{AVG_CHUNK_SIZE, ChunkReference, MAX_CHUNK_SIZE, MIN_CHUNK_SIZE};
use crate::ccs::v3::schema::{AuthorityDocumentV3, FileContentLayoutV3, PackageKindV3};
use crate::filesystem::{EphemeralObjectStageMetrics, EphemeralObjectStore};
use crate::packages::payload::PackagePayloadFile;
use crate::payload::{PayloadContentAuthority, PayloadNode};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// Exact physical work from the sole payload preparation pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PayloadPreparationMetrics {
    pub(crate) duration: Duration,
    pub(crate) files_examined: u64,
    pub(crate) chunks_derived: u64,
    pub(crate) unique_chunks_derived: u64,
    pub(crate) source_files_opened: u64,
    pub(crate) source_bytes_read: u64,
    pub(crate) source_files_reopened: u64,
    pub(crate) source_bytes_reread: u64,
    pub(crate) chunk_identity_bytes_hashed: u64,
    pub(crate) whole_content_bytes_hashed: u64,
    pub(crate) crypto_bytes_hashed: u64,
    pub(crate) staged_object_bytes_written: u64,
    pub(crate) staged_object_deduplicated_bytes: u64,
    pub(crate) staged_object_deduplications: u64,
    pub(crate) staged_unique_objects: u64,
    pub(crate) staged_object_canonical_bytes_reread: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedFileEvidence {
    node: PayloadNode,
    content: Option<PayloadContentAuthority>,
    layout: FileContentLayoutV3,
}

/// Disposable authenticated object evidence retained until final archive emit.
pub(crate) struct PreparedPayloadObjectSet {
    _workspace: tempfile::TempDir,
    store: EphemeralObjectStore,
    files: BTreeMap<String, PreparedFileEvidence>,
    metrics: PayloadPreparationMetrics,
}

impl PreparedPayloadObjectSet {
    /// Prepare every payload source once under the canonical CCS budget.
    pub(crate) fn prepare(payloads: &[PackagePayloadFile], output_parent: &Path) -> Result<Self> {
        Self::prepare_with_layouts(payloads, output_parent, None)
    }

    /// Prepare generic authoring sources against their projected layout.
    pub(crate) fn prepare_for_authority(
        payloads: &[PackagePayloadFile],
        authority: &AuthorityDocumentV3,
        output_parent: &Path,
    ) -> Result<Self> {
        crate::ccs::v3::authority_census(authority).map_err(|error| anyhow::anyhow!("{error}"))?;
        let PackageKindV3::Package(package) = &authority.kind else {
            bail!("prepared payload objects require v3 package authority");
        };
        if package.files.len() != payloads.len() {
            bail!(
                "payload source file count {} disagrees with projected v3 authority {}",
                payloads.len(),
                package.files.len()
            );
        }
        let mut payload_by_path = BTreeMap::new();
        for payload in payloads {
            if payload_by_path
                .insert(payload.path.as_str(), payload)
                .is_some()
            {
                bail!(
                    "payload source path {} appears more than once",
                    payload.path
                );
            }
        }
        let mut layouts = BTreeMap::new();
        for file in &package.files {
            let payload = payload_by_path.get(file.path.as_str()).with_context(|| {
                format!("projected payload {} has no source descriptor", file.path)
            })?;
            if payload.node != file.node || payload.content_authority != file.content {
                bail!(
                    "payload source evidence for {} disagrees with projected v3 authority",
                    file.path
                );
            }
            if layouts
                .insert(file.path.clone(), file.content_layout.clone())
                .is_some()
            {
                bail!(
                    "projected payload path {} appears more than once",
                    file.path
                );
            }
        }
        let prepared = Self::prepare_with_layouts(payloads, output_parent, Some(&layouts))?;
        prepared.reconcile_authority(authority)?;
        Ok(prepared)
    }

    fn prepare_with_layouts(
        payloads: &[PackagePayloadFile],
        output_parent: &Path,
        projected_layouts: Option<&BTreeMap<String, FileContentLayoutV3>>,
    ) -> Result<Self> {
        let started = Instant::now();
        fs::create_dir_all(output_parent)?;
        let bounds = CCS_BUDGET.archive_decode_bounds()?;
        let files_examined =
            u64::try_from(payloads.len()).context("payload file count exceeds u64 authority")?;
        if files_examined > CCS_BUDGET.max_files {
            bail!(
                "payload file count {files_examined} exceeds CCS budget {}",
                CCS_BUDGET.max_files
            );
        }

        let mut seen_paths = BTreeSet::new();
        let mut logical_payload_bytes = 0_u64;
        for payload in payloads {
            if !seen_paths.insert(payload.path.as_str()) {
                bail!(
                    "payload source path {} appears more than once",
                    payload.path
                );
            }
            payload
                .node
                .validate_content(payload.content_authority.as_ref())
                .map_err(|error| anyhow::anyhow!("invalid payload {}: {error}", payload.path))?;
            if let Some(content) = &payload.content_authority {
                logical_payload_bytes = bounds.add_payload_bytes(
                    "prepared payload logical bytes",
                    logical_payload_bytes,
                    content.size,
                )?;
            }
        }

        let workspace = tempfile::Builder::new()
            .prefix(".conary-ccs-payload-")
            .tempdir_in(output_parent)?;
        let mut store = EphemeralObjectStore::new(workspace.path().join("objects"))?;
        let mut files = BTreeMap::new();
        let mut metrics = PayloadPreparationMetrics {
            files_examined,
            ..Default::default()
        };
        let mut unique_chunk_identities = HashSet::new();
        let mut payload_references = 0_u64;
        let mut unique_staged_bytes = 0_u64;

        for payload in payloads {
            let layout = projected_layouts
                .map(|layouts| {
                    layouts.get(&payload.path).cloned().with_context(|| {
                        format!("payload {} has no projected content layout", payload.path)
                    })
                })
                .transpose()?
                .unwrap_or_else(|| match &payload.content_authority {
                    Some(content) if content.size >= u64::from(MIN_CHUNK_SIZE) => {
                        FileContentLayoutV3::FastCdcV2020 {
                            min_size: MIN_CHUNK_SIZE,
                            average_size: AVG_CHUNK_SIZE,
                            max_size: MAX_CHUNK_SIZE,
                            chunks: Vec::new(),
                        }
                    }
                    Some(_) => FileContentLayoutV3::WholeObject,
                    None => FileContentLayoutV3::NoContent,
                });
            let evidence = match (&payload.content_authority, layout) {
                (
                    Some(content),
                    FileContentLayoutV3::FastCdcV2020 {
                        min_size,
                        average_size,
                        max_size,
                        ..
                    },
                ) => {
                    let mut reader = payload
                        .open_content()
                        .map_err(anyhow::Error::from)
                        .with_context(|| {
                            format!("open payload {} for canonical preparation", payload.path)
                        })?;
                    metrics.source_files_opened =
                        checked_add(metrics.source_files_opened, 1, "payload source open count")?;
                    let mut whole_hasher =
                        crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
                    let mut references = Vec::new();
                    let processed =
                        crate::ccs::chunking::Chunker::with_sizes(min_size, average_size, max_size)
                            .visit_reader_chunks(reader.as_mut(), |chunk| {
                                let reference = chunk.reference();
                                payload_references =
                                    checked_add(payload_references, 1, "payload reference count")?;
                                bounds.admit_payload_references(
                                    "prepared payload references",
                                    payload_references,
                                )?;
                                if !store.inventory().contains_key(&reference.sha256) {
                                    unique_staged_bytes = bounds.add_payload_bytes(
                                        "prepared unique object bytes",
                                        unique_staged_bytes,
                                        u64::from(reference.size),
                                    )?;
                                }
                                whole_hasher.update(chunk.data());
                                metrics.chunk_identity_bytes_hashed = checked_add(
                                    metrics.chunk_identity_bytes_hashed,
                                    u64::from(chunk.length()),
                                    "payload chunk identity hash bytes",
                                )?;
                                metrics.chunks_derived =
                                    checked_add(metrics.chunks_derived, 1, "payload chunk count")?;
                                unique_chunk_identities.insert(reference.sha256.clone());
                                let staged =
                                    store.stage_chunk(chunk).map_err(anyhow::Error::from)?;
                                add_stage_metrics(&mut metrics, staged)?;
                                references.push(reference);
                                Ok(())
                            })
                            .with_context(|| {
                                format!("derive and stage canonical payload {}", payload.path)
                            })?;
                    let actual = whole_hasher.finalize().value;
                    if processed != content.size || actual != content.sha256 {
                        bail!(
                            "payload {} is {actual}/{processed}, native content authority requires {}/{}",
                            payload.path,
                            content.sha256,
                            content.size
                        );
                    }
                    metrics.source_bytes_read = checked_add(
                        metrics.source_bytes_read,
                        processed,
                        "payload source bytes read",
                    )?;
                    metrics.whole_content_bytes_hashed = checked_add(
                        metrics.whole_content_bytes_hashed,
                        processed,
                        "payload whole-content hash bytes",
                    )?;
                    PreparedFileEvidence {
                        node: payload.node.clone(),
                        content: Some(content.clone()),
                        layout: FileContentLayoutV3::FastCdcV2020 {
                            min_size,
                            average_size,
                            max_size,
                            chunks: references,
                        },
                    }
                }
                (Some(content), FileContentLayoutV3::WholeObject) => {
                    payload_references =
                        checked_add(payload_references, 1, "payload reference count")?;
                    bounds.admit_payload_references(
                        "prepared payload references",
                        payload_references,
                    )?;
                    if !store.inventory().contains_key(&content.sha256) {
                        unique_staged_bytes = bounds.add_payload_bytes(
                            "prepared unique object bytes",
                            unique_staged_bytes,
                            content.size,
                        )?;
                    }
                    let mut reader = payload
                        .open_content()
                        .map_err(anyhow::Error::from)
                        .with_context(|| {
                            format!(
                                "open whole payload {} for canonical preparation",
                                payload.path
                            )
                        })?;
                    metrics.source_files_opened =
                        checked_add(metrics.source_files_opened, 1, "payload source open count")?;
                    let staged = store
                        .stage_reader_expected_once(reader.as_mut(), content.size, &content.sha256)
                        .map_err(anyhow::Error::from)
                        .with_context(|| {
                            format!("authenticate and stage whole payload {}", payload.path)
                        })?;
                    metrics.source_bytes_read = checked_add(
                        metrics.source_bytes_read,
                        content.size,
                        "payload source bytes read",
                    )?;
                    metrics.whole_content_bytes_hashed = checked_add(
                        metrics.whole_content_bytes_hashed,
                        staged.object_identity_bytes_hashed,
                        "payload whole-content hash bytes",
                    )?;
                    add_stage_metrics(&mut metrics, staged)?;
                    PreparedFileEvidence {
                        node: payload.node.clone(),
                        content: Some(content.clone()),
                        layout: FileContentLayoutV3::WholeObject,
                    }
                }
                (None, FileContentLayoutV3::NoContent) => PreparedFileEvidence {
                    node: payload.node.clone(),
                    content: None,
                    layout: FileContentLayoutV3::NoContent,
                },
                (Some(_), FileContentLayoutV3::NoContent) => {
                    bail!(
                        "regular payload {} projects no content layout",
                        payload.path
                    )
                }
                (None, FileContentLayoutV3::WholeObject)
                | (None, FileContentLayoutV3::FastCdcV2020 { .. }) => {
                    bail!(
                        "non-regular payload {} projects object content authority",
                        payload.path
                    )
                }
            };
            files.insert(payload.path.clone(), evidence);
        }

        metrics.unique_chunks_derived = u64::try_from(unique_chunk_identities.len())
            .context("unique chunk count exceeds u64 authority")?;
        let inventoried_objects = u64::try_from(store.inventory().len())
            .context("staged object count exceeds u64 authority")?;
        if metrics.staged_unique_objects != inventoried_objects {
            bail!(
                "staged object miss count {} disagrees with authored inventory {inventoried_objects}",
                metrics.staged_unique_objects
            );
        }
        if inventoried_objects > CCS_BUDGET.max_payload_objects() {
            bail!(
                "staged object count {} exceeds CCS budget {}",
                inventoried_objects,
                CCS_BUDGET.max_payload_objects()
            );
        }
        if unique_staged_bytes != metrics.staged_object_bytes_written {
            bail!(
                "prepared unique object census {unique_staged_bytes} disagrees with staged writes {}",
                metrics.staged_object_bytes_written
            );
        }
        metrics.crypto_bytes_hashed = checked_add(
            metrics.chunk_identity_bytes_hashed,
            metrics.whole_content_bytes_hashed,
            "aggregate payload cryptographic hash bytes",
        )?;
        metrics.duration = started.elapsed();

        Ok(Self {
            _workspace: workspace,
            store,
            files,
            metrics,
        })
    }

    pub(crate) fn metrics(&self) -> &PayloadPreparationMetrics {
        &self.metrics
    }

    pub(crate) fn chunks_for(&self, path: &str) -> Result<Option<Vec<ChunkReference>>> {
        let evidence = self
            .files
            .get(path)
            .with_context(|| format!("prepared payload evidence is missing {path}"))?;
        Ok(match &evidence.layout {
            FileContentLayoutV3::FastCdcV2020 { chunks, .. } => Some(chunks.clone()),
            FileContentLayoutV3::WholeObject | FileContentLayoutV3::NoContent => None,
        })
    }

    /// Fail closed unless prepared layouts and objects exactly equal authority.
    pub(crate) fn reconcile_authority(&self, authority: &AuthorityDocumentV3) -> Result<()> {
        let PackageKindV3::Package(package) = &authority.kind else {
            bail!("prepared payload objects require v3 package authority");
        };
        if package.files.len() != self.files.len() {
            bail!(
                "prepared payload file count {} disagrees with signed authority {}",
                self.files.len(),
                package.files.len()
            );
        }
        let mut seen = BTreeSet::new();
        for file in &package.files {
            if !seen.insert(file.path.as_str()) {
                bail!("signed payload path {} appears more than once", file.path);
            }
            let prepared = self.files.get(&file.path).with_context(|| {
                format!(
                    "signed payload {} has no prepared source evidence",
                    file.path
                )
            })?;
            if prepared.node != file.node
                || prepared.content != file.content
                || prepared.layout != file.content_layout
            {
                bail!(
                    "prepared payload evidence for {} disagrees with projected v3 authority",
                    file.path
                );
            }
        }
        let (expected, _) = crate::ccs::verify::content::expected_objects(authority)?;
        reconcile_object_inventory(&expected, self.store.inventory())
    }

    pub(crate) fn inventory(&self) -> &BTreeMap<String, u64> {
        self.store.inventory()
    }

    pub(crate) fn open_object(&self, sha256: &str) -> Result<Box<dyn Read + Send>> {
        self.store.open_object(sha256).map_err(anyhow::Error::from)
    }
}

fn reconcile_object_inventory(
    expected: &BTreeMap<String, u64>,
    prepared: &BTreeMap<String, u64>,
) -> Result<()> {
    if expected == prepared {
        return Ok(());
    }
    let missing = expected
        .keys()
        .find(|sha256| !prepared.contains_key(*sha256));
    let extra = prepared
        .keys()
        .find(|sha256| !expected.contains_key(*sha256));
    let conflicting_size = expected.iter().find_map(|(sha256, expected_size)| {
        prepared
            .get(sha256)
            .filter(|prepared_size| *prepared_size != expected_size)
            .map(|prepared_size| format!("{sha256}:{expected_size}!={prepared_size}"))
    });
    bail!(
        "prepared object inventory disagrees with projected v3 authority (missing: {}, extra: {}, conflicting size: {})",
        missing.map_or("none", String::as_str),
        extra.map_or("none", String::as_str),
        conflicting_size.as_deref().unwrap_or("none")
    )
}

fn add_stage_metrics(
    total: &mut PayloadPreparationMetrics,
    value: EphemeralObjectStageMetrics,
) -> Result<()> {
    total.staged_object_bytes_written = checked_add(
        total.staged_object_bytes_written,
        value.unique_bytes_written,
        "staged object write bytes",
    )?;
    total.staged_object_deduplicated_bytes = checked_add(
        total.staged_object_deduplicated_bytes,
        value.deduplicated_bytes_avoided,
        "staged object deduplicated bytes",
    )?;
    total.staged_object_deduplications = checked_add(
        total.staged_object_deduplications,
        value.hits,
        "staged object deduplication count",
    )?;
    total.staged_unique_objects = checked_add(
        total.staged_unique_objects,
        value.misses,
        "staged unique object count",
    )?;
    total.staged_object_canonical_bytes_reread = checked_add(
        total.staged_object_canonical_bytes_reread,
        value.canonical_bytes_reread,
        "staged object canonical reread bytes",
    )?;
    Ok(())
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64> {
    left.checked_add(right)
        .with_context(|| format!("{label} overflow"))
}

#[cfg(test)]
mod tests;
