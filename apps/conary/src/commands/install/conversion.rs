// src/commands/install/conversion.rs

//! CCS conversion during package installation
//!
//! Handles converting legacy packages (RPM, DEB, Arch) to CCS format
//! during installation when --convert-to-ccs is specified.

use super::super::open_db;
use super::PackageFormatType;
use super::batch::{BatchInstaller, prepare_package_for_batch};
use super::blocklist;
use super::dep_mode::DepMode;
use super::dep_resolution;
use super::resolve::check_provides_dependencies;
use super::{
    CcsTransactionInstallOptions, ComponentSelection, LegacyReplayOptions,
    RepositoryInstallProvenance, repository_install_provenance_from_package,
    verify_static_repository_ccs_package_if_needed,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::{ConversionOptions, LegacyConverter};
use conary_core::db::models::RepositoryProvide;
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::packages::common::PackageMetadata;
use conary_core::repository;
use conary_core::resolver::MissingDependency;
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;
use std::collections::HashMap;
use std::path::Path;
use tempfile::TempDir;
use tracing::info;

/// Bounded dependency expansion for live CCS installs.
///
/// Kernel installs need to reach the initramfs toolchain without allowing a
/// full unbounded distro dependency takeover:
/// kernel -> kernel-core -> dracut -> cpio.
pub const DEFAULT_CCS_DEPENDENCY_PASSES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingCcsProvider {
    name: String,
    version: String,
    provides: Vec<String>,
}

impl PendingCcsProvider {
    fn from_package(ccs_pkg: &CcsPackage) -> Self {
        let mut provides: Vec<String> = Vec::new();
        provides.push(ccs_pkg.name().to_string());
        provides.extend(ccs_pkg.manifest().provides.capabilities.iter().cloned());
        for soname in &ccs_pkg.manifest().provides.sonames {
            provides.push(soname.clone());
            provides.push(format!("soname({soname})"));
        }
        for binary in &ccs_pkg.manifest().provides.binaries {
            provides.push(binary.clone());
            provides.push(format!("binary({binary})"));
        }
        for pkgconfig in &ccs_pkg.manifest().provides.pkgconfig {
            provides.push(pkgconfig.clone());
            provides.push(format!("pkgconfig({pkgconfig})"));
        }
        provides.sort();
        provides.dedup();

        Self {
            name: ccs_pkg.name().to_string(),
            version: ccs_pkg.version().to_string(),
            provides,
        }
    }
}

fn capability_name_matches(provided: &str, dep_name: &str) -> bool {
    provided == dep_name || provided.starts_with(&format!("{dep_name} "))
}

fn pending_provider_directly_satisfies(
    provider: &PendingCcsProvider,
    dep: &MissingDependency,
) -> bool {
    if !provider
        .provides
        .iter()
        .any(|provided| capability_name_matches(provided, &dep.name))
    {
        return false;
    }

    match &dep.constraint {
        VersionConstraint::Any => true,
        constraint => conary_core::version::RpmVersion::parse(&provider.version)
            .map(|version| constraint.satisfies(&version))
            .unwrap_or(false),
    }
}

fn pending_provider_satisfies_dependency(
    conn: &rusqlite::Connection,
    pending_providers: &[PendingCcsProvider],
    dep: &MissingDependency,
) -> bool {
    if pending_providers
        .iter()
        .any(|provider| pending_provider_directly_satisfies(provider, dep))
    {
        return true;
    }

    let requests = [(dep.name.clone(), dep.constraint.clone())];
    let Ok(resolved) = repository::resolve_dependency_requests(conn, &requests) else {
        return false;
    };

    resolved.iter().any(|(_, package)| {
        pending_providers.iter().any(|provider| {
            provider.name == package.package.name && provider.version == package.package.version
        })
    })
}

fn package_self_provides(ccs_pkg: &CcsPackage, dep_name: &str) -> bool {
    let mut provided: std::collections::HashSet<String> = std::collections::HashSet::new();
    provided.insert(ccs_pkg.name().to_string());
    provided.extend(ccs_pkg.manifest().provides.capabilities.iter().cloned());
    for soname in &ccs_pkg.manifest().provides.sonames {
        provided.insert(soname.clone());
        provided.insert(format!("soname({soname})"));
    }
    for binary in &ccs_pkg.manifest().provides.binaries {
        provided.insert(binary.clone());
        provided.insert(format!("binary({binary})"));
    }
    for pkgconfig in &ccs_pkg.manifest().provides.pkgconfig {
        provided.insert(pkgconfig.clone());
        provided.insert(format!("pkgconfig({pkgconfig})"));
    }

    provided.contains(dep_name)
}

/// Check whether a dependency string is a conditional/rich RPM dependency
/// that should be skipped during conversion install.
///
/// NOTE: This duplicates the text heuristic in `conary_core::db::models::repository`.
/// The normalized `dependency_model::ConditionalRequirementBehavior` now handles
/// this classification during repo sync.  This function remains for CCS conversion
/// of local packages where we only have the raw dependency text.
// TODO: remove after full migration -- when local package dependencies are
// parsed through the structured `dependency_model`, this heuristic is redundant.
fn is_conditional_rpm_dependency(dep_name: &str) -> bool {
    dep_name.contains(" if ")
        || dep_name.contains(" unless ")
        || dep_name.contains(" with ")
        || dep_name.contains(" without ")
        || dep_name.starts_with("((")
}

fn is_ignored_rpm_dependency(dep_name: &str) -> bool {
    dep_name.starts_with("rpmlib(")
        || dep_name.starts_with("config(")
        || dep_name.starts_with('/')
        || is_conditional_rpm_dependency(dep_name)
}

fn build_dependency_requests(
    missing: &[MissingDependency],
    to_install: &[dep_resolution::ResolvedDep],
) -> Vec<(String, VersionConstraint)> {
    to_install
        .iter()
        .map(|dep| {
            let constraint = missing
                .iter()
                .find(|candidate| candidate.name == dep.name)
                .map(|candidate| candidate.constraint.clone())
                .unwrap_or(VersionConstraint::Any);
            (dep.name.clone(), constraint)
        })
        .collect()
}

fn is_already_installed_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("already installed"))
}

