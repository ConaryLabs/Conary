// apps/conary/src/commands/ccs/export.rs

//! Export installed CCS packages to external container formats.

use anyhow::{Context, Result};
use std::path::Path;

/// Export CCS packages to container image format.
pub fn cmd_ccs_export(
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

    let report = export(export_format, packages, Path::new(output), &trust_policy)?;
    crate::ui::row(
        crate::ui::Status::Ok,
        &[&format!("Exported OCI image: {}", report.output.display())],
    );
    println!("  Packages: {}", report.package_names.join(", "));
    println!("  Layer size: {} bytes", report.layer_size);
    println!();
    println!("To load the image:");
    println!("  podman load < {}", report.output.display());
    println!("  # or");
    println!(
        "  skopeo copy oci-archive:{} containers-storage:localhost/{}:latest",
        report.output.display(),
        report
            .package_names
            .first()
            .map(String::as_str)
            .unwrap_or("image")
    );
    Ok(())
}
