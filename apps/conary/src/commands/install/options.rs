// src/commands/install/options.rs

use super::{DepMode, LegacyReplayOptions};
use anyhow::Result;
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::repository::versioning::{VersionScheme, resolve_package_version_scheme};
use conary_core::scriptlet::SandboxMode;

/// Options for package installation
#[derive(Debug, Clone, Default)]
pub struct InstallOptions<'a> {
    /// Path to the package database
    pub db_path: &'a str,
    /// Filesystem root for installation
    pub root: &'a str,
    /// Specific version to install
    pub version: Option<String>,
    /// Specific repository to use
    pub repo: Option<String>,
    /// Preferred architecture to resolve/install
    pub architecture: Option<String>,
    /// Preview without installing
    pub dry_run: bool,
    /// Skip dependency resolution
    pub no_deps: bool,
    /// Skip scriptlet execution
    pub no_scripts: bool,
    /// Human-readable reason for installation
    pub selection_reason: Option<&'a str>,
    /// Sandbox mode for scriptlet execution
    pub sandbox_mode: SandboxMode,
    /// Allow installing older versions
    pub allow_downgrade: bool,
    /// Convert legacy packages to CCS format
    pub convert_to_ccs: bool,
    /// Skip the automatic state snapshot that is normally captured after
    /// a successful install.  Named `no_capture` for CLI consistency with
    /// `--no-capture`; equivalent to "skip scriptlet output capture" in
    /// some package managers but here it controls state snapshots.
    pub no_capture: bool,
    /// Force install/reinstall checks, but not adopted-package ownership
    pub force: bool,
    /// Dependency handling mode: satisfy, adopt, takeover.
    /// `None` means the user did not explicitly set `--dep-mode`, so the
    /// policy-aware resolver uses the system model convergence intent.
    pub dep_mode: Option<DepMode>,
    /// Skip confirmation prompts
    pub yes: bool,
    /// Install from a specific distro (cross-distro canonical resolution)
    pub from_distro: Option<String>,
    /// Repository provenance supplied by an internal caller that already
    /// selected and downloaded the package before calling `cmd_install`.
    pub(crate) repository_provenance: Option<RepositoryInstallProvenance>,
    /// Raw legacy scriptlet replay admission flags. Defaults fail closed.
    pub legacy_replay: LegacyReplayOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositoryInstallProvenance {
    pub repository_id: i64,
    pub source_distro: Option<String>,
    pub version_scheme: Option<String>,
}

pub(crate) fn repository_install_provenance_from_package(
    package: &RepositoryPackage,
    repository: &Repository,
) -> Result<RepositoryInstallProvenance> {
    let repository_id = repository
        .id
        .ok_or_else(|| anyhow::anyhow!("Selected repository has no database ID"))?;
    let source_distro = package
        .distro
        .clone()
        .or_else(|| repository.default_strategy_distro.clone());
    let version_scheme =
        resolve_package_version_scheme(package, repository).map(|scheme| match scheme {
            VersionScheme::Rpm => "rpm".to_string(),
            VersionScheme::Debian => "debian".to_string(),
            VersionScheme::Arch => "arch".to_string(),
        });

    Ok(RepositoryInstallProvenance {
        repository_id,
        source_distro,
        version_scheme,
    })
}
