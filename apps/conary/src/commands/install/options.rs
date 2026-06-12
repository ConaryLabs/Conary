// src/commands/install/options.rs

use super::{DepMode, LegacyReplayOptions};
use anyhow::{Context, Result};
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use conary_core::db::models::{Repository, RepositoryPackage, RepositoryPackageKey};
use conary_core::repository::RepositorySourceKind;
use conary_core::repository::versioning::{VersionScheme, resolve_package_version_scheme};
use conary_core::scriptlet::SandboxMode;
use std::path::Path;

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
    pub source_kind: RepositorySourceKind,
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
        source_kind: match repository.default_strategy.as_deref() {
            Some("static") => RepositorySourceKind::Static,
            Some("remi") => RepositorySourceKind::Remi,
            _ => RepositorySourceKind::Native,
        },
    })
}

pub(crate) fn verify_static_repository_ccs_package_if_needed(
    db_path: &str,
    ccs_path: &Path,
    repository_provenance: Option<&RepositoryInstallProvenance>,
) -> Result<()> {
    let Some(provenance) = repository_provenance
        .filter(|provenance| provenance.source_kind == RepositorySourceKind::Static)
    else {
        return Ok(());
    };

    let conn = crate::commands::open_db(db_path)?;
    let keys = RepositoryPackageKey::trusted_keys_for_repository(&conn, provenance.repository_id)
        .with_context(|| {
        format!(
            "load active static repository package keys for repository {}",
            provenance.repository_id
        )
    })?;
    let policy = TrustPolicy::strict(keys);
    let verification = verify_package(ccs_path, &policy).with_context(|| {
        format!(
            "Static repository package signature verification failed for {}",
            ccs_path.display()
        )
    })?;
    if !verification.valid {
        anyhow::bail!(
            "Static repository package signature verification failed for {}",
            ccs_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::repository::RepositorySourceKind;

    #[test]
    fn repository_install_provenance_from_package_tags_static_repository() {
        let mut repository = Repository::new(
            "static-repo".to_string(),
            "https://static.example.invalid/repo".to_string(),
        );
        repository.id = Some(88);
        repository.default_strategy = Some("static".to_string());
        repository.default_strategy_distro = Some("fedora".to_string());
        let mut package = RepositoryPackage::new(
            88,
            "tree".to_string(),
            "2.2.1-4.fc44".to_string(),
            "sha256:abc123".to_string(),
            1024,
            "https://static.example.invalid/tree.ccs".to_string(),
        );
        package.version_scheme = Some("rpm".to_string());

        let provenance = repository_install_provenance_from_package(&package, &repository).unwrap();

        assert_eq!(provenance.repository_id, 88);
        assert_eq!(provenance.source_distro.as_deref(), Some("fedora"));
        assert_eq!(provenance.version_scheme.as_deref(), Some("rpm"));
        assert_eq!(provenance.source_kind, RepositorySourceKind::Static);
    }
}
