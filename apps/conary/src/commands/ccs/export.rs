// apps/conary/src/commands/ccs/export.rs

//! Export installed CCS packages to external container formats.

use anyhow::{Context, Result};
use std::path::Path;

/// Export CCS packages to container image format.
pub async fn cmd_ccs_export(
    packages: &[String],
    output: &str,
    format: &str,
    policy_path: &str,
) -> Result<()> {
    use conary_core::ccs::export::{ExportFormat, export};
    use conary_core::ccs::verify::TrustPolicy;

    let export_format = ExportFormat::parse(format)
        .ok_or_else(|| anyhow::anyhow!("Unknown export format: {format}. Supported: oci"))?;
    let trust_policy = TrustPolicy::from_file(Path::new(policy_path))
        .with_context(|| format!("Failed to load CCS trust policy: {policy_path}"))?;

    export(export_format, packages, Path::new(output), &trust_policy)
}
