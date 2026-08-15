// conary-test/src/report/corpus.rs

//! Fail-closed JSON envelope for attributable just-works corpus evidence.

use conary_core::corpus::{
    CorpusAggregate, CorpusCaseResult, CorpusCoverageAggregate, CorpusSemantic, aggregate_cases,
    aggregate_coverage,
};
use serde::Serialize;

pub const CORPUS_REPORT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Serialize)]
pub struct CorpusReport<'a> {
    pub schema_version: u32,
    pub expected_cases: usize,
    pub aggregate: CorpusAggregate,
    pub coverage: CorpusCoverageAggregate,
    pub cases: &'a [CorpusCaseResult],
}

impl<'a> CorpusReport<'a> {
    pub fn from_cases(
        cases: &'a [CorpusCaseResult],
        expected_cases: usize,
        required: &[CorpusSemantic],
    ) -> Option<Self> {
        (expected_cases > 0 || !cases.is_empty() || !required.is_empty()).then(|| Self {
            schema_version: CORPUS_REPORT_SCHEMA_VERSION,
            expected_cases,
            aggregate: aggregate_cases(cases),
            coverage: aggregate_coverage(required, cases),
            cases,
        })
    }
}
