// src/commands/install/refinery.rs
//! Refinery server integration for pre-converted CCS packages

use anyhow::{Context, Result};
use conary::db::models::ProvideEntry;
use conary::scriptlet::SandboxMode;
use rusqlite::Connection;
use tempfile::TempDir;
use tracing::debug;

/// Install a package from a Refinery server
///
/// This fetches pre-converted CCS packages from a Refinery, which converts
/// legacy packages (RPM/DEB/Arch) to CCS format on-demand.
#[allow(clippy::too_many_arguments)]
pub fn cmd_install_from_refinery(
    package: &str,
    refinery_url: &str,
    distro: Option<&str>,
    version: Option<&str>,
    db_path: &str,
    root: &str,
    dry_run: bool,
    no_deps: bool,
    _no_scripts: bool,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    use conary::repository::refinery::RefineryClient;

    let distro = distro.ok_or_else(|| {
        anyhow::anyhow!("--distro is required when using --refinery (arch, fedora, ubuntu, debian)")
    })?;

    println!("Fetching {} from Refinery: {}", package, refinery_url);

    // Create Refinery client
    let client = RefineryClient::new(refinery_url)
        .with_context(|| format!("Failed to connect to Refinery at {}", refinery_url))?;

    // Health check
    if !client.health_check()? {
        return Err(anyhow::anyhow!("Refinery at {} is not healthy", refinery_url));
    }

    // Create temp directory for CCS package
    let temp_dir = TempDir::new()
        .context("Failed to create temporary directory")?;

    // Fetch package (handles 202 polling automatically)
    println!("Requesting package conversion...");
    let ccs_path = client.fetch_package(distro, package, version, temp_dir.path())
        .with_context(|| format!("Failed to fetch {} from Refinery", package))?;

    println!("Downloaded CCS package: {}", ccs_path.display());

    if dry_run {
        println!("[dry-run] Would install CCS package: {}", ccs_path.display());
        return Ok(());
    }

    // Install the CCS package using ccs-install
    let ccs_path_str = ccs_path.to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid CCS package path"))?;

    // Use ccs_install command
    crate::commands::ccs::cmd_ccs_install(
        ccs_path_str,
        db_path,
        root,
        dry_run,
        true, // allow_unsigned (Refinery packages aren't signed yet)
        None, // policy
        None, // components
        sandbox_mode,
        no_deps,
    )
}

/// Check if missing dependencies are satisfied by packages in the provides table
///
/// This is a self-contained approach that doesn't query the host package manager.
/// Instead, it checks if any tracked package provides the required capability.
///
/// Returns a tuple of:
/// - satisfied: Vec of (dep_name, provider_name, version)
/// - unsatisfied: Vec of MissingDependency (cloned)
#[allow(clippy::type_complexity)]
pub fn check_provides_dependencies(
    conn: &Connection,
    missing: &[conary::resolver::MissingDependency],
) -> (
    Vec<(String, String, Option<String>)>,
    Vec<conary::resolver::MissingDependency>,
) {
    let mut satisfied = Vec::new();
    let mut unsatisfied = Vec::new();

    for dep in missing {
        // Check if this capability is provided by any tracked package (with fuzzy matching)
        match ProvideEntry::find_satisfying_provider_fuzzy(conn, &dep.name) {
            Ok(Some((provider, version))) => {
                satisfied.push((dep.name.clone(), provider, Some(version)));
            }
            Ok(None) => {
                unsatisfied.push(dep.clone());
            }
            Err(e) => {
                debug!("Error checking provides for {}: {}", dep.name, e);
                unsatisfied.push(dep.clone());
            }
        }
    }

    (satisfied, unsatisfied)
}
