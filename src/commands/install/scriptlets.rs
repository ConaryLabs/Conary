// src/commands/install/scriptlets.rs

//! Scriptlet execution during package installation
//!
//! Handles pre-install, post-install, and upgrade scriptlet execution
//! with proper handling for different package formats (RPM, DEB, Arch).

use super::PackageFormatType;
use anyhow::Result;
use conary::components::ComponentType;
use conary::db::models::ScriptletEntry;
use conary::packages::traits::{Scriptlet, ScriptletPhase};
use conary::scriptlet::{ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor};
use rusqlite::Connection;
use std::path::Path;
use tracing::{info, warn};

/// Determine the scriptlet package format from install format type
pub fn to_scriptlet_format(format: PackageFormatType) -> ScriptletPackageFormat {
    match format {
        PackageFormatType::Rpm => ScriptletPackageFormat::Rpm,
        PackageFormatType::Deb => ScriptletPackageFormat::Deb,
        PackageFormatType::Arch => ScriptletPackageFormat::Arch,
    }
}

/// Build the execution mode based on upgrade status
pub fn build_execution_mode(old_version: Option<&str>) -> ExecutionMode {
    match old_version {
        Some(ver) => ExecutionMode::Upgrade {
            old_version: ver.to_string(),
        },
        None => ExecutionMode::Install,
    }
}

/// Execute pre-install scriptlet for a package
///
/// For Arch packages during upgrade, uses PreUpgrade phase.
/// For RPM/DEB, always uses PreInstall (they distinguish via $1 argument).
pub fn run_pre_install(
    root: &Path,
    pkg_name: &str,
    pkg_version: &str,
    scriptlets: &[Scriptlet],
    format: ScriptletPackageFormat,
    execution_mode: &ExecutionMode,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    let executor = ScriptletExecutor::new(root, pkg_name, pkg_version, format)
        .with_sandbox_mode(sandbox_mode);

    // For Arch packages during upgrade, use PreUpgrade; for RPM/DEB always use PreInstall
    let pre_phase = if format == ScriptletPackageFormat::Arch
        && matches!(execution_mode, ExecutionMode::Upgrade { .. })
    {
        ScriptletPhase::PreUpgrade
    } else {
        ScriptletPhase::PreInstall
    };

    // For Arch: if pre_upgrade is missing, do nothing (it's intentional)
    // For RPM/DEB: pre_install handles both cases via $1 argument
    if let Some(pre) = scriptlets.iter().find(|s| s.phase == pre_phase) {
        info!("Running {} scriptlet...", pre.phase);
        executor.execute(pre, execution_mode)?;
    }

    Ok(())
}

/// Execute post-install scriptlet for a package
///
/// For Arch packages during upgrade, uses PostUpgrade phase.
/// For RPM/DEB, always uses PostInstall (they distinguish via $1 argument).
///
/// Post-install failures are logged as warnings but don't fail the install
/// since files are already deployed.
pub fn run_post_install(
    root: &Path,
    pkg_name: &str,
    pkg_version: &str,
    scriptlets: &[Scriptlet],
    format: ScriptletPackageFormat,
    execution_mode: &ExecutionMode,
    sandbox_mode: SandboxMode,
) {
    let executor = ScriptletExecutor::new(root, pkg_name, pkg_version, format)
        .with_sandbox_mode(sandbox_mode);

    // For Arch packages during upgrade, use PostUpgrade; for RPM/DEB always use PostInstall
    let post_phase = if format == ScriptletPackageFormat::Arch
        && matches!(execution_mode, ExecutionMode::Upgrade { .. })
    {
        ScriptletPhase::PostUpgrade
    } else {
        ScriptletPhase::PostInstall
    };

    // For Arch: if post_upgrade is missing, do nothing (it's intentional)
    // For RPM/DEB: post_install handles both cases via $1 argument
    if let Some(post) = scriptlets.iter().find(|s| s.phase == post_phase) {
        info!("Running {} scriptlet...", post.phase);
        if let Err(e) = executor.execute(post, execution_mode) {
            // Post-install failure is serious but files are already deployed
            warn!("{} scriptlet failed: {}. Package files are installed.", post.phase, e);
            eprintln!("WARNING: {} scriptlet failed: {}", post.phase, e);
        }
    }
}