/// Check whether a single dependency can be found in any enabled repository.
///
/// This is intentionally non-transitive: we only check existence, not whether
/// the package's own dependencies are satisfiable.  The full transitive SAT
/// solve happens later, during the actual download/install step.
fn resolve_repository_dependency_name(
    conn: &rusqlite::Connection,
    dep: &MissingDependency,
) -> Option<String> {
    let requests = [(dep.name.clone(), dep.constraint.clone())];
    if repository::resolve_dependency_requests(conn, &requests)
        .map(|resolved| !resolved.is_empty())
        .unwrap_or(false)
    {
        return Some(dep.name.clone());
    }

    RepositoryProvide::find_by_cli_exact_query(conn, &dep.name)
        .ok()?
        .into_iter()
        .find_map(|provider| {
            if provider.capability == dep.name {
                return None;
            }

            let requests = [(provider.capability.clone(), dep.constraint.clone())];
            repository::resolve_dependency_requests(conn, &requests)
                .ok()
                .filter(|resolved| !resolved.is_empty())
                .map(|_| provider.capability)
        })
}

fn promote_repo_resolvable_satisfy_deps(
    conn: &rusqlite::Connection,
    dep_plan: &mut dep_resolution::DepResolutionPlan,
) {
    if dep_plan.unresolvable.is_empty() {
        return;
    }

    // Partition unresolvable deps into repo-found vs still-unresolvable using
    // lightweight per-package lookups instead of a full transitive SAT solve.
    let mut promoted = Vec::new();
    let mut still_unresolvable = Vec::new();

    for dep in dep_plan.unresolvable.drain(..) {
        // Critical live-root packages and runtime capabilities must never be
        // promoted into repository installs during satisfy mode. If they reach
        // this point, keep honoring the blocklist boundary instead of asking
        // Remi to replace packages such as systemd, coreutils, or glibc.
        if blocklist::is_blocked(&dep.name) {
            info!(
                "Keeping satisfy-mode dependency '{}' blocked on the live system instead of promoting it to a repository install",
                dep.name
            );
            dep_plan.blocked.push(dep.name);
            continue;
        }

        if let Some(resolution_name) = resolve_repository_dependency_name(conn, &dep) {
            promoted.push((dep, resolution_name));
        } else {
            still_unresolvable.push(dep);
        }
    }

    if !promoted.is_empty() {
        info!(
            "Promoting {} satisfy-mode dependencies to repository installs: {}",
            promoted.len(),
            promoted
                .iter()
                .map(|(_, resolution_name)| resolution_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (dep, resolution_name) in promoted {
            if dep_plan
                .to_install
                .iter()
                .all(|existing| existing.name != resolution_name)
            {
                dep_plan.to_install.push(dep_resolution::ResolvedDep {
                    name: resolution_name,
                    version: Some(dep.constraint.to_string()),
                    required_by: dep.required_by,
                });
            }
        }
    }

    dep_plan.unresolvable = still_unresolvable;
}

/// Result of attempting CCS conversion
pub enum ConversionResult {
    /// Package was converted, install via CCS path
    Converted { ccs_path: String, temp_dir: TempDir },
    /// Conversion skipped (already converted or not needed)
    Skipped,
}

pub struct ConvertedCcsInstallOptions<'a> {
    pub ccs_path: &'a str,
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub sandbox_mode: SandboxMode,
    pub no_deps: bool,
    pub no_scripts: bool,
    pub allow_downgrade: bool,
    pub allow_capabilities: bool,
    pub dep_mode: Option<DepMode>,
    pub yes: bool,
    pub dependency_passes_remaining: usize,
    pub repository_provenance: Option<RepositoryInstallProvenance>,
    pub legacy_replay: LegacyReplayOptions,
}

/// Attempt to convert a legacy package to CCS format
///
/// Returns `ConversionResult::Converted` if conversion succeeded and installation
/// should proceed via the CCS installer, or `ConversionResult::Skipped` if
/// conversion was skipped (e.g., already converted).
pub async fn try_convert_to_ccs(
    pkg: &dyn PackageFormat,
    package_path: &Path,
    format: PackageFormatType,
    db_path: &str,
    allow_capabilities: bool,
) -> Result<ConversionResult> {
    info!("Converting {} to CCS format...", pkg.name());

    // Compute checksum of original package for deduplication
    let package_bytes = std::fs::read(package_path).with_context(|| {
        format!(
            "Failed to read package file for checksum: {}",
            package_path.display()
        )
    })?;
    let original_checksum = conary_core::hash::sha256_prefixed(&package_bytes);

    // Determine format string
    let format_str = match format {
        PackageFormatType::Rpm => "rpm",
        PackageFormatType::Deb => "deb",
        PackageFormatType::Arch => "arch",
    };

    // Open database early to check for existing conversion
    let conn = open_db(db_path)?;

    // Check if already converted (skip re-conversion)
    if let Some(existing) =
        conary_core::db::models::ConvertedPackage::find_by_checksum(&conn, &original_checksum)?
    {
        if existing.needs_reconversion() {
            info!("Re-converting {} (algorithm upgraded)", pkg.name());
            conary_core::db::models::ConvertedPackage::delete_by_checksum(
                &conn,
                &original_checksum,
            )?;
        } else {
            // Already converted and up to date
            info!(
                "Package {} already converted, using regular install path",
                pkg.name()
            );
            println!(
                "Note: {} was previously converted - using standard install",
                pkg.name()
            );
            return Ok(ConversionResult::Skipped);
        }
    }

    // Extract files for conversion
    let extracted = pkg
        .extract_file_contents()
        .with_context(|| format!("Failed to extract files for conversion: {}", pkg.name()))?;

    // Build PackageMetadata from the package
    let metadata = PackageMetadata {
        package_path: package_path.to_path_buf(),
        name: pkg.name().to_string(),
        version: pkg.version().to_string(),
        architecture: pkg.architecture().map(|s| s.to_string()),
        description: pkg.description().map(|s| s.to_string()),
        files: pkg.files().to_vec(),
        dependencies: pkg.dependencies().to_vec(),
        provides: pkg.provides().to_vec(),
        scriptlets: pkg.scriptlets().to_vec(),
        native_scriptlet_abi: pkg.native_scriptlet_abi().to_vec(),
        config_files: Vec::new(),
    };

    // Create temp directory for CCS output
    let ccs_temp = TempDir::new().context("Failed to create temp directory for CCS conversion")?;

    let options = ConversionOptions {
        enable_chunking: true,
        output_dir: ccs_temp.path().to_path_buf(),
    };

    let converter = LegacyConverter::new(options);
    let conversion_result = converter
        .convert(&metadata, &extracted, format_str, &original_checksum)
        .with_context(|| format!("Failed to convert {} to CCS format", pkg.name()))?;

    // Get the package path
    let ccs_package_path = conversion_result
        .package_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Conversion succeeded but no package path returned"))?;
    let converted_ccs_path = ccs_package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Converted CCS path is not valid UTF-8"))?;
    let converted_ccs_pkg = CcsPackage::parse(converted_ccs_path)
        .context("Failed to parse converted CCS package for capability policy")?;
    crate::commands::ccs::enforce_ccs_capability_policy(
        &converted_ccs_pkg,
        allow_capabilities,
        None,
    )?;

    info!(
        "Converted {} to CCS format: {} (scriptlet_fidelity: {})",
        pkg.name(),
        ccs_package_path.display(),
        conversion_result.scriptlet_metadata.scriptlet_fidelity
    );

    // Serialize extracted provenance to JSON for audit trail
    let provenance_json = conversion_result
        .legacy_provenance
        .as_ref()
        .and_then(|prov| prov.to_json().ok());

    if let Some(ref prov) = conversion_result.legacy_provenance
        && prov.has_content()
    {
        info!("Provenance extracted: {}", prov.summary());
    }

    // Create conversion record
    let mut converted_pkg = conary_core::db::models::ConvertedPackage::new(
        conversion_result.original_format.clone(),
        conversion_result.original_checksum.clone(),
    );
    converted_pkg.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;
    converted_pkg.extracted_provenance_json = provenance_json;
    converted_pkg.insert(&conn)?;

    let ccs_path = ccs_package_path.to_string_lossy().to_string();
    Ok(ConversionResult::Converted {
        ccs_path,
        temp_dir: ccs_temp,
    })
}

/// Install a converted CCS package
///
/// This is a wrapper that calls the CCS installer with appropriate options.
pub async fn install_converted_ccs(opts: ConvertedCcsInstallOptions<'_>) -> Result<()> {
    install_converted_ccs_with_pending(opts, Vec::new(), false).await
}

async fn install_converted_ccs_with_pending(
    opts: ConvertedCcsInstallOptions<'_>,
    pending_providers: Vec<PendingCcsProvider>,
    defer_generation: bool,
) -> Result<()> {
    let ConvertedCcsInstallOptions {
        ccs_path,
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        no_scripts,
        allow_downgrade,
        allow_capabilities,
        dep_mode,
        yes,
        dependency_passes_remaining,
        repository_provenance,
        legacy_replay,
    } = opts;

    verify_static_repository_ccs_package_if_needed(
        db_path,
        Path::new(ccs_path),
        repository_provenance.as_ref(),
    )?;

    let ccs_pkg = CcsPackage::parse(ccs_path).context("Failed to parse converted CCS package")?;
    crate::commands::ccs::enforce_ccs_capability_policy(&ccs_pkg, allow_capabilities, None)?;

    if !no_deps {
        let conn = open_db(db_path)?;
        let mut scoped_pending_providers = pending_providers;
        scoped_pending_providers.push(PendingCcsProvider::from_package(&ccs_pkg));
        let missing: Vec<MissingDependency> = ccs_pkg
            .dependencies()
            .iter()
            .filter(|dep| !package_self_provides(&ccs_pkg, &dep.name))
            // Skip RPM-internal capabilities and filesystem deps.
            // TODO: remove after full migration -- use scheme-aware dependency
            // classification from `dependency_model` instead of string prefixes.
            .filter(|dep| !is_ignored_rpm_dependency(&dep.name))
            .map(|dep| MissingDependency {
                name: dep.name.clone(),
                constraint: dep
                    .version
                    .as_ref()
                    .and_then(|v| VersionConstraint::parse(v).ok())
                    .unwrap_or(VersionConstraint::Any),
                required_by: vec![ccs_pkg.name().to_string()],
            })
            .collect();

        if !missing.is_empty() {
            let (tracked_satisfied, unresolved_missing) =
                check_provides_dependencies(&conn, &missing);

            for (dep_name, provider, _version) in &tracked_satisfied {
                info!(
                    "Dependency {} already satisfied by tracked provider {}",
                    dep_name, provider
                );
            }
            let mut pending_satisfied = Vec::new();
            let mut still_unresolved_missing = Vec::new();
            for dep in unresolved_missing {
                if pending_provider_satisfies_dependency(&conn, &scoped_pending_providers, &dep) {
                    pending_satisfied.push(dep);
                } else {
                    still_unresolved_missing.push(dep);
                }
            }
            for dep in &pending_satisfied {
                info!(
                    "Dependency {} already satisfied by pending CCS transaction provider",
                    dep.name
                );
            }
            let unresolved_missing = still_unresolved_missing;

            // Resolve the effective dep-mode from the explicit option or
            // the system model convergence intent.
            let convergence_intent = if conary_core::model::model_exists(None) {
                conary_core::model::load_model(None)
                    .ok()
                    .map(|m| m.system.convergence.clone())
                    .unwrap_or_default()
            } else {
                conary_core::model::ConvergenceIntent::default()
            };
            let effective =
                dep_mode.unwrap_or_else(|| DepMode::from_convergence_intent(&convergence_intent));
            let mut dep_plan = dep_resolution::resolve_missing_deps_policy_aware(
                &conn,
                &unresolved_missing,
                Some(effective),
                &convergence_intent,
            );
            if matches!(effective, DepMode::Satisfy) {
                promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);
            }

            if !dep_plan.to_adopt.is_empty() && !dry_run {
                crate::commands::adopt::cmd_adopt(&dep_plan.to_adopt, db_path, false, false)
                    .await?;
            }

            if !dep_plan.to_install.is_empty() {
                let dep_requests =
                    build_dependency_requests(&unresolved_missing, &dep_plan.to_install);

                if dry_run {
                    repository::resolve_dependency_requests(&conn, &dep_requests).with_context(
                        || {
                            format!(
                                "Failed to resolve dependencies from repositories for '{}'",
                                ccs_pkg.name()
                            )
                        },
                    )?;
                } else {
                    if !yes {
                        println!();
                        print!(
                            "Proceed with {} dependency changes? [Y/n] ",
                            dep_plan.to_install.len() + dep_plan.to_adopt.len()
                        );
                        use std::io::Write;
                        std::io::stdout().flush()?;

                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input)?;
                        let input = input.trim().to_lowercase();
                        if input == "n" || input == "no" {
                            println!("Cancelled.");
                            return Ok(());
                        }
                    }

                    let to_download =
                        repository::resolve_dependency_requests(&conn, &dep_requests)?;
                    if !to_download.is_empty() {
                        let temp_dir = TempDir::new()?;
                        let keyring_dir = keyring_dir(db_path);
                        let mut provenance_by_dep = HashMap::new();
                        for (dep_name, pkg_with_repo) in &to_download {
                            provenance_by_dep.insert(
                                dep_name.clone(),
                                repository_install_provenance_from_package(
                                    &pkg_with_repo.package,
                                    &pkg_with_repo.repository,
                                )?,
                            );
                        }
                        let downloaded = repository::download_dependencies(
                            &to_download,
                            temp_dir.path(),
                            Some(&keyring_dir),
                        )
                        .await?;
                        let parent_name = ccs_pkg.name().to_string();
                        let mut prepared_packages = Vec::with_capacity(downloaded.len());

                        for (dep_name, dep_path) in &downloaded {
                            if dep_path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("ccs"))
                            {
                                let dep_ccs_path = dep_path.to_str().ok_or_else(|| {
                                    anyhow::anyhow!("Invalid CCS path (non-UTF8)")
                                })?;
                                // Allow one nested dependency pass for
                                // downloaded CCS dependencies. Kernel meta
                                // packages need this for kernel-core -> dracut,
                                // but grandchildren remain non-recursive inside
                                // that child call so dependency expansion stays
                                // bounded.
                                let nested_dependency_passes =
                                    dependency_passes_remaining.saturating_sub(1);
                                let child_pending_providers = scoped_pending_providers.clone();
                                let install_result = Box::pin(install_converted_ccs_with_pending(
                                    ConvertedCcsInstallOptions {
                                        ccs_path: dep_ccs_path,
                                        db_path,
                                        root,
                                        dry_run,
                                        sandbox_mode,
                                        no_deps: dependency_passes_remaining == 0,
                                        no_scripts,
                                        allow_downgrade,
                                        allow_capabilities,
                                        dep_mode,
                                        yes,
                                        dependency_passes_remaining: nested_dependency_passes,
                                        repository_provenance: provenance_by_dep
                                            .get(dep_name)
                                            .cloned(),
                                        legacy_replay,
                                    },
                                    child_pending_providers,
                                    true,
                                ))
                                .await;
                                match install_result {
                                    Ok(()) => {}
                                    Err(e) if is_already_installed_error(&e) => {
                                        info!(
                                            "Dependency {} already installed, skipping",
                                            dep_name
                                        );
                                    }
                                    Err(e) => {
                                        return Err(e).with_context(|| {
                                            format!("Failed to install CCS dependency {}", dep_name)
                                        });
                                    }
                                }
                                continue;
                            }

                            let reason = format!("Required by {}", parent_name);
                            match prepare_package_for_batch(
                                dep_path,
                                db_path,
                                &reason,
                                allow_downgrade,
                            ) {
                                Ok(prepared) => {
                                    let mut prepared = prepared;
                                    prepared.repository_provenance =
                                        provenance_by_dep.get(dep_name).cloned();
                                    prepared_packages.push(prepared);
                                }
                                Err(e) if e.to_string().contains("already installed") => {
                                    info!("Dependency {} already installed, skipping", dep_name);
                                }
                                Err(e) => {
                                    return Err(anyhow::anyhow!(
                                        "Failed to prepare dependency {}: {}",
                                        dep_name,
                                        e
                                    ));
                                }
                            }
                        }

                        if !prepared_packages.is_empty() {
                            let installer = BatchInstaller::new(
                                db_path,
                                root,
                                sandbox_mode,
                                no_scripts,
                                legacy_replay,
                            );
                            installer.install_batch(prepared_packages)?;
                        }
                    }
                }
            }

            if !dep_plan.unresolvable.is_empty() {
                let (_satisfied, still_missing) =
                    check_provides_dependencies(&conn, &dep_plan.unresolvable);
                if !still_missing.is_empty() {
                    let mut detail_lines = Vec::new();
                    for dep in &still_missing {
                        detail_lines.push(format!(
                            "  {} {} (required by: {})",
                            dep.name,
                            dep.constraint,
                            dep.required_by.join(", "),
                        ));
                    }
                    return Err(anyhow::anyhow!(
                        "Cannot install {}: {} unresolvable dependencies (dep-mode={}, convergence={}):\n{}",
                        ccs_pkg.name(),
                        still_missing.len(),
                        effective,
                        convergence_intent.display_name(),
                        detail_lines.join("\n"),
                    ));
                }
            }
        }
    }

    println!("Installing converted CCS package...");
    let mut conn = open_db(db_path)?;
    super::install_ccs_package_transactionally(
        &mut conn,
        &ccs_pkg,
        CcsTransactionInstallOptions {
            db_path,
            root,
            dry_run,
            defer_generation,
            quiet: false,
            no_scripts,
            sandbox_mode,
            allow_downgrade,
            reinstall: false,
            selection_reason: None,
            component_selection: ComponentSelection::All,
            selected_manifest_components: None,
            repository_provenance,
            legacy_replay,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
