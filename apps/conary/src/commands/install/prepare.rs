// src/commands/install/prepare.rs
//! Package parsing and pre-installation validation

use super::PackageFormatType;
use anyhow::{Context, Result};
use conary_core::components::ComponentType;
use conary_core::db::models::Trove;
use conary_core::packages::PackageFormat;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::rpm::RpmPackage;
use conary_core::repository::versioning::{VersionScheme, compare_mixed_repo_versions};
use rusqlite::Connection;
use std::cmp::Ordering;
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
    Upgrade(Box<conary_core::db::models::Trove>),
    /// Downgrade to an older version (when --allow-downgrade is used)
    Downgrade(Box<conary_core::db::models::Trove>),
}

/// Check if package is already installed and determine upgrade status
pub fn check_upgrade_status(
    conn: &Connection,
    pkg: &dyn PackageFormat,
    format: PackageFormatType,
    allow_downgrade: bool,
) -> Result<UpgradeCheck> {
    let existing = conary_core::db::models::Trove::find_by_name(conn, pkg.name())?;

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

            match compare_installed_and_incoming_versions(trove, pkg.version(), format) {
                Some(Ordering::Less) => {
                    info!(
                        "Upgrading {} from version {} to {}",
                        pkg.name(),
                        trove.version,
                        pkg.version()
                    );
                    return Ok(UpgradeCheck::Upgrade(Box::new(trove.clone())));
                }
                Some(Ordering::Equal | Ordering::Greater) => {
                    if allow_downgrade {
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
                None => warn!(
                    "Could not compare versions {} and {}",
                    trove.version,
                    pkg.version()
                ),
            }
        }
    }

    Ok(UpgradeCheck::FreshInstall)
}

fn compare_installed_and_incoming_versions(
    trove: &Trove,
    incoming_version: &str,
    incoming_format: PackageFormatType,
) -> Option<Ordering> {
    compare_mixed_repo_versions(
        installed_version_scheme(trove),
        &trove.version,
        version_scheme_for_format(incoming_format),
        incoming_version,
    )
}

fn installed_version_scheme(trove: &Trove) -> VersionScheme {
    trove
        .version_scheme
        .as_deref()
        .and_then(parse_version_scheme)
        .unwrap_or(VersionScheme::Rpm)
}

fn version_scheme_for_format(format: PackageFormatType) -> VersionScheme {
    match format {
        PackageFormatType::Rpm => VersionScheme::Rpm,
        PackageFormatType::Deb => VersionScheme::Debian,
        PackageFormatType::Arch => VersionScheme::Arch,
    }
}

fn parse_version_scheme(raw: &str) -> Option<VersionScheme> {
    match raw {
        "rpm" => Some(VersionScheme::Rpm),
        "debian" => Some(VersionScheme::Debian),
        "arch" => Some(VersionScheme::Arch),
        _ => None,
    }
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
            Self::Specific(types) => types
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, Trove, TroveType};
    use conary_core::db::schema;
    use conary_core::packages::traits::{
        ConfigFileInfo, Dependency, ExtractedFile, PackageFile, Scriptlet,
    };

    struct TestPackage {
        name: String,
        version: String,
        architecture: Option<String>,
    }

    impl conary_core::packages::PackageFormat for TestPackage {
        fn parse(_path: &str) -> conary_core::Result<Self>
        where
            Self: Sized,
        {
            unreachable!("tests construct package instances directly")
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn version(&self) -> &str {
            &self.version
        }

        fn architecture(&self) -> Option<&str> {
            self.architecture.as_deref()
        }

        fn description(&self) -> Option<&str> {
            None
        }

        fn files(&self) -> &[PackageFile] {
            &[]
        }

        fn dependencies(&self) -> &[Dependency] {
            &[]
        }

        fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> {
            Ok(Vec::new())
        }

        fn scriptlets(&self) -> &[Scriptlet] {
            &[]
        }

        fn config_files(&self) -> &[ConfigFileInfo] {
            &[]
        }

        fn to_trove(&self) -> Trove {
            let mut trove = Trove::new_with_source(
                self.name.clone(),
                self.version.clone(),
                TroveType::Package,
                InstallSource::Repository,
            );
            trove.architecture = self.architecture.clone();
            trove
        }
    }

    fn create_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn check_upgrade_status_uses_debian_version_scheme() {
        let conn = create_test_db();
        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0~beta1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.architecture = Some("amd64".to_string());
        trove.version_scheme = Some("debian".to_string());
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            architecture: Some("amd64".to_string()),
        };

        let result = check_upgrade_status(&conn, &pkg, PackageFormatType::Deb, false).unwrap();
        assert!(matches!(result, UpgradeCheck::Upgrade(_)));
    }

    #[test]
    fn check_upgrade_status_uses_arch_version_scheme() {
        let conn = create_test_db();
        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.version_scheme = Some("arch".to_string());
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0-2".to_string(),
            architecture: Some("x86_64".to_string()),
        };

        let result = check_upgrade_status(&conn, &pkg, PackageFormatType::Arch, false).unwrap();
        assert!(matches!(result, UpgradeCheck::Upgrade(_)));
    }
}
