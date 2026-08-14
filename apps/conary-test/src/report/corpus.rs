// conary-test/src/report/corpus.rs

//! Fail-closed JSON envelope for attributable just-works corpus evidence.

use conary_core::corpus::{CorpusAggregate, CorpusCaseResult, aggregate_cases};
use serde::Serialize;

pub const CORPUS_REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct CorpusReport<'a> {
    pub schema_version: u32,
    pub expected_cases: usize,
    pub aggregate: CorpusAggregate,
    pub cases: &'a [CorpusCaseResult],
}

impl<'a> CorpusReport<'a> {
    pub fn from_cases(cases: &'a [CorpusCaseResult], expected_cases: usize) -> Option<Self> {
        (expected_cases > 0 || !cases.is_empty()).then(|| Self {
            schema_version: CORPUS_REPORT_SCHEMA_VERSION,
            expected_cases,
            aggregate: aggregate_cases(cases),
            cases,
        })
    }
}
