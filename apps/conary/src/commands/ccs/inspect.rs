// apps/conary/src/commands/ccs/inspect.rs

//! CCS package inspection and verification
//!
//! Commands for inspecting package contents and verifying signatures.

use anyhow::{Context, Result};
use conary_core::ccs::{TrustPolicy, UntrustedPackageInspection, verify};
use std::path::Path;

mod render;

/// Inspect a CCS package
pub fn cmd_ccs_inspect(
    package: &str,
    show_files: bool,
    show_hooks: bool,
    show_deps: bool,
    format: &str,
) -> Result<()> {
    let path = Path::new(package);

    if !path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    // Load and parse the package
    let pkg = UntrustedPackageInspection::inspect_untrusted_file(path)
        .context("Failed to inspect untrusted CCS package")?;

    // Output in requested format
    if format == "json" {
        render::print_json(&pkg, show_files, show_hooks, show_deps)?;
    } else {
        // Human-readable output
        render::print_summary(&pkg);

        if show_files {
            println!();
            render::print_files(&pkg);
        }

        if show_hooks {
            println!();
            render::print_hooks(&pkg);
        }

        if show_deps {
            println!();
            render::print_dependencies(&pkg);
        }
    }

    Ok(())
}

/// Verify a CCS package signature and contents
pub fn cmd_ccs_verify(package: &str, policy_path: Option<String>) -> Result<()> {
    let path = Path::new(package);

    if !path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Verifying: {}", path.display());
    println!();

    // Load or create trust policy
    let policy = if let Some(policy_file) = policy_path {
        TrustPolicy::from_file(Path::new(&policy_file)).context("Failed to load trust policy")?
    } else if let Some(local_policy) = super::local_dev::local_dev_trust_policy()? {
        println!("Using local-dev CCS trust policy for verification.");
        local_policy
    } else {
        anyhow::bail!("CCS verification requires --policy or an initialized local-dev signing key");
    };

    // Run verification
    let result = verify::verify_package(path, &policy).context("Verification failed")?;

    // Print results
    crate::ui::row(
        crate::ui::Status::Ok,
        &[&format!(
            "{} v{}",
            result.package_name(),
            result.package_version()
        )],
    );
    let signature = result
        .signature()
        .key_id
        .as_deref()
        .map(|id| format!("Signature: valid key={id}"))
        .unwrap_or_else(|| "Signature: valid".to_string());
    crate::ui::row(crate::ui::Status::Ok, &[&signature]);
    crate::ui::row(
        crate::ui::Status::Ok,
        &[&format!(
            "Content: {} files verified",
            result.files_checked()
        )],
    );

    Ok(())
}
