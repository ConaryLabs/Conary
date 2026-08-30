// conary-core/src/ccs/builder/payload_preparation.rs

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
mod tests {
    use super::*;
    use crate::packages::payload::ReopenablePayload;
    use std::sync::Arc;

    fn payload(path: &str, bytes: &[u8]) -> PackagePayloadFile {
        payload_with_authority(path, bytes, bytes)
    }

    fn payload_with_authority(
        path: &str,
        source_bytes: &[u8],
        authority_bytes: &[u8],
    ) -> PackagePayloadFile {
        PackagePayloadFile::new(
            path.to_string(),
            PayloadNode::regular(0o644),
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(authority_bytes),
                size: authority_bytes.len() as u64,
            }),
            Some(ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(
                source_bytes.to_vec(),
            ))),
        )
        .unwrap()
    }

    fn stable_object_list_fixture() -> Vec<u8> {
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        (0..768 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    fn chunked_authority(
        path: &str,
        bytes: &[u8],
        chunks: Vec<ChunkReference>,
    ) -> AuthorityDocumentV3 {
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("prepared-chunks");
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].path = path.to_string();
        package.files[0].node = PayloadNode::regular(0o644);
        package.files[0].content = Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        });
        package.files[0].content_layout = FileContentLayoutV3::FastCdcV2020 {
            min_size: MIN_CHUNK_SIZE,
            average_size: AVG_CHUNK_SIZE,
            max_size: MAX_CHUNK_SIZE,
            chunks,
        };
        authority.components.get_mut("main").unwrap().total_size = bytes.len() as u64;
        authority
    }

    #[test]
    fn streamed_preparation_preserves_the_stable_full_object_list() {
        let bytes = stable_object_list_fixture();
        let temp = tempfile::tempdir().unwrap();
        let prepared = PreparedPayloadObjectSet::prepare(
            &[payload("/stable-object-list", &bytes)],
            temp.path(),
        )
        .unwrap();
        let actual = prepared.chunks_for("/stable-object-list").unwrap().unwrap();
        let expected = [
            (
                "04bea0eadad0893ab43b0db5e503ce9e575c0a79e2f1d4a04b79956a961a15ac",
                75_896,
            ),
            (
                "29fb403e0548031d0ac6a33a4d3ecd6ae3e54862c123ab355d735d5f39c9267c",
                21_072,
            ),
            (
                "04113790af2f2a345556d84c787edebef410ea8d03f6e893ee9566487cc60ddc",
                67_367,
            ),
            (
                "795c90d7b69a8452a2867af86970553e448ac20fb6a46ad257df711df6972f94",
                52_640,
            ),
            (
                "59d3aca68160be78a15814d9b412558d8899d0886de023af25ba932fcfd3b0c4",
                71_617,
            ),
            (
                "a8985d49674f2d622157a9444aebcd1e44c02a06ab64093dd8f96171196ac49e",
                85_160,
            ),
            (
                "6aee9cde4406f81fcf860902cde51f5354b11bb447e95ead25e8206913ee5fa7",
                98_811,
            ),
            (
                "e87f4b64b96319c6763891cc36a475bb51917442e5e2c36abe7dcc3066fd007f",
                73_038,
            ),
            (
                "f2ce3db37b04cf68eda67cf51b04252bc3dc40dea2a48b0eae62453f3abb5774",
                27_502,
            ),
            (
                "65f0fbad0221ab4e2a6380893d3a540ef75f08b921a2755a27212afb7461935b",
                80_029,
            ),
            (
                "f8bac509de2f75838219c539a0fa99f0bb80dde8753a7ecc08edc1f8f5ce1314",
                78_124,
            ),
            (
                "14dd8c75fc6f005d288a5367a9d56f37403b583905110b650ec92a116417c490",
                55_176,
            ),
        ]
        .map(|(sha256, size)| ChunkReference {
            sha256: sha256.to_string(),
            size,
        });

        assert_eq!(actual, expected);
        let expected_inventory = expected
            .iter()
            .map(|reference| (reference.sha256.clone(), u64::from(reference.size)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(prepared.inventory(), &expected_inventory);

        let mut reconstructed = Vec::with_capacity(bytes.len());
        for reference in &expected {
            prepared
                .open_object(&reference.sha256)
                .unwrap()
                .read_to_end(&mut reconstructed)
                .unwrap();
        }
        assert_eq!(reconstructed, bytes);

        let authority = chunked_authority("/stable-object-list", &bytes, expected.to_vec());
        prepared.reconcile_authority(&authority).unwrap();
    }

    #[test]
    fn projected_chunk_sequences_fail_closed_before_signing() {
        let path = "/projected-chunks";
        let bytes = stable_object_list_fixture();
        let initial_temp = tempfile::tempdir().unwrap();
        let prepared =
            PreparedPayloadObjectSet::prepare(&[payload(path, &bytes)], initial_temp.path())
                .unwrap();
        let chunks = prepared.chunks_for(path).unwrap().unwrap();
        let authority = chunked_authority(path, &bytes, chunks.clone());
        prepared.reconcile_authority(&authority).unwrap();

        let mut missing = chunks.clone();
        missing.remove(3);
        let mut extra = chunks.clone();
        extra.push(chunks[0].clone());
        let mut reordered = chunks.clone();
        reordered.swap(2, 3);
        let mut discontinuous = chunks.clone();
        discontinuous[0].size -= 1;
        discontinuous[1].size += 1;

        for drifted_chunks in [missing, extra, reordered, discontinuous] {
            let drifted = chunked_authority(path, &bytes, drifted_chunks);
            let temp = tempfile::tempdir().unwrap();
            assert!(
                PreparedPayloadObjectSet::prepare_for_authority(
                    &[payload(path, &bytes)],
                    &drifted,
                    temp.path(),
                )
                .is_err()
            );
        }

        let mut wrong_layout = authority;
        let PackageKindV3::Package(package) = &mut wrong_layout.kind else {
            unreachable!()
        };
        package.files[0].content_layout = FileContentLayoutV3::WholeObject;
        assert!(prepared.reconcile_authority(&wrong_layout).is_err());
    }

    #[test]
    fn exact_inventory_reconciliation_rejects_missing_extra_and_conflicting_size() {
        let first = "11".repeat(32);
        let second = "22".repeat(32);
        let extra = "33".repeat(32);
        let expected = BTreeMap::from([(first.clone(), 7), (second.clone(), 9)]);

        let missing = BTreeMap::from([(first.clone(), 7)]);
        let missing_error = reconcile_object_inventory(&expected, &missing)
            .unwrap_err()
            .to_string();
        assert!(missing_error.contains(&second));

        let with_extra =
            BTreeMap::from([(first.clone(), 7), (second.clone(), 9), (extra.clone(), 5)]);
        let extra_error = reconcile_object_inventory(&expected, &with_extra)
            .unwrap_err()
            .to_string();
        assert!(extra_error.contains(&extra));

        let conflicting = BTreeMap::from([(first.clone(), 8), (second, 9)]);
        let size_error = reconcile_object_inventory(&expected, &conflicting)
            .unwrap_err()
            .to_string();
        assert!(size_error.contains(&format!("{first}:7!=8")));
    }

    #[test]
    fn mixed_payloads_open_once_and_count_each_physical_hash_once() {
        let temp = tempfile::tempdir().unwrap();
        let small = vec![0x31; MIN_CHUNK_SIZE as usize - 1];
        let large = (0..MIN_CHUNK_SIZE as usize * 5)
            .map(|index| (index.wrapping_mul(37) % 251) as u8)
            .collect::<Vec<_>>();
        let payloads = vec![
            payload("/small-a", &small),
            payload("/small-b", &small),
            payload("/threshold", &large),
        ];

        let prepared = PreparedPayloadObjectSet::prepare(&payloads, temp.path()).unwrap();
        let metrics = prepared.metrics();
        let source_bytes = (small.len() * 2 + large.len()) as u64;

        assert_eq!(metrics.files_examined, 3);
        assert_eq!(metrics.source_files_opened, 3);
        assert_eq!(metrics.source_bytes_read, source_bytes);
        assert_eq!(metrics.source_files_reopened, 0);
        assert_eq!(metrics.source_bytes_reread, 0);
        assert_eq!(metrics.chunk_identity_bytes_hashed, large.len() as u64);
        assert_eq!(metrics.whole_content_bytes_hashed, source_bytes);
        assert_eq!(
            metrics.crypto_bytes_hashed,
            source_bytes + large.len() as u64
        );
        assert_eq!(
            metrics.staged_object_bytes_written + metrics.staged_object_deduplicated_bytes,
            source_bytes
        );
        assert!(metrics.staged_object_deduplications >= 1);
        assert_eq!(metrics.staged_object_canonical_bytes_reread, 0);
        assert_eq!(
            metrics.staged_unique_objects,
            prepared.inventory().len() as u64
        );
        assert!(prepared.chunks_for("/small-a").unwrap().is_none());
        assert!(prepared.chunks_for("/threshold").unwrap().is_some());
    }

    #[test]
    fn duplicate_whole_objects_are_each_authenticated_and_written_once() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"duplicate whole-object payload";
        let prepared = PreparedPayloadObjectSet::prepare(
            &[payload("/whole-a", bytes), payload("/whole-b", bytes)],
            temp.path(),
        )
        .unwrap();
        let metrics = prepared.metrics();

        assert_eq!(metrics.source_files_opened, 2);
        assert_eq!(metrics.source_bytes_read, bytes.len() as u64 * 2);
        assert_eq!(metrics.chunk_identity_bytes_hashed, 0);
        assert_eq!(metrics.whole_content_bytes_hashed, bytes.len() as u64 * 2);
        assert_eq!(metrics.staged_unique_objects, 1);
        assert_eq!(metrics.staged_object_deduplications, 1);
        assert_eq!(metrics.staged_object_bytes_written, bytes.len() as u64);
        assert_eq!(metrics.staged_object_deduplicated_bytes, bytes.len() as u64);
        assert_eq!(metrics.staged_object_canonical_bytes_reread, 0);
    }

    #[test]
    fn zero_threshold_minus_one_and_threshold_use_the_exact_layout_split() {
        let temp = tempfile::tempdir().unwrap();
        let empty = Vec::new();
        let below = vec![0x42; MIN_CHUNK_SIZE as usize - 1];
        let threshold = vec![0x43; MIN_CHUNK_SIZE as usize];
        let payloads = vec![
            payload("/empty-a", &empty),
            payload("/empty-b", &empty),
            payload("/below", &below),
            payload("/threshold", &threshold),
        ];

        let prepared = PreparedPayloadObjectSet::prepare(&payloads, temp.path()).unwrap();

        assert!(prepared.chunks_for("/empty-a").unwrap().is_none());
        assert!(prepared.chunks_for("/below").unwrap().is_none());
        assert!(prepared.chunks_for("/threshold").unwrap().is_some());
        assert_eq!(prepared.metrics().source_files_opened, 4);
        assert!(prepared.metrics().staged_object_deduplications >= 1);
        assert_eq!(prepared.metrics().staged_object_canonical_bytes_reread, 0);
    }

    #[test]
    fn repeated_large_chunks_are_hashed_once_per_occurrence_but_written_once() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = vec![0xa7; MAX_CHUNK_SIZE as usize * 3];
        let prepared =
            PreparedPayloadObjectSet::prepare(&[payload("/repeated", &bytes)], temp.path())
                .unwrap();
        let metrics = prepared.metrics();

        assert_eq!(metrics.source_files_opened, 1);
        assert_eq!(metrics.source_bytes_read, bytes.len() as u64);
        assert_eq!(metrics.chunks_derived, 3);
        assert_eq!(metrics.unique_chunks_derived, 1);
        assert_eq!(metrics.staged_unique_objects, 1);
        assert_eq!(metrics.staged_object_deduplications, 2);
        assert_eq!(
            metrics.staged_object_bytes_written,
            u64::from(MAX_CHUNK_SIZE)
        );
        assert_eq!(
            metrics.staged_object_deduplicated_bytes,
            u64::from(MAX_CHUNK_SIZE) * 2
        );
        assert_eq!(metrics.chunk_identity_bytes_hashed, bytes.len() as u64);
        assert_eq!(metrics.whole_content_bytes_hashed, bytes.len() as u64);
    }

    #[test]
    fn hardlink_non_owner_is_examined_but_never_opened_or_staged() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"rpm hardlink owner bytes";
        let mut owner = payload("/usr/bin/owner", bytes);
        owner.node.kind = crate::payload::PayloadNodeKind::Regular {
            hardlink_identity: Some("rpm:1:7".to_string()),
        };
        let mut alias_node = owner.node.clone();
        alias_node.kind = crate::payload::PayloadNodeKind::Hardlink {
            target: "/usr/bin/owner".to_string(),
            identity: "rpm:1:7".to_string(),
        };
        let alias =
            PackagePayloadFile::new("/usr/bin/alias".to_string(), alias_node, None, None).unwrap();

        let prepared = PreparedPayloadObjectSet::prepare(&[owner, alias], temp.path()).unwrap();
        let metrics = prepared.metrics();

        assert_eq!(metrics.files_examined, 2);
        assert_eq!(metrics.source_files_opened, 1);
        assert_eq!(metrics.source_bytes_read, bytes.len() as u64);
        assert_eq!(metrics.staged_unique_objects, 1);
        assert_eq!(prepared.inventory().len(), 1);
        assert!(prepared.chunks_for("/usr/bin/alias").unwrap().is_none());
    }

    #[test]
    fn changed_short_and_extra_sources_fail_before_returning_prepared_authority() {
        let expected = b"expected payload bytes";
        for source in [
            b"mutated!payload bytes".as_slice(),
            &expected[..expected.len() - 1],
            b"expected payload bytes!".as_slice(),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let result = PreparedPayloadObjectSet::prepare(
                &[payload_with_authority("/payload", source, expected)],
                temp.path(),
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn duplicate_paths_fail_before_opening_payload_sources() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"same";
        let result = PreparedPayloadObjectSet::prepare(
            &[payload("/duplicate", bytes), payload("/duplicate", bytes)],
            temp.path(),
        );
        let error = match result {
            Ok(_) => panic!("duplicate payload paths unexpectedly prepared"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("more than once"));
    }

    #[test]
    fn projected_authority_must_match_prepared_layout_and_inventory_exactly() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"hello world\n";
        let payloads = [PackagePayloadFile::new(
            "/usr/bin/hello".to_string(),
            PayloadNode::regular(0o755),
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(bytes),
                size: bytes.len() as u64,
            }),
            Some(ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(
                bytes.to_vec(),
            ))),
        )
        .unwrap()];
        let prepared = PreparedPayloadObjectSet::prepare(&payloads, temp.path()).unwrap();
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("prepared");
        prepared.reconcile_authority(&authority).unwrap();

        let mut drifted = authority.clone();
        let PackageKindV3::Package(package) = &mut drifted.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().sha256 = "0".repeat(64);
        let error = prepared
            .reconcile_authority(&drifted)
            .unwrap_err()
            .to_string();
        assert!(error.contains("disagrees with projected v3 authority"));
    }
}