/// Query scriptlets for an existing package (for upgrade scenarios)
pub fn get_old_package_scriptlets(
    conn: &Connection,
    old_trove_id: Option<i64>,
) -> Result<Vec<ScriptletEntry>> {
    match old_trove_id {
        Some(id) => ScriptletEntry::find_by_trove(conn, id)
            .map_err(|e| anyhow::anyhow!("Failed to query scriptlets: {}", e)),
        None => Ok(Vec::new()),
    }
}

/// Execute pre-remove scriptlet for old package during upgrade
///
/// Only runs for RPM/DEB formats - Arch does NOT run removal scripts during upgrade.
pub fn run_old_pre_remove(
    root: &Path,
    old_name: &str,
    old_version: &str,
    new_version: &str,
    old_scriptlets: &[ScriptletEntry],
    format: ScriptletPackageFormat,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    // Arch does NOT run removal scripts during upgrade
    if format == ScriptletPackageFormat::Arch || old_scriptlets.is_empty() {
        return Ok(());
    }

    let executor = ScriptletExecutor::new(root, old_name, old_version, format)
        .with_sandbox_mode(sandbox_mode);

    let upgrade_removal_mode = ExecutionMode::UpgradeRemoval {
        new_version: new_version.to_string(),
    };

    if let Some(pre_remove) = old_scriptlets.iter().find(|s| s.phase == "pre-remove") {
        info!("Running old package pre-remove scriptlet (upgrade)...");
        executor.execute_entry(pre_remove, &upgrade_removal_mode)?;
    }

    Ok(())
}

/// Execute post-remove scriptlet for old package during upgrade
///
/// Only runs for RPM/DEB formats - Arch does NOT run removal scripts during upgrade.
/// Post-remove failures during upgrade are not fatal since files are already replaced.
pub fn run_old_post_remove(
    root: &Path,
    old_name: &str,
    old_version: &str,
    new_version: &str,
    old_scriptlets: &[ScriptletEntry],
    format: ScriptletPackageFormat,
    sandbox_mode: SandboxMode,
) {
    // Arch does NOT run removal scripts during upgrade
    if format == ScriptletPackageFormat::Arch || old_scriptlets.is_empty() {
        return;
    }

    let executor = ScriptletExecutor::new(root, old_name, old_version, format)
        .with_sandbox_mode(sandbox_mode);

    let upgrade_removal_mode = ExecutionMode::UpgradeRemoval {
        new_version: new_version.to_string(),
    };

    if let Some(post_remove) = old_scriptlets.iter().find(|s| s.phase == "post-remove") {
        info!("Running old package post-remove scriptlet (upgrade)...");
        // Post-remove failure during upgrade is not fatal - files are already replaced
        if let Err(e) = executor.execute_entry(post_remove, &upgrade_removal_mode) {
            warn!("Old package post-remove scriptlet failed: {}. Continuing anyway.", e);
            eprintln!("WARNING: Old package post-remove scriptlet failed: {}", e);
        }
    }
}

/// Check if scriptlets should run based on installed components
///
/// Scriptlets only run when :runtime or :lib component is being installed.
#[allow(dead_code)]
pub fn should_run(installed_components: &[ComponentType]) -> bool {
    conary::components::should_run_scriptlets(installed_components)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_scriptlet_format() {
        assert_eq!(to_scriptlet_format(PackageFormatType::Rpm), ScriptletPackageFormat::Rpm);
        assert_eq!(to_scriptlet_format(PackageFormatType::Deb), ScriptletPackageFormat::Deb);
        assert_eq!(to_scriptlet_format(PackageFormatType::Arch), ScriptletPackageFormat::Arch);
    }

    #[test]
    fn test_build_execution_mode() {
        match build_execution_mode(None) {
            ExecutionMode::Install => {}
            _ => panic!("Expected Install mode"),
        }

        match build_execution_mode(Some("1.0.0")) {
            ExecutionMode::Upgrade { old_version } => {
                assert_eq!(old_version, "1.0.0");
            }
            _ => panic!("Expected Upgrade mode"),
        }
    }
}
