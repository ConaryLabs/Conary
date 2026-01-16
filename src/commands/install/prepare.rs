// src/commands/install/prepare.rs
//! Package parsing and pre-installation validation

use super::PackageFormatType;
use anyhow::{Context, Result};
use conary::components::ComponentType;
use conary::packages::arch::ArchPackage;
use conary::packages::deb::DebPackage;
use conary::packages::rpm::RpmPackage;
use conary::packages::PackageFormat;
use conary::version::RpmVersion;
use rusqlite::Connection;
use std::path::Path;
use tracing::{info, warn};

/// Parse a package file and return the appropriate parser
pub fn parse_package(path: &Path, format: PackageFormatType) -> Result<Box<dyn PackageFormat>> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    let pkg: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(
            RpmPackage::parse(path_str)
                .with_context(|| format!("Failed to parse RPM package '{}'", path_str))?,
        ),
        PackageFormatType::Deb => Box::new(
            DebPackage::parse(path_str)
                .with_context(|| format!("Failed to parse DEB package '{}'", path_str))?,
        ),
        PackageFormatType::Arch => Box::new(
            ArchPackage::parse(path_str)
                .with_context(|| format!("Failed to parse Arch package '{}'", path_str))?,
        ),
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        pkg.name(),
        pkg.version(),
        pkg.files().len(),
        pkg.dependencies().len()
    );

    Ok(pkg)
}

/// Result of checking for existing package installation
pub enum UpgradeCheck {
    /// Fresh install - no existing package
    FreshInstall,
    /// Upgrade from an older version (boxed to reduce enum size)
    Upgrade(Box<conary::db::models::Trove>),
    /// Downgrade to an older version (when --allow-downgrade is used)
    Downgrade(Box<conary::db::models::Trove>),
}

/// Check if package is already installed and determine upgrade status
pub fn check_upgrade_status(
    conn: &Connection,
    pkg: &dyn PackageFormat,
    allow_downgrade: bool,
) -> Result<UpgradeCheck> {
    let existing = conary::db::models::Trove::find_by_name(conn, pkg.name())?;

    for trove in &existing {
        if trove.architecture == pkg.architecture().map(|s: &str| s.to_string()) {
            if trove.version == pkg.version() {
                return Err(anyhow::anyhow!(
                    "Package {} version {} ({}) is already installed",
                    pkg.name(),
                    pkg.version(),
                    pkg.architecture().unwrap_or("no-arch")
                ));
            }

            match (
                RpmVersion::parse(&trove.version),
                RpmVersion::parse(pkg.version()),
            ) {
                (Ok(existing_ver), Ok(new_ver)) => {
                    if new_ver > existing_ver {
                        info!(
                            "Upgrading {} from version {} to {}",
                            pkg.name(),
                            trove.version,
                            pkg.version()
                        );
                        return Ok(UpgradeCheck::Upgrade(Box::new(trove.clone())));
                    } else if allow_downgrade {
                        warn!(
                            "Downgrading {} from version {} to {}",
                            pkg.name(),
                            trove.version,
                            pkg.version()
                        );
                        return Ok(UpgradeCheck::Downgrade(Box::new(trove.clone())));
                    } else {
                        return Err(anyhow::anyhow!(
                            "Cannot downgrade package {} from version {} to {} (use --allow-downgrade to override)",
                            pkg.name(),
                            trove.version,
                            pkg.version()
                        ));
                    }
                }
                _ => warn!(
                    "Could not compare versions {} and {}",
                    trove.version,
                    pkg.version()
                ),
            }
        }
    }

    Ok(UpgradeCheck::FreshInstall)
}

/// Represents which components to install
#[derive(Debug, Clone)]
pub enum ComponentSelection {
    /// Install only default components (runtime, lib, config)
    Defaults,
    /// Install all components
    All,
    /// Install specific component(s)
    Specific(Vec<ComponentType>),
}

impl ComponentSelection {
    /// Check if a component type should be installed
    pub fn should_install(&self, comp_type: ComponentType) -> bool {
        match self {
            Self::All => true,
            Self::Defaults => comp_type.is_default(),
            Self::Specific(types) => types.contains(&comp_type),
        }
    }

    /// Get a display string for the selection
    pub fn display(&self) -> String {
        match self {
            Self::All => "all".to_string(),
            Self::Defaults => "defaults (runtime, lib, config)".to_string(),
            Self::Specific(types) => types.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "),
        }
    }
}
