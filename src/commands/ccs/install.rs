// src/commands/ccs/install.rs

//! CCS package installation
//!
//! Commands for installing CCS packages with signature verification,
//! dependency checking, and hook execution.

use anyhow::{Context, Result};
use conary::ccs::{verify, CcsPackage, HookExecutor, TrustPolicy};
use conary::packages::traits::PackageFormat;
use std::path::Path;

/// Install a CCS package
///
/// This is a minimal implementation that validates and extracts the package.
/// Full transaction support will be added in a future iteration.
#[allow(clippy::too_many_arguments)]
pub fn cmd_ccs_install(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    _components: Option<Vec<String>>,
    _sandbox: crate::commands::SandboxMode,
    no_deps: bool,
) -> Result<()> {
    let package_path = Path::new(package);

    if !package_path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Installing CCS package: {}", package_path.display());

    // Step 1: Verify signature (unless --allow-unsigned)
    if !allow_unsigned {
        let trust_policy = if let Some(policy_path) = &policy {
            TrustPolicy::from_file(Path::new(policy_path))
                .context("Failed to load trust policy")?
        } else {
            TrustPolicy::default()
        };

        let result = verify::verify_package(package_path, &trust_policy)?;
        if !result.valid {
            if trust_policy.allow_unsigned {
                println!("Warning: Package signature verification failed, but continuing (allow_unsigned policy)");
                for warning in &result.warnings {
                    println!("  - {}", warning);
                }
            } else {
                anyhow::bail!(
                    "Package signature verification failed. Use --allow-unsigned to install anyway.\n  Signature: {:?}\n  Content: {:?}",
                    result.signature_status,
                    result.content_status
                );
            }
        } else {
            println!("Signature verified: {:?}", result.signature_status);
        }
    } else {
        println!("Warning: Skipping signature verification (--allow-unsigned)");
    }

    // Step 2: Parse the package
    println!("Parsing package...");
    let ccs_pkg = CcsPackage::parse(package)?;

    println!(
        "Package: {} v{} ({} files)",
        ccs_pkg.name(),
        ccs_pkg.version(),
        ccs_pkg.files().len()
    );

    // Step 3: Check for existing installation
    let conn = conary::db::open(db_path).context("Failed to open package database")?;

    let existing = conary::db::models::Trove::find_by_name(&conn, ccs_pkg.name())?;
    if !existing.is_empty() {
        let old = &existing[0];
        if old.version == ccs_pkg.version() {
            anyhow::bail!(
                "Package {} version {} is already installed",
                ccs_pkg.name(),
                ccs_pkg.version()
            );
        }
        println!("Upgrading {} from {} to {}", ccs_pkg.name(), old.version, ccs_pkg.version());
    }

    // Step 4: Check dependencies
    if no_deps {
        println!("Skipping dependency check (--no-deps)");
    } else {
        println!("Checking dependencies...");
        for dep in ccs_pkg.dependencies() {
            let satisfied = conary::db::models::ProvideEntry::is_capability_satisfied(&conn, &dep.name)?;
            if !satisfied {
                let pkg_exists = conary::db::models::Trove::find_by_name(&conn, &dep.name)?;
                if pkg_exists.is_empty() {
                    if dry_run {
                        println!("  Missing dependency: {} (would fail)", dep.name);
                    } else {
                        anyhow::bail!(
                            "Missing dependency: {}{}",
                            dep.name,
                            dep.version
                                .as_ref()
                                .map(|v| format!(" {}", v))
                                .unwrap_or_default()
                        );
                    }
                }
            }
        }
        println!("Dependencies satisfied.");
    }

    if dry_run {
        println!();
        println!("[DRY RUN] Would install {} files:", ccs_pkg.files().len());
        for file in ccs_pkg.files().iter().take(10) {
            println!("  {}", file.path);
        }
        if ccs_pkg.files().len() > 10 {
            println!("  ... and {} more", ccs_pkg.files().len() - 10);
        }
        return Ok(());
    }

    // Step 5: Extract file contents
    println!("Extracting files...");
    let extracted_files = ccs_pkg.extract_file_contents()?;
    println!("Extracted {} files", extracted_files.len());

    // Step 6: Execute pre-hooks
    let mut hook_executor = HookExecutor::new(Path::new(root));
    let hooks = &ccs_pkg.manifest().hooks;

    if !hooks.users.is_empty() || !hooks.groups.is_empty() || !hooks.directories.is_empty() {
        println!("Executing pre-install hooks...");
        if let Err(e) = hook_executor.execute_pre_hooks(hooks) {
            anyhow::bail!("Pre-install hook failed: {}", e);
        }
    }

    // Step 7: Deploy files to filesystem
    println!("Deploying files to filesystem...");
    let root_path = std::path::Path::new(root);
    let mut files_deployed = 0;

    for file in &extracted_files {
        let dest_path = if file.path.starts_with('/') {
            root_path.join(file.path.trim_start_matches('/'))
        } else {
            root_path.join(&file.path)
        };

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        std::fs::write(&dest_path, &file.content)?;

        // Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(file.mode as u32))?;
        }

        files_deployed += 1;
    }

    println!("Deployed {} files to {}", files_deployed, root);

    // Step 8: Register in database
    println!("Updating database...");
    {
        let tx = conn.unchecked_transaction()?;

        // Remove old version if upgrading
        if !existing.is_empty() {
            let old = &existing[0];
            if let Some(old_id) = old.id {
                // Delete old files
                tx.execute("DELETE FROM files WHERE trove_id = ?1", [old_id])?;
                // Delete old provides
                tx.execute("DELETE FROM provides WHERE trove_id = ?1", [old_id])?;
                // Delete old trove
                tx.execute("DELETE FROM troves WHERE id = ?1", [old_id])?;
            }
        }

        // Create trove
        let mut trove = ccs_pkg.to_trove();
        let trove_id = trove.insert(&tx)?;

        // Register files
        for file in &extracted_files {
            let hash = file.sha256.clone().unwrap_or_default();
            let mut file_entry = conary::db::models::FileEntry::new(
                file.path.clone(),
                hash,
                file.size,
                file.mode,
                trove_id,
            );
            file_entry.insert(&tx)?;
        }

        // Create provides entry for the package itself
        let mut provide = conary::db::models::ProvideEntry::new(
            trove_id,
            ccs_pkg.name().to_string(),
            Some(ccs_pkg.version().to_string()),
        );
        provide.insert(&tx)?;

        // Register additional provides from manifest
        for cap in &ccs_pkg.manifest().provides.capabilities {
            if cap != ccs_pkg.name() {
                let mut cap_provide = conary::db::models::ProvideEntry::new(
                    trove_id,
                    cap.clone(),
                    None,
                );
                cap_provide.insert(&tx)?;
            }
        }

        tx.commit()?;
    }

    // Step 9: Execute post-hooks
    if !hooks.systemd.is_empty()
        || !hooks.tmpfiles.is_empty()
        || !hooks.sysctl.is_empty()
        || !hooks.alternatives.is_empty()
    {
        println!("Executing post-install hooks...");
        if let Err(e) = hook_executor.execute_post_hooks(hooks) {
            println!("Warning: Post-install hook failed: {}", e);
        }
    }

    println!();
    println!("Successfully installed {} v{}", ccs_pkg.name(), ccs_pkg.version());

    Ok(())
}
