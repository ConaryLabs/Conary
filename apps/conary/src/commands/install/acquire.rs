// src/commands/install/acquire.rs

use super::conversion::{
    ConversionResult, ConvertedCcsInstallOptions, DEFAULT_CCS_DEPENDENCY_PASSES,
    install_converted_ccs, try_convert_to_ccs,
};
use super::prepare::parse_package;
use super::resolve::{
    PolicyOptions, ResolutionOutcome, ResolvedPackage, ResolvedSourceType,
    resolve_package_path_with_policy,
};
use super::{
    DepMode, InstallPhase, InstallProgress, LegacyReplayOptions, PackageFormatType,
    RepositoryInstallProvenance, detect_package_format,
};
use anyhow::{Context, Result};
use conary_core::packages::PackageFormat;
use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
use conary_core::repository::resolution_policy::ResolutionPolicy;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use tracing::info;

/// Parameters for CCS direct-install that are forwarded from `InstallOptions`.
pub(super) struct CcsInstallParams<'a> {
    pub(super) db_path: &'a str,
    pub(super) root: &'a str,
    pub(super) dry_run: bool,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) no_deps: bool,
    pub(super) no_scripts: bool,
    pub(super) allow_downgrade: bool,
    pub(super) dep_mode: Option<DepMode>,
    pub(super) yes: bool,
    pub(super) repository_provenance: Option<RepositoryInstallProvenance>,
    pub(super) legacy_replay: LegacyReplayOptions,
}

/// Resolve a package path, detect its format, and parse it.
///
/// Handles early returns for CCS packages (from Remi, by extension, or via
/// conversion).  Returns `None` if the package was already installed as CCS
/// (no further processing needed), or `Some(...)` with the parsed legacy
/// package and its format type.
#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_and_parse_package(
    conn: &rusqlite::Connection,
    package_name: &str,
    package: &str,
    db_path: &str,
    version: Option<&str>,
    repo: Option<&str>,
    architecture: Option<&str>,
    convert_to_ccs: bool,
    no_capture: bool,
    policy: &ResolutionPolicy,
    primary_flavor: Option<RepositoryDependencyFlavor>,
    ccs_opts: &CcsInstallParams<'_>,
) -> Result<
    Option<(
        Box<dyn PackageFormat>,
        PackageFormatType,
        Option<RepositoryInstallProvenance>,
    )>,
> {
    // Create progress tracker for single package installation
    let progress = InstallProgress::single("Installing");
    progress.set_phase(package_name, InstallPhase::Downloading);

    // Build policy options for resolution.  The root request carries the
    // full policy; transitive deps will inherit the mixing policy but not
    // the request scope (that is handled inside the resolver).
    let policy_opts = PolicyOptions {
        policy: Some(policy.clone()),
        is_root: true,
        primary_flavor,
    };

    // Resolve package path (download if needed).
    // Checksum verification and temp-file cleanup on failure are handled
    // inside conary_core::repository::download (fix 1.4).
    // TODO(round2): Surface partial-download byte counts in error messages
    // so users can diagnose connection issues vs corrupt mirrors.
    let resolved = match resolve_package_path_with_policy(
        package_name,
        db_path,
        version,
        repo,
        architecture,
        &progress,
        &policy_opts,
    )
    .await
    {
        Err(e) => {
            print_package_suggestions(conn, package_name);
            return Err(e);
        }
        Ok(ResolutionOutcome::AlreadyInstalled { name, version }) => {
            // Use a specific error type that the caller handles as a clean exit
            return Err(anyhow::anyhow!(
                "ALREADY_INSTALLED:{} {} is already installed (skipping download)",
                name,
                version
            ));
        }
        Ok(ResolutionOutcome::Resolved(pkg)) => pkg,
    };

    // If resolved from Remi, it's already CCS format - install directly
    if resolved.source_type == ResolvedSourceType::Remi {
        info!("Package from Remi is already CCS format, installing directly");
        let ccs_path = resolved
            .path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid CCS path (non-UTF8)"))?;
        install_converted_ccs(ConvertedCcsInstallOptions {
            ccs_path,
            db_path: ccs_opts.db_path,
            root: ccs_opts.root,
            dry_run: ccs_opts.dry_run,
            sandbox_mode: ccs_opts.sandbox_mode,
            no_deps: ccs_opts.no_deps,
            no_scripts: ccs_opts.no_scripts,
            allow_downgrade: ccs_opts.allow_downgrade,
            dep_mode: ccs_opts.dep_mode,
            yes: ccs_opts.yes,
            dependency_passes_remaining: DEFAULT_CCS_DEPENDENCY_PASSES,
            repository_provenance: install_provenance_from_resolved(&resolved)
                .or_else(|| ccs_opts.repository_provenance.clone()),
            legacy_replay: ccs_opts.legacy_replay,
        })
        .await?;
        return Ok(None);
    }

    let path_str = resolved
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    // Check if it's a CCS package by extension (from update command or local file)
    if path_str.ends_with(".ccs") {
        info!("Detected CCS package from path extension, installing directly");
        install_converted_ccs(ConvertedCcsInstallOptions {
            ccs_path: path_str,
            db_path: ccs_opts.db_path,
            root: ccs_opts.root,
            dry_run: ccs_opts.dry_run,
            sandbox_mode: ccs_opts.sandbox_mode,
            no_deps: ccs_opts.no_deps,
            no_scripts: ccs_opts.no_scripts,
            allow_downgrade: ccs_opts.allow_downgrade,
            dep_mode: ccs_opts.dep_mode,
            yes: ccs_opts.yes,
            dependency_passes_remaining: DEFAULT_CCS_DEPENDENCY_PASSES,
            repository_provenance: install_provenance_from_resolved(&resolved)
                .or_else(|| ccs_opts.repository_provenance.clone()),
            legacy_replay: ccs_opts.legacy_replay,
        })
        .await?;
        return Ok(None);
    }

    // Detect format and parse legacy packages
    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
    info!("Detected package format: {:?}", format);

    let repository_provenance = install_provenance_from_resolved(&resolved)
        .or_else(|| ccs_opts.repository_provenance.clone());

    progress.set_phase(package, InstallPhase::Parsing);
    let pkg = parse_package(&resolved.path, format)?;

    // Convert to CCS format if requested (only for legacy packages)
    if convert_to_ccs {
        progress.set_status(&format!("Converting {} to CCS format...", pkg.name()));

        match try_convert_to_ccs(pkg.as_ref(), &resolved.path, format, db_path, !no_capture).await?
        {
            ConversionResult::Converted {
                ccs_path,
                temp_dir: _temp_dir,
            } => {
                // Install via CCS path (temp_dir kept alive until install completes)
                install_converted_ccs(ConvertedCcsInstallOptions {
                    ccs_path: &ccs_path,
                    db_path: ccs_opts.db_path,
                    root: ccs_opts.root,
                    dry_run: ccs_opts.dry_run,
                    sandbox_mode: ccs_opts.sandbox_mode,
                    no_deps: ccs_opts.no_deps,
                    no_scripts: ccs_opts.no_scripts,
                    allow_downgrade: ccs_opts.allow_downgrade,
                    dep_mode: ccs_opts.dep_mode,
                    yes: ccs_opts.yes,
                    dependency_passes_remaining: DEFAULT_CCS_DEPENDENCY_PASSES,
                    repository_provenance: install_provenance_from_resolved(&resolved)
                        .or_else(|| ccs_opts.repository_provenance.clone()),
                    legacy_replay: ccs_opts.legacy_replay,
                })
                .await?;
                return Ok(None);
            }
            ConversionResult::Skipped => {
                // Already converted - fall through to regular install path
            }
        }
    }

    Ok(Some((pkg, format, repository_provenance)))
}

