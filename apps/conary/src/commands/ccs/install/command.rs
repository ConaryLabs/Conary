// src/commands/ccs/install/command.rs

use anyhow::{Context, Result};
use conary_core::ccs::archive_reader::read_ccs_archive;
use conary_core::ccs::verify::VerificationResult;
use conary_core::ccs::{CcsPackage, TrustPolicy, verify};
use conary_core::packages::traits::PackageFormat;
use std::fs::File;
use std::path::Path;

use super::super::payload_paths::validate_ccs_payload_paths;
use super::capability_policy::enforce_ccs_capability_policy;
use super::component_selection::{select_ccs_components, sorted_available_component_names};
use super::dependency::{
    package_self_provides, validate_incoming_version_against_dependents,
    validate_package_dependency,
};
use crate::commands::install::{
    CcsTransactionInstallOptions, LegacyReplayOptions, install_ccs_package_transactionally,
};
use crate::commands::open_db;

/// Install a CCS package
///
/// This is a minimal implementation that validates and extracts the package.
/// Full transaction support will be added in a future iteration.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_ccs_install(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    components: Option<Vec<String>>,
    sandbox: crate::commands::SandboxMode,
    no_deps: bool,
    reinstall: bool,
    allow_capabilities: bool,
    capability_policy: Option<String>,
) -> Result<()> {
    cmd_ccs_install_with_replay_options(
        package,
        db_path,
        root,
        dry_run,
        allow_unsigned,
        policy,
        components,
        sandbox,
        no_deps,
        false,
        reinstall,
        allow_capabilities,
        capability_policy,
        LegacyReplayOptions::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_ccs_install_with_replay_options(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    components: Option<Vec<String>>,
    sandbox: crate::commands::SandboxMode,
    no_deps: bool,
    no_scripts: bool,
    reinstall: bool,
    allow_capabilities: bool,
    capability_policy: Option<String>,
    legacy_replay: LegacyReplayOptions,
) -> Result<()> {
    let package_path = Path::new(package);

    if !package_path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Installing CCS package: {}", package_path.display());

    let archive = read_ccs_archive(
        File::open(package_path)
            .with_context(|| format!("Failed to open package {}", package_path.display()))?,
    )
    .context("Failed to read CCS archive")?;
    let has_v2_authority = archive.v2_authority.is_some();
    if allow_unsigned && has_v2_authority {
        anyhow::bail!(
            "native CCS v2 packages require strict signature verification; --allow-unsigned cannot bypass v2 authority"
        );
    }

    let mut verification_result: Option<VerificationResult> = None;

    // Step 1: Verify signature (unless --allow-unsigned)
    if !allow_unsigned {
        let trust_policy = if let Some(policy_path) = &policy {
            TrustPolicy::from_file(Path::new(policy_path)).context("Failed to load trust policy")?
        } else {
            TrustPolicy::default()
        };

        let result = match verify::verify_package(package_path, &trust_policy) {
            Ok(result) => result,
            Err(err)
                if matches!(
                    err.downcast_ref::<conary_core::ccs::verify::VerifyError>(),
                    Some(conary_core::ccs::verify::VerifyError::NotSigned)
                ) =>
            {
                anyhow::bail!("Package is not signed. Use --allow-unsigned to install anyway.");
            }
            Err(err) => return Err(err).context("Package verification failed"),
        };
        if let Some(expired_warning) = result
            .warnings
            .iter()
            .find(|warning| warning.contains("seconds old"))
        {
            anyhow::bail!("Package signature expired: {expired_warning}");
        }
        if !result.valid {
            if trust_policy.allow_unsigned {
                println!(
                    "Warning: Package signature verification failed, but continuing (allow_unsigned policy)"
                );
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
        verification_result = Some(result);
    } else {
        println!("Warning: Skipping signature verification (--allow-unsigned)");
    }

    // Step 2: Parse the package
    println!("Parsing package...");
    let ccs_pkg = if has_v2_authority {
        let verification = verification_result.as_ref().context(
            "native CCS v2 packages require strict signature verification before parsing",
        )?;
        CcsPackage::parse_verified_v2(package, verification)?
    } else {
        CcsPackage::parse(package)?
    };

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

    enforce_ccs_capability_policy(&ccs_pkg, allow_capabilities, capability_policy.as_deref())?;

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
    validate_incoming_version_against_dependents(&conn, ccs_pkg.name(), ccs_pkg.version())?;

    // Step 4: Check dependencies
    if no_deps {
        println!("Skipping dependency check (--no-deps)");
    } else {
        println!("Checking dependencies...");
        for dep in &ccs_pkg.manifest().requires.packages {
            if package_self_provides(&ccs_pkg, &dep.name) {
                continue;
            }
            validate_package_dependency(&conn, &dep.name, dep.version.as_deref(), dry_run)?;
        }
        for cap in &ccs_pkg.manifest().requires.capabilities {
            let capability_name = cap.name();
            if package_self_provides(&ccs_pkg, capability_name) {
                continue;
            }
            let satisfied =
                conary_core::db::models::ProvideEntry::is_declared_capability_satisfied(
                    &conn,
                    capability_name,
                )?;
            if !satisfied {
                if dry_run {
                    println!("  Missing dependency: {capability_name} (would fail)");
                } else {
                    anyhow::bail!(
                        "Missing dependency: {}{}",
                        capability_name,
                        cap.version().map(|v| format!(" {v}")).unwrap_or_default()
                    );
                }
            }
        }
        println!("Dependencies satisfied.");
    }

    let available_components = sorted_available_component_names(&ccs_pkg);
    let component_selection =
        selected_components.to_install_component_selection(&available_components);

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
                no_scripts,
                sandbox_mode: match sandbox {
                    crate::commands::SandboxMode::None => conary_core::scriptlet::SandboxMode::None,
                    crate::commands::SandboxMode::Auto => conary_core::scriptlet::SandboxMode::Auto,
                    crate::commands::SandboxMode::Always => {
                        conary_core::scriptlet::SandboxMode::Always
                    }
                },
                allow_downgrade: false,
                reinstall,
                selection_reason: None,
                component_selection,
                selected_manifest_components: Some(selected_components.names.clone()),
                repository_provenance: None,
                legacy_replay,
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
            no_scripts,
            sandbox_mode: match sandbox {
                crate::commands::SandboxMode::None => conary_core::scriptlet::SandboxMode::None,
                crate::commands::SandboxMode::Auto => conary_core::scriptlet::SandboxMode::Auto,
                crate::commands::SandboxMode::Always => conary_core::scriptlet::SandboxMode::Always,
            },
            allow_downgrade: false,
            reinstall,
            selection_reason: None,
            component_selection,
            selected_manifest_components: Some(selected_components.names.clone()),
            repository_provenance: None,
            legacy_replay,
        },
    )?;
    let _changeset_id = tx_result.changeset_id;
    let post_commit_warnings = tx_result.post_commit_warnings;

    println!();
    if post_commit_warnings.is_empty() {
        println!(
            "Successfully installed {} v{}",
            ccs_pkg.name(),
            ccs_pkg.version()
        );
    } else {
        println!(
            "Installed {} v{} with warnings",
            ccs_pkg.name(),
            ccs_pkg.version()
        );
        for warning in &post_commit_warnings {
            println!("  - {warning}");
        }
    }

    Ok(())
}
