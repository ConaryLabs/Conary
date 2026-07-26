// src/commands/install/prepare.rs
//! Package parsing and pre-installation validation

use super::{InstallIntent, InstallSemantics};
use crate::commands::PackageFormatType;
use anyhow::{Context, Result};
use conary_core::components::ComponentType;
use conary_core::db::models::Trove;
use conary_core::packages::PackageFormat;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::rpm::RpmPackage;
use conary_core::repository::selector::package_architectures_match;
use conary_core::repository::versioning::{VersionScheme, compare_package_identities};
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
        pkg.requirements().len()
    );

    Ok(pkg)
}

/// Result of checking for existing package installation
pub enum UpgradeCheck {
    /// Fresh install - no existing package
    FreshInstall,
    /// The exact package identity is already installed.
    AlreadyInstalled(Box<conary_core::db::models::Trove>),
    /// Upgrade from an older version (boxed to reduce enum size)
    Upgrade(Box<conary_core::db::models::Trove>),
    /// Downgrade to an older version (when --allow-downgrade is used)
    Downgrade(Box<conary_core::db::models::Trove>),
    /// Explicit replacement across package-manager version schemes.
    Replatform(Box<conary_core::db::models::Trove>),
}

/// Check if package is already installed and determine upgrade status
pub fn check_upgrade_status(
    conn: &Connection,
    pkg: &dyn PackageFormat,
    semantics: &InstallSemantics,
    allow_downgrade: bool,
    intent: InstallIntent,
) -> Result<UpgradeCheck> {
    let existing = conary_core::db::models::Trove::find_by_name(conn, pkg.name())?;

    for trove in &existing {
        if architectures_share_install_slot(
            trove.version_scheme,
            trove.architecture.as_deref(),
            pkg.version_scheme(),
            pkg.architecture(),
        ) {
            if trove.version == pkg.version()
                && trove.package_release.as_deref() == pkg.package_release()
            {
                return Ok(UpgradeCheck::AlreadyInstalled(Box::new(trove.clone())));
            }

            if trove.version_scheme != semantics.version_scheme {
                if intent == InstallIntent::Replatform {
                    info!(
                        "Replatforming {} from {} version scheme {} to {} version scheme {}",
                        pkg.name(),
                        trove.version_scheme.as_str(),
                        trove.version,
                        semantics.version_scheme.as_str(),
                        pkg.version()
                    );
                    return Ok(UpgradeCheck::Replatform(Box::new(trove.clone())));
                }
                return Err(anyhow::anyhow!(
                    "Cannot replace package {} across {} and {} version schemes without an explicit replatform operation",
                    pkg.name(),
                    trove.version_scheme.as_str(),
                    semantics.version_scheme.as_str()
                ));
            }

            match compare_installed_and_incoming_versions(
                trove,
                pkg.version(),
                pkg.package_release(),
                semantics,
            )? {
                Ordering::Less => {
                    info!(
                        "Upgrading {} from version {} to {}",
                        pkg.name(),
                        trove.version,
                        pkg.version()
                    );
                    return Ok(UpgradeCheck::Upgrade(Box::new(trove.clone())));
                }
                Ordering::Equal | Ordering::Greater => {
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
            }
        }
    }

    Ok(UpgradeCheck::FreshInstall)
}

fn architectures_share_install_slot(
    installed_scheme: VersionScheme,
    installed: Option<&str>,
    incoming_scheme: VersionScheme,
    incoming: Option<&str>,
) -> bool {
    match (installed, incoming) {
        (Some(installed), Some(incoming)) => package_architectures_match(
            installed_scheme,
            installed,
            incoming_scheme,
            incoming,
            &conary_core::repository::registry::detect_system_arch(),
        ),
        _ => false,
    }
}

fn compare_installed_and_incoming_versions(
    trove: &Trove,
    incoming_version: &str,
    incoming_release: Option<&str>,
    semantics: &InstallSemantics,
) -> Result<Ordering> {
    Ok(compare_package_identities(
        trove.version_scheme,
        &trove.version,
        trove.package_release.as_deref(),
        semantics.version_scheme,
        incoming_version,
        incoming_release,
    )?)
}

pub(super) fn version_scheme_for_format(format: PackageFormatType) -> VersionScheme {
    match format {
        PackageFormatType::Rpm => VersionScheme::Rpm,
        PackageFormatType::Deb => VersionScheme::Debian,
        PackageFormatType::Arch => VersionScheme::Arch,
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
    use conary_core::packages::traits::{ConfigFileInfo, PackageFile};

    struct TestPackage {
        name: String,
        version: String,
        version_scheme: VersionScheme,
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

        fn version_scheme(&self) -> conary_core::repository::versioning::VersionScheme {
            self.version_scheme
        }

        fn architecture(&self) -> Option<&str> {
            self.architecture.as_deref()
        }

        fn debian_multi_arch(
            &self,
        ) -> Option<conary_core::repository::dependency_model::DebianMultiArch> {
            (self.version_scheme == VersionScheme::Debian)
                .then_some(conary_core::repository::dependency_model::DebianMultiArch::No)
        }

        fn description(&self) -> Option<&str> {
            None
        }

        fn files(&self) -> &[PackageFile] {
            &[]
        }

        fn requirements(
            &self,
        ) -> &[conary_core::repository::dependency_model::RepositoryRequirementGroup] {
            &[]
        }

        fn package_payload(&self) -> conary_core::Result<conary_core::packages::PackagePayload> {
            Ok(conary_core::packages::PackagePayload::default())
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
                conary_core::repository::versioning::VersionScheme::Conary,
            );
            trove.architecture = self.architecture.clone();
            trove
        }
    }

    fn create_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
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
            conary_core::repository::versioning::VersionScheme::Debian,
        );
        trove.architecture = Some("amd64".to_string());
        trove.debian_multi_arch =
            Some(conary_core::repository::dependency_model::DebianMultiArch::No);
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            version_scheme: VersionScheme::Debian,
            architecture: Some("amd64".to_string()),
        };

        let result = check_upgrade_status(
            &conn,
            &pkg,
            &InstallSemantics::native_package(PackageFormatType::Deb),
            false,
            InstallIntent::PackageChange,
        )
        .unwrap();
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
            conary_core::repository::versioning::VersionScheme::Arch,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0-2".to_string(),
            version_scheme: VersionScheme::Arch,
            architecture: Some("x86_64".to_string()),
        };

        let result = check_upgrade_status(
            &conn,
            &pkg,
            &InstallSemantics::native_package(PackageFormatType::Arch),
            false,
            InstallIntent::PackageChange,
        )
        .unwrap();
        assert!(matches!(result, UpgradeCheck::Upgrade(_)));
    }

    #[test]
    fn check_upgrade_status_returns_typed_already_installed_outcome() {
        let conn = create_test_db();
        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Arch,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0-1".to_string(),
            version_scheme: VersionScheme::Arch,
            architecture: Some("x86_64".to_string()),
        };

        let result = check_upgrade_status(
            &conn,
            &pkg,
            &InstallSemantics::native_package(PackageFormatType::Arch),
            false,
            InstallIntent::PackageChange,
        )
        .unwrap();
        assert!(matches!(result, UpgradeCheck::AlreadyInstalled(_)));
    }

    #[test]
    fn check_upgrade_status_matches_cross_distro_architecture_aliases() {
        let conn = create_test_db();
        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Arch,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0-1".to_string(),
            version_scheme: VersionScheme::Debian,
            architecture: Some("amd64".to_string()),
        };

        let result = check_upgrade_status(
            &conn,
            &pkg,
            &InstallSemantics::native_package(PackageFormatType::Deb),
            false,
            InstallIntent::PackageChange,
        )
        .unwrap();
        assert!(matches!(result, UpgradeCheck::AlreadyInstalled(_)));
    }

    #[test]
    fn check_upgrade_status_matches_architecture_independent_markers() {
        let conn = create_test_db();
        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        trove.architecture = Some("noarch".to_string());
        trove.insert(&conn).unwrap();

        let pkg = TestPackage {
            name: "demo".to_string(),
            version: "1.0-1".to_string(),
            version_scheme: VersionScheme::Debian,
            architecture: Some("all".to_string()),
        };

        let result = check_upgrade_status(
            &conn,
            &pkg,
            &InstallSemantics::native_package(PackageFormatType::Deb),
            false,
            InstallIntent::PackageChange,
        )
        .unwrap();
        assert!(matches!(result, UpgradeCheck::AlreadyInstalled(_)));
    }
}
