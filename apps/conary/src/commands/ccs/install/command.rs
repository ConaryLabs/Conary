// src/commands/ccs/install/command.rs

use anyhow::{Context, Result};
use conary_core::ccs::{CcsPackage, TrustPolicy, verify};
use conary_core::packages::traits::PackageFormat;
use std::path::Path;

use super::super::payload_paths::validate_ccs_payload_paths;
use super::capability_declaration::validate_ccs_capability_declaration;
use super::component_selection::select_ccs_components;
use super::dependency::validate_incoming_version_against_dependents;
use crate::commands::install::{CcsTransactionInstallOptions, install_ccs_package_transactionally};
use crate::commands::open_db;

/// Install a CCS package
#[allow(clippy::too_many_arguments)]
pub async fn cmd_ccs_install(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    policy: Option<String>,
    components: Option<Vec<String>>,
    sandbox: crate::commands::SandboxMode,
    no_deps: bool,
    reinstall: bool,
) -> Result<()> {
    let package_path = Path::new(package);

    if !package_path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Installing CCS package: {}", package_path.display());

    let trust_policy = if let Some(policy_path) = &policy {
        TrustPolicy::from_file(Path::new(policy_path)).context("Failed to load trust policy")?
    } else if let Some(local_policy) = super::super::local_dev::local_dev_trust_policy()? {
        println!("Using local-dev CCS trust policy.");
        local_policy
    } else {
        anyhow::bail!(
            "CCS install requires --policy or an initialized local-dev signing key; unsigned and untrusted packages cannot be installed"
        );
    };
    let verification = verify::verify_package(package_path, &trust_policy)
        .context("CCS package authority verification failed")?;
    println!(
        "Verified signed CCS v3 authority ({} payload files)",
        verification.files_checked()
    );

    // Step 2: Parse the package
    println!("Parsing package...");
    let ccs_pkg = CcsPackage::from_verified_archive(package, &verification)?;

    println!(
        "Package: {} v{} ({} files)",
        ccs_pkg.name(),
        ccs_pkg.version(),
        ccs_pkg.files().len()
    );

    let selected_components = select_ccs_components(&ccs_pkg, components)?;
    if selected_components.names.is_empty() {
        println!("Installing metadata-only package (no file components)");
    } else {
        println!(
            "Installing components: {}",
            selected_components.names.join(", ")
        );
    }

    validate_ccs_capability_declaration(&ccs_pkg)?;

    // Step 3: Check for existing installation
    let mut conn = open_db(db_path)?;

    let existing = conary_core::db::models::Trove::find_by_name(&conn, ccs_pkg.name())?;
    if !existing.is_empty() {
        let old = &existing[0];
        if old.version == ccs_pkg.version() {
            if reinstall {
                println!(
                    "Reinstalling {} {} (--reinstall)",
                    ccs_pkg.name(),
                    ccs_pkg.version()
                );
            } else {
                anyhow::bail!(
                    "Package {} version {} is already installed",
                    ccs_pkg.name(),
                    ccs_pkg.version()
                );
            }
        }
        println!(
            "Upgrading {} from {} to {}",
            ccs_pkg.name(),
            old.version,
            ccs_pkg.version()
        );
    }
    validate_incoming_version_against_dependents(
        &conn,
        ccs_pkg.name(),
        ccs_pkg.version(),
        ccs_pkg.manifest().package.version_scheme,
    )?;

    // Step 4: Check dependencies
    if no_deps {
        println!("Skipping dependency check (--no-deps)");
    } else {
        println!("Checking dependencies...");
        let effective_policy = conary_core::repository::load_effective_policy(
            &conn,
            conary_core::repository::resolution_policy::RequestScope::Any,
        )?;
        let resolution = conary_core::resolver::solve_requirement_groups_with_policy(
            &conn,
            ccs_pkg.requirements(),
            ccs_pkg.version_scheme(),
            &effective_policy.resolution,
        )?;
        if let Some(conflict) = resolution.conflict_message {
            if dry_run {
                println!("  Dependency conflict (would fail): {conflict}");
            } else {
                anyhow::bail!("dependency conflict: {conflict}");
            }
        }
        let repository_dependencies = resolution
            .install_order
            .iter()
            .filter(|package| package.source == conary_core::resolver::SatSource::Repository)
            .map(|package| format!("{} {}", package.name, package.version))
            .collect::<Vec<_>>();
        if !repository_dependencies.is_empty() {
            if dry_run {
                println!(
                    "  Missing installed dependencies (would require install): {}",
                    repository_dependencies.join(", ")
                );
            } else {
                anyhow::bail!(
                    "CCS install requires repository dependencies not yet installed: {}",
                    repository_dependencies.join(", ")
                );
            }
        }
        println!("Dependencies satisfied.");
    }

    if dry_run {
        install_ccs_package_transactionally(
            &mut conn,
            &ccs_pkg,
            CcsTransactionInstallOptions {
                db_path,
                root,
                dry_run,
                defer_generation: false,
                quiet: false,
                sandbox_mode: sandbox,
                allow_downgrade: false,
                intent: crate::commands::install::InstallIntent::PackageChange,
                reinstall,
                selection_reason: None,
                selected_manifest_components: Some(selected_components.names.clone()),
                repository_provenance: None,
            },
        )?;
        return Ok(());
    }

    validate_ccs_payload_paths(Path::new(root), &ccs_pkg, &selected_components.names)?;

    let tx_result = install_ccs_package_transactionally(
        &mut conn,
        &ccs_pkg,
        CcsTransactionInstallOptions {
            db_path,
            root,
            dry_run,
            defer_generation: false,
            quiet: false,
            sandbox_mode: sandbox,
            allow_downgrade: false,
            intent: crate::commands::install::InstallIntent::PackageChange,
            reinstall,
            selection_reason: None,
            selected_manifest_components: Some(selected_components.names.clone()),
            repository_provenance: None,
        },
    )?;
    let _changeset_id = tx_result.changeset_id;

    println!();
    println!(
        "Successfully installed {} v{}",
        ccs_pkg.name(),
        ccs_pkg.version()
    );

    Ok(())
}
