// src/commands/install/conversion.rs

//! CCS conversion during package installation
//!
//! Handles converting legacy packages (RPM, DEB, Arch) to CCS format
//! during installation when --convert-to-ccs is specified.

use super::super::open_db;
use super::PackageFormatType;
use super::batch::{BatchInstaller, prepare_package_for_batch};
use super::dep_mode::DepMode;
use super::dep_resolution;
use super::resolve::check_provides_dependencies;
use anyhow::{Context, Result};
use conary_core::capability::inference::InferenceOptions;
use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::{ConversionOptions, FidelityLevel, LegacyConverter};
use conary_core::db::models::RepositoryProvide;
use conary_core::db::models::generate_capability_variations;
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::packages::common::PackageMetadata;
use conary_core::repository;
use conary_core::repository::selector::{PackageSelector, SelectionOptions};
use conary_core::resolver::MissingDependency;
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;
use sha2::{Digest, Sha256};
use std::path::Path;
use tempfile::TempDir;
use tracing::{info, warn};

fn package_self_provides(ccs_pkg: &CcsPackage, dep_name: &str) -> bool {
    let provided: std::collections::HashSet<String> = std::iter::once(ccs_pkg.name().to_string())
        .chain(ccs_pkg.manifest().provides.capabilities.iter().cloned())
        .chain(ccs_pkg.manifest().provides.sonames.iter().cloned())
        .chain(ccs_pkg.manifest().provides.binaries.iter().cloned())
        .chain(ccs_pkg.manifest().provides.pkgconfig.iter().cloned())
        .collect();

    if provided.contains(dep_name) {
        return true;
    }

    for variation in generate_capability_variations(dep_name) {
        if provided.contains(&variation) {
            return true;
        }
    }

    false
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

/// Check whether a single dependency can be found in any enabled repository,
/// either by direct name, capability variation, or normalized provides lookup.
///
/// This is intentionally non-transitive: we only check existence, not whether
/// the package's own dependencies are satisfiable.  The full transitive SAT
/// solve happens later, during the actual download/install step.
fn is_repo_resolvable(conn: &rusqlite::Connection, dep_name: &str) -> bool {
    let options = SelectionOptions::default();

    // 1. Direct package name lookup
    if PackageSelector::find_best_package(conn, dep_name, &options).is_ok() {
        return true;
    }

    // 2. Capability variations (e.g. strip arch suffix, soname stems)
    for variation in generate_capability_variations(dep_name) {
        if PackageSelector::find_best_package(conn, &variation, &options).is_ok() {
            return true;
        }
    }

    // 3. Normalized repository_provides table
    if let Ok(provides) = RepositoryProvide::find_by_capability(conn, dep_name)
        && !provides.is_empty()
    {
        return true;
    }

    false
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
        if is_repo_resolvable(conn, &dep.name) {
            promoted.push(dep);
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
                .map(|dep| dep.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for dep in promoted {
            if dep_plan
                .to_install
                .iter()
                .all(|existing| existing.name != dep.name)
            {
                dep_plan.to_install.push(dep_resolution::ResolvedDep {
                    name: dep.name,
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
    pub dep_mode: DepMode,
    pub yes: bool,
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
    capture_scriptlets: bool,
) -> Result<ConversionResult> {
    info!("Converting {} to CCS format...", pkg.name());

    // Compute checksum of original package for deduplication
    let package_bytes = std::fs::read(package_path).with_context(|| {
        format!(
            "Failed to read package file for checksum: {}",
            package_path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&package_bytes);
    let hash_result = hasher.finalize();
    let original_checksum = format!("sha256:{:x}", hash_result);

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
        scriptlets: pkg.scriptlets().to_vec(),
        config_files: Vec::new(),
    };

    // Create temp directory for CCS output
    let ccs_temp = TempDir::new().context("Failed to create temp directory for CCS conversion")?;

    let options = ConversionOptions {
        enable_chunking: true,
        output_dir: ccs_temp.path().to_path_buf(),
        auto_classify: true,
        min_fidelity: FidelityLevel::Partial,
        capture_scriptlets,
        enable_inference: true,
        inference_options: InferenceOptions::fast(),
    };

    let converter = LegacyConverter::new(options);
    let conversion_result = converter
        .convert(&metadata, &extracted, format_str, &original_checksum)
        .with_context(|| format!("Failed to convert {} to CCS format", pkg.name()))?;

    // Warn if fidelity is below High
    if conversion_result.fidelity.level < FidelityLevel::High {
        warn!(
            "Conversion fidelity is {}: complex scripts may not be fully analyzed",
            conversion_result.fidelity.level
        );
        eprintln!(
            "WARNING: Conversion fidelity is {} - complex legacy scripts may not be fully analyzed",
            conversion_result.fidelity.level
        );
    }

    // Get the package path
    let ccs_package_path = conversion_result
        .package_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Conversion succeeded but no package path returned"))?;

    info!(
        "Converted {} to CCS format: {} (fidelity: {})",
        pkg.name(),
        ccs_package_path.display(),
        conversion_result.fidelity.level
    );

    // Serialize hooks to JSON for storage
    let hooks_json = serde_json::to_string(&conversion_result.detected_hooks)
        .unwrap_or_else(|_| "{}".to_string());

    // Serialize inferred capabilities to JSON for audit trail
    let inferred_caps_json = conversion_result
        .inferred_capabilities
        .as_ref()
        .and_then(|caps| serde_json::to_string(caps).ok());

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
        conversion_result.fidelity.level.to_string(),
    );
    converted_pkg.detected_hooks = Some(hooks_json);
    converted_pkg.inferred_caps_json = inferred_caps_json;
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
    let ConvertedCcsInstallOptions {
        ccs_path,
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        no_scripts,
        allow_downgrade,
        dep_mode,
        yes,
    } = opts;

    if !no_deps {
        let conn = open_db(db_path)?;
        let ccs_pkg =
            CcsPackage::parse(ccs_path).context("Failed to parse converted CCS package")?;
        let missing: Vec<MissingDependency> = ccs_pkg
            .dependencies()
            .iter()
            .filter(|dep| !package_self_provides(&ccs_pkg, &dep.name))
            // Skip RPM-internal capabilities and filesystem deps.
            // TODO: remove after full migration -- use scheme-aware dependency
            // classification from `dependency_model` instead of string prefixes.
            .filter(|dep| !dep.name.starts_with("rpmlib(") && !dep.name.starts_with('/'))
            .filter(|dep| !is_conditional_rpm_dependency(&dep.name))
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

            // Use policy-aware resolution so the convergence intent provides a
            // default dep-mode when the user has not explicitly set one.
            let convergence_intent = if conary_core::model::model_exists(None) {
                conary_core::model::load_model(None)
                    .ok()
                    .map(|m| m.system.convergence.clone())
                    .unwrap_or_default()
            } else {
                conary_core::model::ConvergenceIntent::default()
            };
            let mut dep_plan = dep_resolution::resolve_missing_deps_policy_aware(
                &conn,
                &unresolved_missing,
                Some(dep_mode),
                &convergence_intent,
            );
            if matches!(dep_mode, DepMode::Satisfy) {
                promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);
            }

            if !dep_plan.to_adopt.is_empty() && !dry_run {
                crate::commands::adopt::cmd_adopt(&dep_plan.to_adopt, db_path, false).await?;
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
                        let downloaded = repository::download_dependencies(
                            &to_download,
                            temp_dir.path(),
                            Some(&keyring_dir),
                        )?;
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
                                // Break recursion: sub-deps are system packages
                                // handled by the system PM in satisfy mode, or
                                // already resolved at the parent level.  Allowing
                                // unbounded recursive Remi downloads would create
                                // an exponential chain of server-side conversions.
                                Box::pin(install_converted_ccs(ConvertedCcsInstallOptions {
                                    ccs_path: dep_ccs_path,
                                    db_path,
                                    root,
                                    dry_run,
                                    sandbox_mode,
                                    no_deps: true,
                                    no_scripts,
                                    allow_downgrade,
                                    dep_mode,
                                    yes,
                                }))
                                .await
                                .with_context(|| {
                                    format!("Failed to install CCS dependency {}", dep_name)
                                })?;
                                continue;
                            }

                            let reason = format!("Required by {}", parent_name);
                            match prepare_package_for_batch(
                                dep_path,
                                db_path,
                                &reason,
                                allow_downgrade,
                            ) {
                                Ok(prepared) => prepared_packages.push(prepared),
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
                            let installer =
                                BatchInstaller::new(db_path, root, sandbox_mode, no_scripts);
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
                        dep_mode,
                        convergence_intent.display_name(),
                        detail_lines.join("\n"),
                    ));
                }
            }
        }
    }

    println!("Installing converted CCS package...");
    super::super::ccs::cmd_ccs_install(
        ccs_path,
        db_path,
        root,
        dry_run,
        true, // allow_unsigned - converted packages aren't signed yet
        None, // policy
        None, // components - install all
        sandbox_mode,
        true,  // deps already handled above; skip redundant check in cmd_ccs_install
        false, // reinstall - not applicable for conversions
        false, // allow_capabilities - not applicable for conversions
        None,  // capability_policy - use default
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};
    use conary_core::db::schema;
    use conary_core::version::VersionConstraint;

    fn test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn detects_conditional_rpm_dependencies() {
        assert!(is_conditional_rpm_dependency(
            "((kernel-modules-extra-uname-r = 6.19.6-200.fc43.x86_64) if kernel-modules-extra-matched)"
        ));
        assert!(!is_conditional_rpm_dependency("kernel-core-uname-r"));
    }

    #[test]
    fn promote_only_repo_resolvable_deps() {
        let conn = test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        // coreutils-common exists in the repo (direct name match)
        let mut pkg = RepositoryPackage::new(
            repo_id,
            "coreutils-common".to_string(),
            "9.7-8.fc43".to_string(),
            "sha256:cc".to_string(),
            100,
            "https://example.invalid/coreutils-common.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();

        // kernel-core exists and provides kernel-core-uname-r via normalized table
        let mut kpkg = RepositoryPackage::new(
            repo_id,
            "kernel-core".to_string(),
            "6.19.6-200.fc43".to_string(),
            "sha256:kc".to_string(),
            200,
            "https://example.invalid/kernel-core.rpm".to_string(),
        );
        kpkg.insert(&conn).unwrap();
        let kpkg_id = kpkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            kpkg_id,
            "kernel-core-uname-r".to_string(),
            Some("6.19.6-200.fc43.x86_64".to_string()),
            "package".to_string(),
            Some("kernel-core-uname-r = 6.19.6-200.fc43.x86_64".to_string()),
        );
        provide.insert(&conn).unwrap();

        let mut dep_plan = dep_resolution::DepResolutionPlan {
            unresolvable: vec![
                conary_core::resolver::MissingDependency {
                    name: "coreutils-common".to_string(),
                    constraint: VersionConstraint::Any,
                    required_by: vec!["kernel".to_string()],
                },
                conary_core::resolver::MissingDependency {
                    name: "kernel-core-uname-r".to_string(),
                    constraint: VersionConstraint::parse("= 6.19.6-200.fc43.x86_64").unwrap(),
                    required_by: vec!["kernel".to_string()],
                },
                conary_core::resolver::MissingDependency {
                    name: "nonexistent-fantasy-pkg".to_string(),
                    constraint: VersionConstraint::Any,
                    required_by: vec!["kernel".to_string()],
                },
            ],
            ..Default::default()
        };

        promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

        // Two deps should have been promoted
        assert_eq!(dep_plan.to_install.len(), 2, "expected 2 promoted deps");
        let promoted_names: Vec<&str> = dep_plan
            .to_install
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(promoted_names.contains(&"coreutils-common"));
        assert!(promoted_names.contains(&"kernel-core-uname-r"));

        // The nonexistent dep should remain unresolvable
        assert_eq!(
            dep_plan.unresolvable.len(),
            1,
            "expected 1 still-unresolvable"
        );
        assert_eq!(dep_plan.unresolvable[0].name, "nonexistent-fantasy-pkg");
    }

    #[test]
    fn promote_skips_when_all_unresolvable() {
        let conn = test_db();

        let mut dep_plan = dep_resolution::DepResolutionPlan {
            unresolvable: vec![conary_core::resolver::MissingDependency {
                name: "nonexistent-pkg".to_string(),
                constraint: VersionConstraint::Any,
                required_by: vec!["test".to_string()],
            }],
            ..Default::default()
        };

        promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

        assert!(dep_plan.to_install.is_empty());
        assert_eq!(dep_plan.unresolvable.len(), 1);
        assert_eq!(dep_plan.unresolvable[0].name, "nonexistent-pkg");
    }
}
