// conary-core/src/ccs/verify/object_sink.rs

//! Storage authority for signature-first CCS object verification.

use super::VerifyError;
use crate::filesystem::{
    CasStore, VerifiedObjectBatch, VerifiedObjectBatchMetrics, VerifiedObjectSet,
};
use crate::packages::payload::{PayloadSpool, ReopenablePayload};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) enum ObjectDestination<'a> {
    Spool,
    Permanent(&'a CasStore),
}

enum ObjectBacking<'a> {
    Spool {
        spool: PayloadSpool,
        store: CasStore,
        sources: HashMap<String, ReopenablePayload>,
    },
    Permanent(VerifiedObjectBatch<'a>),
}

pub(super) struct VerifiedObjectSink<'a> {
    backing: ObjectBacking<'a>,
    seen: BTreeSet<String>,
}

pub(super) struct VerifiedObjectOutput {
    pub sources: HashMap<String, ReopenablePayload>,
    pub metrics: Option<VerifiedObjectBatchMetrics>,
}

impl<'a> VerifiedObjectSink<'a> {
    pub fn new(
        destination: ObjectDestination<'a>,
        expected: &BTreeMap<String, u64>,
    ) -> Result<Self> {
        let backing = match destination {
            ObjectDestination::Spool => {
                let required = expected
                    .values()
                    .try_fold(0_u64, |total, size| total.checked_add(*size));
                let required = required.context("CCS signed object-size arithmetic overflow")?;
                let spool = PayloadSpool::new(required)?;
                let store = CasStore::new(spool.root().join("objects"))?;
                ObjectBacking::Spool {
                    spool,
                    store,
                    sources: HashMap::new(),
                }
            }
            ObjectDestination::Permanent(cas) => ObjectBacking::Permanent(
                cas.verified_object_batch(
                    expected
                        .iter()
                        .map(|(sha256, size)| (sha256.clone(), *size)),
                )?,
            ),
        };
        Ok(Self {
            backing,
            seen: BTreeSet::new(),
        })
    }

    pub fn seen(&self) -> &BTreeSet<String> {
        &self.seen
    }

    pub fn ingest(&mut self, sha256: &str, size: u64, reader: &mut dyn Read) -> Result<()> {
        if !self.seen.insert(sha256.to_string()) {
            return Err(VerifyError::PackageError(format!(
                "CCS archive contains duplicate object {sha256}"
            ))
            .into());
        }
        let result = match &mut self.backing {
            ObjectBacking::Spool {
                spool,
                store,
                sources,
            } => store.store_reader_expected(reader, size, sha256).map(|_| {
                let path = store
                    .hash_to_path(sha256)
                    .expect("validated SHA-256 has a canonical CAS path");
                sources.insert(sha256.to_string(), spool.source(path));
            }),
            ObjectBacking::Permanent(batch) => batch.ingest(sha256, reader).map(|_| ()),
        };
        match result {
            Ok(()) => Ok(()),
            Err(crate::Error::ChecksumMismatch { expected, actual }) => {
                Err(VerifyError::PayloadInvalid(format!(
                    "CCS object path hash mismatch: expected {expected}, got {actual}"
                ))
                .into())
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn finish(self) -> Result<VerifiedObjectOutput> {
        match self.backing {
            ObjectBacking::Spool { sources, .. } => Ok(VerifiedObjectOutput {
                sources,
                metrics: None,
            }),
            ObjectBacking::Permanent(batch) => output_from_verified_set(batch.commit()?),
        }
    }
}

fn output_from_verified_set(set: VerifiedObjectSet) -> Result<VerifiedObjectOutput> {
    let metrics = set.metrics();
    let set = Arc::new(set);
    let sources = set
        .objects()
        .map(|(sha256, size)| {
            ReopenablePayload::from_verified_cas_object(Arc::clone(&set), sha256, size)
                .map(|source| (sha256.to_string(), source))
        })
        .collect::<crate::Result<HashMap<_, _>>>()?;
    Ok(VerifiedObjectOutput {
        sources,
        metrics: Some(metrics),
    })
}