fn install_provenance_from_resolved(
    resolved: &ResolvedPackage,
) -> Option<RepositoryInstallProvenance> {
    resolved
        .repository_provenance
        .as_ref()
        .map(|provenance| RepositoryInstallProvenance {
            repository_id: provenance.repository_id,
            source_distro: provenance.source_distro.clone(),
            version_scheme: provenance.version_scheme.clone(),
        })
}

/// Search canonical packages and repository packages for names similar to the
/// given query. Returns up to 5 `(name, distros)` pairs suitable for "did you
/// mean?" suggestions.
fn find_package_suggestions(
    conn: &rusqlite::Connection,
    name: &str,
) -> std::result::Result<Vec<(String, String)>, rusqlite::Error> {
    let mut hits: HashMap<String, Vec<String>> = HashMap::new();

    // 1. Search canonical_packages by substring
    {
        let mut stmt = conn.prepare(
            "SELECT cp.name, pi.distro
             FROM canonical_packages cp
             LEFT JOIN package_implementations pi ON pi.canonical_id = cp.id
             WHERE cp.name LIKE '%' || ?1 || '%'
             LIMIT 10",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (pkg_name, distro) = row?;
            let entry = hits.entry(pkg_name).or_default();
            if let Some(d) = distro
                && !entry.contains(&d)
            {
                entry.push(d);
            }
        }
    }

    // 2. Search repository_packages by prefix
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp.name, r.name
             FROM repository_packages rp
             JOIN repositories r ON r.id = rp.repository_id
             WHERE rp.name LIKE ?1 || '%'
             LIMIT 10",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (pkg_name, repo_name) = row?;
            let entry = hits.entry(pkg_name).or_default();
            if !entry.contains(&repo_name) {
                entry.push(repo_name);
            }
        }
    }

    // Remove exact match (the user already tried it)
    hits.remove(name);

    // Collect, sort by name, and take top 5
    let mut results: Vec<(String, String)> = hits
        .into_iter()
        .map(|(pkg, distros)| {
            let info = if distros.is_empty() {
                String::new()
            } else {
                distros.join(", ")
            };
            (pkg, info)
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.truncate(5);
    Ok(results)
}

/// Print "did you mean?" suggestions when a package is not found.
///
/// Silently does nothing if the DB query fails (don't make a bad error worse).
fn print_package_suggestions(conn: &rusqlite::Connection, package_name: &str) {
    if let Ok(suggestions) = find_package_suggestions(conn, package_name)
        && !suggestions.is_empty()
    {
        eprintln!("\nDid you mean:");
        for (name, distros) in suggestions.iter().take(5) {
            if distros.is_empty() {
                eprintln!("  {name}");
            } else {
                eprintln!("  {name:<20} ({distros})");
            }
        }
        let stem = package_name.split('-').next().unwrap_or(package_name);
        eprintln!("\nUse 'conary canonical search {}' for more options.", stem);
    }
}
