// src/commands/install/validation.rs

use super::{ComponentSelection, DepMode, blocklist};
use anyhow::Result;
use conary_core::components::{ComponentType, parse_component_spec};
use conary_core::db::models::{InstallReason, Trove};
use conary_core::packages::SystemPackageManager;
use tracing::info;

/// Parse a component spec from the package argument and run pre-install
/// validation checks (blocklist, adoption).
///
/// Returns `(package_name, component_selection)`.
pub(super) fn parse_component_and_validate(
    conn: &rusqlite::Connection,
    package: &str,
    dep_mode: DepMode,
    force: bool,
) -> Result<(String, ComponentSelection)> {
    // Parse component spec from package argument (e.g., "nginx:devel" or "nginx:all")
    let (package_name, component_selection) = if let Some((pkg, comp)) =
        parse_component_spec(package)
    {
        let selection = if comp == "all" {
            ComponentSelection::All
        } else if let Some(comp_type) = ComponentType::parse(&comp) {
            ComponentSelection::Specific(vec![comp_type])
        } else {
            return Err(anyhow::anyhow!(
                "Unknown component '{}'. Valid components: runtime, lib, devel, doc, config, all",
                comp
            ));
        };
        (pkg, selection)
    } else {
        // No component spec - install defaults only
        (package.to_string(), ComponentSelection::Defaults)
    };

    info!(
        "Installing package: {} (components: {})",
        package_name,
        component_selection.display()
    );

    // Block installation of critical system packages in takeover mode
    if dep_mode == DepMode::Takeover && blocklist::is_blocked(&package_name) {
        return Err(anyhow::anyhow!(
            "Package '{}' is on the critical system blocklist and cannot be taken over. \
             These packages (glibc, systemd, etc.) must remain managed by the system package manager.",
            package_name
        ));
    }

    // Check if the package is adopted from the system PM. `--force` alone must
    // not silently convert native-manager ownership into Conary ownership.
    if let Some(existing) = Trove::find_one_by_name(conn, &package_name)?
        && existing.install_source.is_adopted()
    {
        if dep_mode == DepMode::Takeover {
            println!(
                "[INFO] Package '{}' is adopted -- proceeding with explicit --dep-mode takeover",
                package_name
            );
        } else {
            let pkg_mgr = SystemPackageManager::detect();
            let force_note = if force {
                " --force does not override adopted package ownership."
            } else {
                ""
            };
            return Err(anyhow::anyhow!(
                "Package '{}' is adopted from {}.{} Run 'conary system adopt --refresh' after native package-manager changes. Use 'conary install {} --dep-mode takeover' \
                 for explicit package takeover, or 'conary system takeover' for generation-level takeover.",
                package_name,
                pkg_mgr.display_name(),
                force_note,
                package_name
            ));
        }
    }

    Ok((package_name, component_selection))
}

/// Check if the package is already installed as a dependency and promote it
/// to explicit.  Returns `true` if no further work is needed (same version).
pub(super) fn try_promote_existing_dep(
    conn: &rusqlite::Connection,
    package_name: &str,
    version: Option<&str>,
    selection_reason: Option<&str>,
) -> Result<bool> {
    // Check if the package is already installed as a dependency - if so, promote it
    // This must happen before we try to download, as we may not need to do anything else
    if let Some(existing) = Trove::find_one_by_name(conn, package_name)?
        && existing.install_reason == InstallReason::Dependency
    {
        // Check if we're requesting a specific version that differs
        let needs_version_change = version.is_some_and(|v| v != existing.version);

        // Promote to explicit
        let reason = selection_reason.unwrap_or("Explicitly installed by user");
        Trove::promote_to_explicit(conn, package_name, Some(reason))?;
        println!("Promoted {} from dependency to explicit", package_name);

        // If same version (or no version specified), we're done
        if !needs_version_change {
            println!("{} {} is already installed", package_name, existing.version);
            return Ok(true);
        }
        // Otherwise continue with version upgrade
        info!(
            "Continuing with version change: {} -> {:?}",
            existing.version, version
        );
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_install_over_adopted_package_is_not_silent_takeover() {
        use crate::commands::test_helpers::create_test_db;
        use conary_core::db::models::{InstallSource, Trove, TroveType};

        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.insert(&conn).unwrap();

        let err = parse_component_and_validate(&conn, "curl", DepMode::Adopt, true).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("curl"));
        assert!(message.contains("--dep-mode takeover"));
        assert!(message.contains("conary system takeover"));
    }

    #[test]
    fn explicit_takeover_over_adopted_package_is_allowed() {
        use crate::commands::test_helpers::create_test_db;
        use conary_core::db::models::{InstallSource, Trove, TroveType};

        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.insert(&conn).unwrap();

        let (package_name, _component_selection) =
            parse_component_and_validate(&conn, "curl", DepMode::Takeover, false).unwrap();

        assert_eq!(package_name, "curl");
    }
}
