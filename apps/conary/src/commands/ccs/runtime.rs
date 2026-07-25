// src/commands/ccs/runtime.rs

//! CCS runtime commands
//!
//! Commands for ephemeral environments, running commands with packages,
//! and exporting to container formats.

use super::super::open_db;
use super::payload_paths::sanitize_package_relative_path;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Export CCS packages to container image format
pub async fn cmd_ccs_export(
    packages: &[String],
    output: &str,
    format: &str,
    policy_path: &str,
) -> Result<()> {
    use conary_core::ccs::export::{ExportFormat, export};
    use conary_core::ccs::verify::TrustPolicy;

    let export_format = ExportFormat::parse(format)
        .ok_or_else(|| anyhow::anyhow!("Unknown export format: {}. Supported: oci", format))?;

    let output_path = Path::new(output);
    let trust_policy = TrustPolicy::from_file(Path::new(policy_path))
        .with_context(|| format!("Failed to load CCS trust policy: {policy_path}"))?;

    export(export_format, packages, output_path, &trust_policy)
}

/// Spawn a shell with packages available in a temporary environment
pub async fn cmd_ccs_shell(
    packages: &[String],
    db_path: &str,
    shell: Option<&str>,
    env_vars: &[String],
    keep: bool,
) -> Result<()> {
    println!(
        "Creating ephemeral environment with packages: {}",
        packages.join(", ")
    );

    let conn = open_db(db_path)?;
    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");

    // Create temporary directory for the environment
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;
    let temp_path = temp_dir.path();

    // Create directory structure
    let bin_dir = temp_path.join("bin");
    let lib_dir = temp_path.join("lib");
    let lib64_dir = temp_path.join("lib64");
    std::fs::create_dir_all(&bin_dir)?;
    std::fs::create_dir_all(&lib_dir)?;
    std::fs::create_dir_all(&lib64_dir)?;

    // Deploy files from each package to the temp environment
    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;
    let mut deployed_count = 0;

    for pkg_name in packages {
        let troves = conary_core::db::models::Trove::find_by_name(&conn, pkg_name)?;
        if troves.is_empty() {
            anyhow::bail!("Package '{}' is not installed", pkg_name);
        }

        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let files = conary_core::db::models::FileEntry::find_by_trove(&conn, trove_id)?;

                for file in &files {
                    // Sanitize to prevent path traversal (e.g. ../../etc/shadow)
                    let rel_path = sanitize_package_relative_path(&file.path)?;
                    let dest_path = temp_path.join(&rel_path);

                    // Create parent directory
                    if let Some(parent) = dest_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    deploy_runtime_file(&temp_path, &dest_path, &cas, file)?;
                    deployed_count += 1;
                }
            }
        }
    }

    println!("Deployed {} files to temporary environment", deployed_count);

    // Build environment variables
    let mut env_map: HashMap<String, String> = std::env::vars().collect();

    // Prepend our paths
    let current_path = env_map.get("PATH").cloned().unwrap_or_default();
    env_map.insert(
        "PATH".to_string(),
        format!("{}:{}", bin_dir.display(), current_path),
    );

    let current_ld_path = env_map.get("LD_LIBRARY_PATH").cloned().unwrap_or_default();
    env_map.insert(
        "LD_LIBRARY_PATH".to_string(),
        format!(
            "{}:{}:{}",
            lib_dir.display(),
            lib64_dir.display(),
            current_ld_path
        ),
    );

    // Add custom environment variables
    for var in env_vars {
        if let Some((key, value)) = var.split_once('=') {
            env_map.insert(key.to_string(), value.to_string());
        }
    }

    // Mark as ephemeral environment
    env_map.insert("CONARY_EPHEMERAL".to_string(), "1".to_string());
    env_map.insert(
        "CONARY_ENV_ROOT".to_string(),
        temp_path.display().to_string(),
    );

    // Determine which shell to use
    let shell_cmd = shell
        .map(String::from)
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_string());

    println!("\nEntering ephemeral shell ({})", shell_cmd);
    println!("Environment root: {}", temp_path.display());
    println!("Type 'exit' to leave the ephemeral environment.\n");

    // Spawn the shell
    let status = Command::new(&shell_cmd)
        .envs(&env_map)
        .status()
        .context("Failed to spawn shell")?;

    // Clean up (unless --keep was specified)
    if keep {
        let kept_path = temp_dir.keep();
        println!("\nKept temporary environment at: {}", kept_path.display());
    } else {
        println!("\nCleaning up ephemeral environment...");
        // TempDir drops automatically
    }

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Shell exited with status: {}", status)
    }
}

