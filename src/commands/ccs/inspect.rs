// src/commands/ccs/inspect.rs

//! CCS package inspection and verification
//!
//! Commands for inspecting package contents and verifying signatures.

use anyhow::{Context, Result};
use conary::ccs::{inspector, verify, InspectedPackage, TrustPolicy};
use std::path::Path;

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
    let pkg = InspectedPackage::from_file(path)
        .context("Failed to read CCS package")?;

    // Output in requested format
    if format == "json" {
        inspector::print_json(&pkg, show_files, show_hooks, show_deps)?;
    } else {
        // Human-readable output
        inspector::print_summary(&pkg);

        if show_files {
            println!();
            inspector::print_files(&pkg);
        }

        if show_hooks {
            println!();
            inspector::print_hooks(&pkg);
        }

        if show_deps {
            println!();
            inspector::print_dependencies(&pkg);
        }
    }

    Ok(())
}

/// Verify a CCS package signature and contents
pub fn cmd_ccs_verify(
    package: &str,
    policy_path: Option<String>,
    allow_unsigned: bool,
) -> Result<()> {
    let path = Path::new(package);

    if !path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Verifying: {}", path.display());
    println!();

    // Load or create trust policy
    let policy = if let Some(policy_file) = policy_path {
        TrustPolicy::from_file(Path::new(&policy_file))
            .context("Failed to load trust policy")?
    } else if allow_unsigned {
        TrustPolicy::permissive()
    } else {
        // Default policy: allow unsigned but warn
        TrustPolicy {
            allow_unsigned: true,
            ..Default::default()
        }
    };

    // Run verification
    let result = verify::verify_package(path, &policy)
        .context("Verification failed")?;

    // Print results
    verify::print_result(&result);

    // Return error if verification failed
    if !result.valid {
        anyhow::bail!("Package verification failed");
    }

    Ok(())
}
