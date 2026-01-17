// src/commands/ccs/init.rs

//! CCS package initialization
//!
//! Commands for initializing new CCS package manifests with
//! automatic detection of existing project metadata.

use anyhow::{Context, Result};
use conary::ccs::CcsManifest;
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