/// Run a command with a package available temporarily
pub async fn cmd_ccs_run(
    package: &str,
    command: &[String],
    db_path: &str,
    env_vars: &[String],
) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!(
            "No command specified. Usage: conary ccs run <package> -- <command> [args...]"
        );
    }

    let conn = open_db(db_path)?;
    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");

    // Create temporary directory
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;
    let temp_path = temp_dir.path();

    // Create directory structure
    let bin_dir = temp_path.join("bin");
    let lib_dir = temp_path.join("lib");
    let lib64_dir = temp_path.join("lib64");
    std::fs::create_dir_all(&bin_dir)?;
    std::fs::create_dir_all(&lib_dir)?;
    std::fs::create_dir_all(&lib64_dir)?;

    // Find and deploy the package
    let troves = conary_core::db::models::Trove::find_by_name(&conn, package)?;
    if troves.is_empty() {
        anyhow::bail!("Package '{}' is not installed", package);
    }

    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;

    for trove in &troves {
        if let Some(trove_id) = trove.id {
            let files = conary_core::db::models::FileEntry::find_by_trove(&conn, trove_id)?;

            for file in &files {
                // Sanitize to prevent path traversal (e.g. ../../etc/shadow)
                let rel_path = sanitize_package_relative_path(&file.path)?;
                let dest_path = temp_path.join(&rel_path);

                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                deploy_runtime_file(&temp_path, &dest_path, &cas, file)?;
            }
        }
    }

    // Build environment
    let mut env_map: HashMap<String, String> = std::env::vars().collect();

    let current_path = env_map.get("PATH").cloned().unwrap_or_default();
    env_map.insert(
        "PATH".to_string(),
        format!("{}:{}", bin_dir.display(), current_path),
    );

    let current_ld_path = env_map.get("LD_LIBRARY_PATH").cloned().unwrap_or_default();
    env_map.insert(
        "LD_LIBRARY_PATH".to_string(),
        format!(
            "{}:{}:{}",
            lib_dir.display(),
            lib64_dir.display(),
            current_ld_path
        ),
    );

    for var in env_vars {
        if let Some((key, value)) = var.split_once('=') {
            env_map.insert(key.to_string(), value.to_string());
        }
    }

    // Run the command
    let cmd_name = &command[0];
    let cmd_args = &command[1..];

    let status = Command::new(cmd_name)
        .args(cmd_args)
        .envs(&env_map)
        .status()
        .with_context(|| format!("Failed to execute: {}", cmd_name))?;

    // TempDir cleans up automatically

    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "command exited with status {}",
            status.code().unwrap_or(1)
        ))
    }
}

fn deploy_runtime_file(
    runtime_root: &std::path::Path,
    destination: &std::path::Path,
    cas: &conary_core::filesystem::CasStore,
    file: &conary_core::db::models::FileEntry,
) -> Result<()> {
    use conary_core::payload::PayloadNodeKind;

    match &file.node.source.kind {
        PayloadNodeKind::Regular { .. } => {
            let content = file.content.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Regular runtime payload {} has no content authority",
                    file.path
                )
            })?;
            let bytes = cas.retrieve(&content.sha256).with_context(|| {
                format!(
                    "Missing CAS object for '{}' (hash: {})",
                    file.path, content.sha256
                )
            })?;
            if bytes.len() as u64 != content.size {
                anyhow::bail!(
                    "CAS object size mismatch for '{}': expected {}, got {}",
                    file.path,
                    content.size,
                    bytes.len()
                );
            }
            std::fs::write(destination, bytes)?;
        }
        PayloadNodeKind::Directory => {
            std::fs::create_dir(destination)?;
        }
        PayloadNodeKind::Symlink { target } => {
            std::os::unix::fs::symlink(target, destination)
                .with_context(|| format!("Failed to create symlink {}", file.path))?;
        }
        PayloadNodeKind::Hardlink { target, .. } => {
            let relative = sanitize_package_relative_path(target)?;
            let target = runtime_root.join(relative);
            std::fs::hard_link(&target, destination).with_context(|| {
                format!(
                    "Failed to create runtime hardlink {} to {}",
                    destination.display(),
                    target.display()
                )
            })?;
        }
        PayloadNodeKind::BlockDevice { .. }
        | PayloadNodeKind::CharacterDevice { .. }
        | PayloadNodeKind::Fifo
        | PayloadNodeKind::Socket => {
            anyhow::bail!(
                "Temporary CCS runtime does not materialize special payload node {}",
                file.path
            );
        }
    }
    conary_core::generation::root_manifest::apply_resolved_payload_metadata(
        destination,
        &file.node,
    )?;
    Ok(())
}
