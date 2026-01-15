// src/commands/ccs.rs
//! CCS package format commands
//!
//! Commands for creating, building, and inspecting CCS packages.

use anyhow::{Context, Result};
use conary::ccs::{builder, inspector, legacy, verify, CcsBuilder, CcsManifest, InspectedPackage, TrustPolicy};
use std::path::Path;

/// Initialize a new CCS manifest in the given directory
pub fn cmd_ccs_init(
    path: &str,
    name: Option<String>,
    version: &str,
    force: bool,
) -> Result<()> {
    let dir = Path::new(path);
    let manifest_path = dir.join("ccs.toml");

    // Check if manifest already exists
    if manifest_path.exists() && !force {
        anyhow::bail!(
            "ccs.toml already exists at {}. Use --force to overwrite.",
            manifest_path.display()
        );
    }

    // Determine package name
    let pkg_name = name.unwrap_or_else(|| {
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-package")
            .to_string()
    });

    // Try to detect existing project metadata
    let manifest = detect_project_and_create_manifest(dir, &pkg_name, version)?;

    // Write the manifest
    let toml = manifest.to_toml().context("Failed to serialize manifest")?;
    std::fs::write(&manifest_path, toml).context("Failed to write ccs.toml")?;

    println!("Created {}", manifest_path.display());
    println!();
    println!("Package: {} v{}", manifest.package.name, manifest.package.version);
    println!();
    println!("Next steps:");
    println!("  1. Edit ccs.toml to add dependencies and hooks");
    println!("  2. Run 'conary ccs-build' to create the package");

    Ok(())
}

/// Detect existing project files and create an appropriate manifest
fn detect_project_and_create_manifest(
    dir: &Path,
    name: &str,
    version: &str,
) -> Result<CcsManifest> {
    let mut manifest = CcsManifest::new_minimal(name, version);

    // Check for Cargo.toml (Rust project)
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.exists() {
        if let Ok(content) = std::fs::read_to_string(&cargo_toml)
            && let Ok(cargo) = content.parse::<toml::Table>()
            && let Some(package) = cargo.get("package").and_then(|p| p.as_table())
        {
            if let Some(n) = package.get("name").and_then(|v| v.as_str()) {
                manifest.package.name = n.to_string();
            }
            if let Some(v) = package.get("version").and_then(|v| v.as_str()) {
                manifest.package.version = v.to_string();
            }
            if let Some(d) = package.get("description").and_then(|v| v.as_str()) {
                manifest.package.description = d.to_string();
            }
            if let Some(l) = package.get("license").and_then(|v| v.as_str()) {
                manifest.package.license = Some(l.to_string());
            }
            if let Some(h) = package.get("homepage").and_then(|v| v.as_str()) {
                manifest.package.homepage = Some(h.to_string());
            }
            if let Some(r) = package.get("repository").and_then(|v| v.as_str()) {
                manifest.package.repository = Some(r.to_string());
            }
        }
        println!("Detected Rust project (Cargo.toml)");
    }

    // Check for package.json (Node.js project)
    let package_json = dir.join("package.json");
    if package_json.exists() {
        if let Ok(content) = std::fs::read_to_string(&package_json)
            && let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content)
        {
            if let Some(n) = pkg.get("name").and_then(|v| v.as_str()) {
                manifest.package.name = n.to_string();
            }
            if let Some(v) = pkg.get("version").and_then(|v| v.as_str()) {
                manifest.package.version = v.to_string();
            }
            if let Some(d) = pkg.get("description").and_then(|v| v.as_str()) {
                manifest.package.description = d.to_string();
            }
            if let Some(l) = pkg.get("license").and_then(|v| v.as_str()) {
                manifest.package.license = Some(l.to_string());
            }
        }
        println!("Detected Node.js project (package.json)");
    }

    // Check for pyproject.toml (Python project)
    let pyproject = dir.join("pyproject.toml");
    if pyproject.exists() {
        if let Ok(content) = std::fs::read_to_string(&pyproject)
            && let Ok(py) = content.parse::<toml::Table>()
            && let Some(project) = py.get("project").and_then(|p| p.as_table())
        {
            if let Some(n) = project.get("name").and_then(|v| v.as_str()) {
                manifest.package.name = n.to_string();
            }
            if let Some(v) = project.get("version").and_then(|v| v.as_str()) {
                manifest.package.version = v.to_string();
            }
            if let Some(d) = project.get("description").and_then(|v| v.as_str()) {
                manifest.package.description = d.to_string();
            }
        }
        println!("Detected Python project (pyproject.toml)");
    }

    Ok(manifest)
}

/// Build a CCS package from a manifest
pub fn cmd_ccs_build(
    path: &str,
    output: &str,
    target: &str,
    source: Option<String>,
    no_classify: bool,
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
