// apps/conary/src/commands/packaging_mcp/records.rs
//! Packaging operation-record readers for MCP service methods.

use std::path::Path;

use anyhow::Result;
use conary_core::diagnostics::PackagingCommandOutput;

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct PackagingRecordSummary {
    pub operation_id: String,
    pub command: String,
    pub status: String,
    pub diagnostics: usize,
}

pub(crate) fn list_recent_records(dir: &Path, limit: usize) -> Result<Vec<PackagingRecordSummary>> {
    let mut paths = super::super::operation_records::list_packaging_records(dir)?;
    paths.reverse();
    paths
        .into_iter()
        .take(limit)
        .map(|path| {
            let record: PackagingCommandOutput =
                super::super::operation_records::load_json_record(&path)?;
            Ok(PackagingRecordSummary {
                operation_id: record.operation_id,
                command: record.command,
                status: format!("{:?}", record.status),
                diagnostics: record.diagnostics.len(),
            })
        })
        .collect()
}

pub(crate) fn read_record(
    dir: &Path,
    operation_id: &str,
) -> Result<Option<PackagingCommandOutput>> {
    super::super::operation_records::load_packaging_record_by_id(dir, operation_id)
}

pub(crate) fn latest_failed_record(dir: &Path) -> Result<Option<PackagingCommandOutput>> {
    super::super::operation_records::load_latest_failed_packaging_record(dir)
}
