// src/commands/ccs/install/command.rs

use anyhow::{Context, Result};
use conary_core::ccs::{CcsPackage, TrustPolicy, verify};
use conary_core::packages::traits::PackageFormat;
use std::path::Path;

use super::super::payload_paths::validate_ccs_payload_paths;
use super::capability_declaration::validate_ccs_capability_declaration;
use super::component_selection::select_ccs_components;
use super::dependency::{incoming_package_identity, validate_incoming_version_against_dependents};
use crate::commands::install::{
    CcsTransactionInstallOptions, InstallIntent, UpgradeCheck, check_ccs_upgrade_status,
    install_ccs_package_transactionally, install_semantics_for_ccs_manifest,
};
use crate::commands::open_db;

/// Resolve the replacement trove through the same CCS upgrade authority used
/// by the transaction, including its exact already-installed rule.
pub(super) fn check_ccs_install_upgrade_status(
    conn: &rusqlite::Connection,
    pkg: &CcsPackage,
    reinstall: bool,
) -> Result<UpgradeCheck> {
    let semantics = install_semantics_for_ccs_manifest(pkg.manifest())?;
    check_ccs_upgrade_status(
        conn,
        pkg,
        &semantics,
        false,
        InstallIntent::PackageChange,
        reinstall,
    )
}

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
    let permanent_cas = (!dry_run)
        .then(|| {
            let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path);
            conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        })
        .transpose()?;
    let verification = match &permanent_cas {
        Some(cas) => verify::verify_package_into_cas(package_path, &trust_policy, cas),
        None => verify::verify_package(package_path, &trust_policy),
    }
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

    let upgrade = check_ccs_install_upgrade_status(&conn, &ccs_pkg, reinstall)?;
    let replacing = match &upgrade {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::AlreadyInstalled(trove) => {
            anyhow::bail!(
                "Package {} version {} ({}) is already installed",
                trove.name,
                trove.version,
                trove.architecture.as_deref().unwrap_or("no-arch")
            )
        }
        UpgradeCheck::Upgrade(trove)
        | UpgradeCheck::Downgrade(trove)
        | UpgradeCheck::Replatform(trove) => {
            if reinstall
                && trove.version == ccs_pkg.version()
                && trove.package_release.as_deref() == ccs_pkg.package_release()
                && trove.architecture.as_deref() == ccs_pkg.architecture()
            {
                println!(
                    "Reinstalling {} {} (--reinstall)",
                    ccs_pkg.name(),
                    ccs_pkg.version()
                );
            } else {
                println!(
                    "Upgrading {} from {} to {}",
                    ccs_pkg.name(),
                    trove.version,
                    ccs_pkg.version()
                );
            }
            Some(trove.as_ref())
        }
    };

    // The validator and the transaction must reason about the same exact
    // outgoing identities. Upgrade selection is architecture-slot based for a
    // CCS install; relation removals are already planned by exact trove ID.
    let relation_plan = conary_core::transaction::plan_package_relations(
        &conn,
        &ccs_pkg,
        ccs_pkg.manifest().package.version_scheme,
    )?;
    let mut outgoing_trove_ids = relation_plan
        .removals
        .iter()
        .map(|removal| removal.trove_id)
        .collect::<Vec<_>>();
    if let Some(trove) = replacing {
        // Relation removal and replacement may duplicate an ID intentionally; the validator deduplicates it.
        let trove_id = trove.id.ok_or_else(|| {
            anyhow::anyhow!(
                "CCS replacement trove '{} {} ({})' has no database id",
                trove.name,
                trove.version,
                trove.architecture.as_deref().unwrap_or("no-arch")
            )
        })?;
        outgoing_trove_ids.push(trove_id);
    }
    let incoming_identity = incoming_package_identity(&ccs_pkg)?;
    validate_incoming_version_against_dependents(&conn, &outgoing_trove_ids, &incoming_identity)?;

    // Step 4: Check dependencies
    if no_deps {
        println!("Skipping dependency check (--no-deps)");
    } else {
        println!("Checking dependencies...");
        let effective_policy = conary_core::repository::load_effective_policy(
            &conn,
            conary_core::repository::resolution_policy::RequestScope::Any,
        )?;
        let resolution = conary_core::resolver::solve_package_requirements_with_policy(
            &conn,
            &ccs_pkg,
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
                requested_source_identity: None,
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
            requested_source_identity: None,
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
