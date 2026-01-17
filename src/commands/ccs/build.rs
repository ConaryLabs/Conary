// src/commands/ccs/build.rs

//! CCS package building
//!
//! Commands for building CCS packages from manifests,
//! including generation of legacy format packages.

use anyhow::{Context, Result};
use conary::ccs::{builder, legacy, CcsBuilder, CcsManifest};
use std::path::Path;

/// Build a CCS package from a manifest
pub fn cmd_ccs_build(
    path: &str,
    output: &str,
    target: &str,
    source: Option<String>,
    no_classify: bool,
    chunked: bool,
    dry_run: bool,
) -> Result<()> {
    let path = Path::new(path);

    // Find the manifest
    let manifest_path = if path.is_file() && path.file_name().map(|n| n == "ccs.toml").unwrap_or(false) {
        path.to_path_buf()
    } else if path.is_dir() {
        path.join("ccs.toml")
    } else {
        anyhow::bail!("Cannot find ccs.toml at {}", path.display());
    };

    if !manifest_path.exists() {
        anyhow::bail!(
            "No ccs.toml found at {}. Run 'conary ccs-init' first.",
            manifest_path.display()
        );
    }

    // Parse the manifest
    let manifest = CcsManifest::from_file(&manifest_path)
        .context("Failed to parse ccs.toml")?;

    println!("Building {} v{}", manifest.package.name, manifest.package.version);

    // Determine source directory
    let source_dir = source
        .as_ref()
        .map(|s| Path::new(s).to_path_buf())
        .unwrap_or_else(|| manifest_path.parent().unwrap().to_path_buf());

    // Parse targets
    let targets: Vec<&str> = if target == "all" {
        vec!["ccs", "deb", "rpm", "arch"]
    } else {
        target.split(',').collect()
    };

    // Create output directory
    let output_dir = Path::new(output);
    if !dry_run {
        std::fs::create_dir_all(output_dir)
            .context("Failed to create output directory")?;
    }

    // Build the package data (needed for all targets)
    let build_result = if !dry_run {
        println!("Scanning source directory: {}", source_dir.display());

        let mut builder_instance = CcsBuilder::new(manifest.clone(), &source_dir);
        if no_classify {
            builder_instance = builder_instance.no_classify();
        }
        if chunked {
            builder_instance = builder_instance.with_chunking();
        } else {
            println!("CDC chunking disabled (use default for delta-efficient updates)");
        }

        let result = builder_instance.build()
            .context("Failed to build package")?;

        builder::print_build_summary(&result);
        Some(result)
    } else {
        None
    };

    if dry_run {
        println!();
        println!("[DRY RUN] Would build:");
    }

    for t in &targets {
        let filename = match *t {
            "ccs" => format!("{}-{}.ccs", manifest.package.name, manifest.package.version),
            "deb" => format!("{}_{}_amd64.deb", manifest.package.name, manifest.package.version),
            "rpm" => format!("{}-{}.x86_64.rpm", manifest.package.name, manifest.package.version),
            "arch" => format!("{}-{}-x86_64.pkg.tar.zst", manifest.package.name, manifest.package.version),
            _ => {
                println!("Unknown target format: {}", t);
                continue;
            }
        };

        let output_path = output_dir.join(&filename);

        if dry_run {
            println!("  {} -> {}", t, output_path.display());
        } else {
            let result = build_result.as_ref().unwrap();

            match *t {
                "ccs" => {
                    println!();
                    println!("Writing CCS package...");
                    builder::write_ccs_package(result, &output_path)
                        .context("Failed to write CCS package")?;
                    println!("  Created: {}", output_path.display());
                }
                "deb" => {
                    println!();
                    println!("Generating DEB package...");
                    let gen_result = legacy::deb::generate(result, &output_path)
                        .context("Failed to generate DEB package")?;
                    println!("  Created: {} ({} bytes)", output_path.display(), gen_result.size);
                    gen_result.loss_report.print_summary("DEB");
                }
                "rpm" => {
                    println!();
                    println!("Generating RPM package...");
                    let gen_result = legacy::rpm::generate(result, &output_path)
                        .context("Failed to generate RPM package")?;
                    println!("  Created: {} ({} bytes)", output_path.display(), gen_result.size);
                    gen_result.loss_report.print_summary("RPM");
                }
                "arch" => {
                    println!();
                    println!("Generating Arch package...");
                    let gen_result = legacy::arch::generate(result, &output_path)
                        .context("Failed to generate Arch package")?;
                    println!("  Created: {} ({} bytes)", output_path.display(), gen_result.size);
                    gen_result.loss_report.print_summary("Arch");
                }
                _ => {}
            }
        }
    }

    if !dry_run {
        println!();
        println!("Build complete!");
    }

    Ok(())
}
