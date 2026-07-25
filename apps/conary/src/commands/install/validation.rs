// src/commands/install/validation.rs

use super::{ComponentSelection, OwnershipMode};
use anyhow::Result;
use conary_core::components::{ComponentType, parse_component_spec};
use conary_core::db::models::{InstallReason, Trove};
use tracing::info;

/// Parse a component spec from the package argument and run pre-install
/// validation checks for persisted ownership state.
///
/// Returns `(package_name, component_selection)`.
pub(super) fn parse_component_and_validate(
    conn: &rusqlite::Connection,
    package: &str,
    architecture: Option<&str>,
    ownership: OwnershipMode,
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

    // Check if the package is adopted from the system PM. Ownership transfer
    // is one explicit operation; there is no second force-style override.
    let installed_variants = Trove::find_by_name(conn, &package_name)?
        .into_iter()
        .filter(|trove| {
            architecture.is_none_or(|requested| trove.architecture.as_deref() == Some(requested))
        })
        .collect::<Vec<_>>();
    let has_adopted_variant = installed_variants
        .iter()
        .any(|trove| trove.install_source.is_adopted());
    if has_adopted_variant {
        if architecture.is_none()
            && installed_variants
                .iter()
                .any(|trove| !trove.install_source.is_adopted())
        {
            anyhow::bail!(
                "Package '{}' has both Conary-owned and adopted installed variants; select the exact variant with --arch before changing ownership",
                package_name
            );
        }
        if ownership == OwnershipMode::Takeover {
            crate::ui::note(&format!(
                "Package '{package_name}' is adopted -- proceeding with explicit --ownership takeover"
            ));
        } else {
            return Err(anyhow::anyhow!(
                "Package '{}' is adopted and its files are not Conary-owned. Run 'conary system adopt --refresh' after external package changes. Use 'conary install {} --ownership takeover' \
                 for explicit package takeover, or 'conary system takeover' for generation-level takeover.",
                package_name,
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
    architecture: Option<&str>,
    selection_reason: Option<&str>,
) -> Result<bool> {
    let mut candidates = Trove::find_by_name(conn, package_name)?
        .into_iter()
        .filter(|trove| version.is_none_or(|requested| trove.version == requested))
        .filter(|trove| {
            architecture.is_none_or(|requested| trove.architecture.as_deref() == Some(requested))
        })
        .collect::<Vec<_>>();

    if candidates.len() > 1 {
        anyhow::bail!(
            "Package '{}' has multiple installed dependency variants; select the exact variant with --version and --arch",
            package_name
        );
    }
    let Some(existing) = candidates.pop() else {
        return Ok(false);
    };
    if existing.install_reason != InstallReason::Dependency {
        return Ok(false);
    }
    let trove_id = existing.id.ok_or_else(|| {
        anyhow::anyhow!(
            "Installed package '{} {}' has no persisted identity",
            existing.name,
            existing.version
        )
    })?;

    let reason = selection_reason.unwrap_or("Explicitly installed by user");
    Trove::promote_to_explicit(conn, trove_id, Some(reason))?;
    println!("Promoted {} from dependency to explicit", package_name);
    println!("{} {} is already installed", package_name, existing.version);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserved_ownership_rejects_install_over_adopted_package() {
        use crate::commands::test_helpers::create_test_db;
        use conary_core::db::models::{InstallSource, Trove, TroveType};
        use conary_core::packages::InstalledPackageIdentity;

        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.0.0-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.native_package_identity = Some(
            InstalledPackageIdentity::rpm(
                "curl-8.0.0-1.x86_64",
                "curl",
                None,
                "8.0.0",
                "1",
                "x86_64",
            )
            .unwrap(),
        );
        trove.insert(&conn).unwrap();

        let err =
            parse_component_and_validate(&conn, "curl", None, OwnershipMode::Preserve).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("curl"));
        assert!(message.contains("--ownership takeover"));
        assert!(message.contains("conary system takeover"));
    }

    #[test]
    fn explicit_takeover_over_adopted_package_is_allowed() {
        use crate::commands::test_helpers::create_test_db;
        use conary_core::db::models::{InstallSource, Trove, TroveType};
        use conary_core::packages::InstalledPackageIdentity;

        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.0.0-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.native_package_identity = Some(
            InstalledPackageIdentity::rpm(
                "curl-8.0.0-1.x86_64",
                "curl",
                None,
                "8.0.0",
                "1",
                "x86_64",
            )
            .unwrap(),
        );
        trove.insert(&conn).unwrap();

        let (package_name, _component_selection) =
            parse_component_and_validate(&conn, "curl", None, OwnershipMode::Takeover).unwrap();

        assert_eq!(package_name, "curl");
    }
}
