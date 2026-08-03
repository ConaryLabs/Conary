// apps/conary/src/commands/ccs/init.rs

//! CCS package initialization
//!
//! Commands for initializing new CCS package manifests with
//! automatic detection of existing project metadata.

use anyhow::{Context, Result};
use conary_core::ccs::CcsManifest;
use std::path::Path;

/// Initialize a new CCS manifest in the given directory
pub async fn cmd_ccs_init(
    path: &str,
    name: Option<String>,
    version: &str,
    force: bool,
    template: Option<super::CcsInitTemplate>,
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

    let manifest = super::templates::build_manifest(template, &pkg_name, version)?;
    let manifest = detect_project_and_update_manifest(dir, manifest)?;

    // Write the manifest
    let toml = manifest.to_toml().context("Failed to serialize manifest")?;
    std::fs::write(&manifest_path, toml).context("Failed to write ccs.toml")?;
    write_template_files(dir, template)?;

    println!("Created {}", manifest_path.display());
    println!();
    println!(
        "Package: {} v{}",
        manifest.package.name, manifest.package.version
    );
    println!();
    println!("Next steps:");
    println!("  1. Edit ccs.toml to add dependencies and hooks");
    println!("  2. Run 'conary ccs build' to create the package");

    Ok(())
}

fn write_template_files(dir: &Path, template: Option<super::CcsInitTemplate>) -> Result<()> {
    match template {
        Some(super::CcsInitTemplate::ConfigNoreplace) => {
            write_file(
                dir.join("etc/conary-example/config.toml"),
                b"message = \"hello from conary\"\n",
            )?;
        }
        Some(super::CcsInitTemplate::Service) => {
            write_file(
                dir.join("usr/bin/conary-example"),
                b"#!/bin/sh\necho conary-example\n",
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    dir.join("usr/bin/conary-example"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .context("Failed to chmod service example binary")?;
            }
            write_file(
                dir.join("usr/lib/systemd/system/conary-example.service"),
                b"[Unit]\nDescription=Conary example service\n\n[Service]\nType=oneshot\nExecStart=/usr/bin/conary-example\n\n[Install]\nWantedBy=multi-user.target\n",
            )?;
        }
        Some(super::CcsInitTemplate::MinimalFile) | None => {}
    }
    Ok(())
}

fn write_file(path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("Failed to write {}", path.display()))
}

/// Detect existing project files and overlay discovered metadata onto a manifest.
fn detect_project_and_update_manifest(
    dir: &Path,
    mut manifest: CcsManifest,
) -> Result<CcsManifest> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::v3::PackageKindTagV3;

    #[test]
    fn project_detection_preserves_template_identity_fields() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[package]
name = "cargo-name"
version = "0.2.0"
description = "cargo description"
license = "MIT"
"#,
        )
        .unwrap();
        let base = super::super::templates::build_manifest(
            Some(super::super::CcsInitTemplate::MinimalFile),
            "hello",
            "0.1.0",
        )
        .unwrap();

        let manifest = detect_project_and_update_manifest(temp.path(), base).unwrap();

        assert_eq!(manifest.package.name, "cargo-name");
        assert_eq!(manifest.package.version, "0.2.0");
        assert_eq!(manifest.package.description, "cargo description");
        assert_eq!(manifest.package.license.as_deref(), Some("MIT"));
        assert_eq!(manifest.package.release.as_str(), "1");
        assert_eq!(manifest.package.kind, PackageKindTagV3::Package);
        assert_eq!(
            manifest
                .package
                .platform
                .as_ref()
                .and_then(|platform| platform.arch.as_deref()),
            Some(conary_core::ccs::manifest::DEFAULT_CONARY_ARCHITECTURE)
        );
    }

    #[tokio::test]
    async fn default_init_writes_explicit_noarch_authority() {
        let temp = tempfile::tempdir().unwrap();

        cmd_ccs_init(
            temp.path().to_str().unwrap(),
            Some("demo".to_string()),
            "0.1.0",
            false,
            None,
        )
        .await
        .unwrap();

        let content = std::fs::read_to_string(temp.path().join("ccs.toml")).unwrap();
        assert!(content.contains("[package.platform]"));
        assert!(content.contains("arch = \"noarch\""));
        let manifest = CcsManifest::parse(&content).unwrap();
        assert_eq!(
            manifest
                .package
                .platform
                .as_ref()
                .and_then(|platform| platform.arch.as_deref()),
            Some(conary_core::ccs::manifest::DEFAULT_CONARY_ARCHITECTURE)
        );
    }

    #[tokio::test]
    async fn config_noreplace_template_writes_example_config_file() {
        let temp = tempfile::tempdir().unwrap();

        cmd_ccs_init(
            temp.path().to_str().unwrap(),
            Some("demo".to_string()),
            "0.1.0",
            false,
            Some(super::super::CcsInitTemplate::ConfigNoreplace),
        )
        .await
        .unwrap();

        assert!(temp.path().join("etc/conary-example/config.toml").exists());
    }

    #[tokio::test]
    async fn service_template_writes_example_binary_and_unit() {
        let temp = tempfile::tempdir().unwrap();

        cmd_ccs_init(
            temp.path().to_str().unwrap(),
            Some("demo".to_string()),
            "0.1.0",
            false,
            Some(super::super::CcsInitTemplate::Service),
        )
        .await
        .unwrap();

        assert!(temp.path().join("usr/bin/conary-example").exists());
        assert!(
            temp.path()
                .join("usr/lib/systemd/system/conary-example.service")
                .exists()
        );
    }
}
