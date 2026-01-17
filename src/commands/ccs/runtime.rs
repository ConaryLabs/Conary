// src/commands/ccs/runtime.rs

//! CCS runtime commands
//!
//! Commands for ephemeral environments, running commands with packages,
//! and exporting to container formats.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Export CCS packages to container image format
pub fn cmd_ccs_export(
    packages: &[String],
    output: &str,
    format: &str,
    db_path: &str,
) -> Result<()> {
    use conary::ccs::export::{export, ExportFormat};

    let export_format = ExportFormat::parse(format)
        .ok_or_else(|| anyhow::anyhow!("Unknown export format: {}. Supported: oci", format))?;

    let output_path = Path::new(output);
    let db_path_opt = if Path::new(db_path).exists() {
        Some(Path::new(db_path))
    } else {
        None
    };

    export(export_format, packages, output_path, db_path_opt)
}

/// Spawn a shell with packages available in a temporary environment
pub fn cmd_ccs_shell(
    packages: &[String],
    db_path: &str,
    shell: Option<&str>,
    env_vars: &[String],
    keep: bool,
) -> Result<()> {
    println!("Creating ephemeral environment with packages: {}", packages.join(", "));

    let conn = conary::db::open(db_path)?;
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
    let cas = conary::filesystem::CasStore::new(&objects_dir)?;
    let mut deployed_count = 0;

    for pkg_name in packages {
        let troves = conary::db::models::Trove::find_by_name(&conn, pkg_name)?;
        if troves.is_empty() {
            anyhow::bail!("Package '{}' is not installed", pkg_name);
        }

        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let files = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?;

                for file in &files {
                    // Determine where to put the file in our temp environment
                    let rel_path = file.path.trim_start_matches('/');
                    let dest_path = temp_path.join(rel_path);

                    // Create parent directory
                    if let Some(parent) = dest_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    // Copy from CAS to temp dir
                    if let Ok(content) = cas.retrieve(&file.sha256_hash) {
                        std::fs::write(&dest_path, &content)?;

                        // Set executable bit if it's in bin or has executable perms
                        if file.path.contains("/bin/") || file.path.contains("/sbin/") {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let perms = std::fs::Permissions::from_mode(0o755);
                                let _ = std::fs::set_permissions(&dest_path, perms);
                            }
                        }
                        deployed_count += 1;
                    }
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
        format!("{}:{}:{}", lib_dir.display(), lib64_dir.display(), current_ld_path),
    );

    // Add custom environment variables
    for var in env_vars {
        if let Some((key, value)) = var.split_once('=') {
            env_map.insert(key.to_string(), value.to_string());
        }
    }

    // Mark as ephemeral environment
    env_map.insert("CONARY_EPHEMERAL".to_string(), "1".to_string());
    env_map.insert("CONARY_ENV_ROOT".to_string(), temp_path.display().to_string());

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
pub fn cmd_ccs_run(
    package: &str,
    command: &[String],
    db_path: &str,
    env_vars: &[String],
) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("No command specified. Usage: conary ccs run <package> -- <command> [args...]");
    }

    let conn = conary::db::open(db_path)?;
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
    let troves = conary::db::models::Trove::find_by_name(&conn, package)?;
    if troves.is_empty() {
        anyhow::bail!("Package '{}' is not installed", package);
    }

    let cas = conary::filesystem::CasStore::new(&objects_dir)?;

    for trove in &troves {
        if let Some(trove_id) = trove.id {
            let files = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?;

            for file in &files {
                let rel_path = file.path.trim_start_matches('/');
                let dest_path = temp_path.join(rel_path);

                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                if let Ok(content) = cas.retrieve(&file.sha256_hash) {
                    std::fs::write(&dest_path, &content)?;

                    if file.path.contains("/bin/") || file.path.contains("/sbin/") {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let perms = std::fs::Permissions::from_mode(0o755);
                            let _ = std::fs::set_permissions(&dest_path, perms);
                        }
                    }
                }
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
        format!("{}:{}:{}", lib_dir.display(), lib64_dir.display(), current_ld_path),
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
        std::process::exit(status.code().unwrap_or(1));
    }
}
