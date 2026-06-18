// src/commands/ccs/build.rs

//! CCS package building
//!
//! Commands for building CCS packages from manifests,
//! including generation of legacy format packages.

use crate::cli::CcsBuildFormat;
use anyhow::{Context, Result};
use conary_core::ccs::{CcsBuilder, CcsManifest, builder, legacy};
use std::path::Path;

/// Build a CCS package from a manifest
pub async fn cmd_ccs_build(
    path: &str,
    output: &str,
    target: &str,
    source: Option<String>,
    no_classify: bool,
    chunked: bool,
    dry_run: bool,
    format: CcsBuildFormat,
    local_dev: bool,
    key: Option<String>,
) -> Result<()> {
    let path = Path::new(path);

    if local_dev && key.is_some() {
        anyhow::bail!("--local-dev and --key are mutually exclusive signing options");
    }
    if format == CcsBuildFormat::V1 && (key.is_some() || local_dev) {
        anyhow::bail!("--key and --local-dev are only supported when building with --format v2");
    }
    if format == CcsBuildFormat::V2 && target != "ccs" {
        anyhow::bail!("--format v2 only supports --target ccs in M4b");
    }
    if format == CcsBuildFormat::V2 && key.is_none() && !local_dev {
        anyhow::bail!("ccs build --format v2 requires --key <private-key> or --local-dev");
    }

    // Find the manifest
    let manifest_path =
        if path.is_file() && path.file_name().map(|n| n == "ccs.toml").unwrap_or(false) {
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
    println!("Parsing manifest...");
    let manifest = CcsManifest::from_file(&manifest_path).context("Failed to parse ccs.toml")?;

    if format == CcsBuildFormat::V2 {
        let findings = conary_core::ccs::v2::authoring::lint_manifest_for_v2_authoring(&manifest);
        if findings.iter().any(|finding| finding.blocks_build) {
            for finding in &findings {
                if finding.blocks_build {
                    eprintln!("{}: {}", finding.code, finding.message);
                    eprintln!("  fix: {}", finding.suggestion);
                }
            }
            anyhow::bail!("ccs build --format v2 blocked by M4b authoring lint");
        }
    }

    println!(
        "Building {} v{}",
        manifest.package.name, manifest.package.version
    );

    // Determine source directory
    let source_dir = match source.as_ref() {
        Some(s) => Path::new(s).to_path_buf(),
        None => manifest_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("manifest path has no parent directory"))?
            .to_path_buf(),
    };

    // Parse and validate targets
    const VALID_TARGETS: &[&str] = &["ccs", "deb", "rpm", "arch"];
    let targets: Vec<&str> = if target == "all" {
        VALID_TARGETS.to_vec()
    } else {
        let parsed: Vec<&str> = target.split(',').collect();
        let invalid: Vec<&&str> = parsed
            .iter()
            .filter(|t| !VALID_TARGETS.contains(t))
            .collect();
        if !invalid.is_empty() {
            anyhow::bail!(
                "Invalid target format(s): {}. Valid targets: ccs, deb, rpm, arch, all",
                invalid
                    .iter()
                    .map(|t| format!("'{t}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        parsed
    };

    // Create output directory
    let output_dir = Path::new(output);
    if !dry_run {
        std::fs::create_dir_all(output_dir).context("Failed to create output directory")?;
    }

    // Build the package data (needed for all targets)
    let build_result = if !dry_run {
        println!("Scanning source directory: {}", source_dir.display());

        let file_count = walkdir::WalkDir::new(&source_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .count();
        println!("Scanning {} files...", file_count);

        let mut builder_instance = CcsBuilder::new(manifest.clone(), &source_dir);
        if no_classify {
            builder_instance = builder_instance.no_classify();
        }
        if chunked {
            builder_instance = builder_instance.with_chunking();
        } else {
            println!("CDC chunking disabled (use default for delta-efficient updates)");
        }

        println!("Compressing...");
        let result = builder_instance
            .build()
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
            "ccs" if format == CcsBuildFormat::V2 => {
                let release = manifest
                    .package
                    .release
                    .as_deref()
                    .context("v2 output naming requires package.release")?;
                format!(
                    "{}-{}-{}.ccs",
                    manifest.package.name, manifest.package.version, release
                )
            }
            "ccs" => format!("{}-{}.ccs", manifest.package.name, manifest.package.version),
            "deb" => format!(
                "{}_{}_amd64.deb",
                manifest.package.name, manifest.package.version
            ),
            "rpm" => format!(
                "{}-{}.x86_64.rpm",
                manifest.package.name, manifest.package.version
            ),
            "arch" => format!(
                "{}-{}-x86_64.pkg.tar.zst",
                manifest.package.name, manifest.package.version
            ),
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
                    if format == CcsBuildFormat::V2 {
                        println!("Writing CCS v2 package...");
                        let debug_toml = manifest.to_toml().context("serialize debug ccs.toml")?;
                        let projected = conary_core::ccs::v2::project_build_result_to_v2(
                            conary_core::ccs::v2::V2AuthoringInput {
                                build: result,
                                local_dev,
                                debug_toml: Some(debug_toml),
                            },
                        )
                        .context("project v2 package authority")?;
                        let signing_key = if local_dev {
                            super::local_dev::load_or_create_local_dev_key()?
                        } else {
                            let key_path = key
                                .as_deref()
                                .context("missing --key for v2 release signing")?;
                            conary_core::ccs::signing::SigningKeyPair::load_from_file(Path::new(
                                key_path,
                            ))
                            .map_err(anyhow::Error::from)?
                        };
                        builder::write_v2_ccs_package(
                            &projected.authority,
                            &projected.payloads_by_path,
                            &output_path,
                            &signing_key,
                            projected.debug_toml.as_deref(),
                            None,
                            None,
                        )
                        .context("Failed to write CCS v2 package")?;
                        if local_dev {
                            println!(
                                "  Signed with local-dev CCS key; release publish will reject this artifact."
                            );
                        }
                    } else {
                        println!("Writing CCS package...");
                        builder::write_ccs_package(result, &output_path)
                            .context("Failed to write CCS package")?;
                    }
                    println!("  Created: {}", output_path.display());
                }
                "deb" => {
                    println!();
                    println!("Generating DEB package...");
                    let gen_result = legacy::deb::generate(result, &output_path)
                        .context("Failed to generate DEB package")?;
                    println!(
                        "  Created: {} ({} bytes)",
                        output_path.display(),
                        gen_result.size
                    );
                    gen_result.loss_report.print_summary("DEB");
                }
                "rpm" => {
                    println!();
                    println!("Generating RPM package...");
                    let gen_result = legacy::rpm::generate(result, &output_path)
                        .context("Failed to generate RPM package")?;
                    println!(
                        "  Created: {} ({} bytes)",
                        output_path.display(),
                        gen_result.size
                    );
                    gen_result.loss_report.print_summary("RPM");
                }
                "arch" => {
                    println!();
                    println!("Generating Arch package...");
                    let gen_result = legacy::arch::generate(result, &output_path)
                        .context("Failed to generate Arch package")?;
                    println!(
                        "  Created: {} ({} bytes)",
                        output_path.display(),
                        gen_result.size
                    );
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
