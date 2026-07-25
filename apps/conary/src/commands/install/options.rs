// src/commands/install/options.rs

use super::OwnershipMode;
use anyhow::{Context, Result};
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use conary_core::db::models::{Repository, RepositoryPackage, RepositoryPackageKey};
use conary_core::repository::RepositorySourceKind;
use conary_core::repository::versioning::resolve_package_version_scheme;
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
    /// Human-readable reason for installation
    pub selection_reason: Option<&'a str>,
    /// Sandbox mode for scriptlet execution
    pub sandbox_mode: SandboxMode,
    /// Allow installing older versions
    pub allow_downgrade: bool,
    /// Convert native packages to CCS format
    pub convert_to_ccs: bool,
    /// Recorded ownership handling mode: preserve or takeover.
    /// `None` means the user did not explicitly set `--ownership`, so the
    /// policy-aware resolver uses the system model convergence intent.
    pub ownership: Option<OwnershipMode>,
    /// Skip confirmation prompts
    pub yes: bool,
    /// Install from a specific distro (cross-distro canonical resolution)
    pub from_distro: Option<String>,
    /// Repository provenance supplied by an internal caller that already
    /// selected and downloaded the package before calling `cmd_install`.
    pub(crate) repository_provenance: Option<RepositoryInstallProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositoryInstallProvenance {
    pub repository_id: i64,
    pub source_distro: Option<String>,
    pub version_scheme: conary_core::repository::versioning::VersionScheme,
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
    let version_scheme = resolve_package_version_scheme(package);

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

pub(crate) fn verify_ccs_package_authority(
    db_path: &str,
    ccs_path: &Path,
    repository_provenance: Option<&RepositoryInstallProvenance>,
) -> Result<conary_core::ccs::VerifiedCcsArchive> {
    let keys = if let Some(provenance) = repository_provenance {
        let conn = crate::commands::open_db(db_path)?;
        let mut keys =
            RepositoryPackageKey::trusted_keys_for_repository(&conn, provenance.repository_id)
                .with_context(|| {
                    format!(
                        "load active package keys for repository {}",
                        provenance.repository_id
                    )
                })?;
        if keys.is_empty() {
            keys = RepositoryPackageKey::trusted_tuf_targets_keys_for_repository(
                &conn,
                provenance.repository_id,
            )
            .with_context(|| {
                format!(
                    "load verified TUF targets keys for repository {}",
                    provenance.repository_id
                )
            })?;
        }
        if keys.is_empty() {
            anyhow::bail!(
                "Repository {} has no verified package-authority keys for CCS installation",
                provenance.repository_id
            );
        }
        keys
    } else {
        crate::commands::ccs::local_dev_trust_policy()?
            .context(
                "Local CCS installation requires an initialized local-dev signing key or repository trust provenance",
            )?
            .trusted_keys()
            .to_vec()
    };
    let policy = TrustPolicy::strict(keys);
    verify_package(ccs_path, &policy).with_context(|| {
        format!(
            "CCS package authority verification failed for {}",
            ccs_path.display()
        )
    })
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
        let package = RepositoryPackage::new(
            88,
            "tree".to_string(),
            "2.2.1-4.fc44".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:abc123".to_string(),
            1024,
            "https://static.example.invalid/tree.ccs".to_string(),
        );
        let provenance = repository_install_provenance_from_package(&package, &repository).unwrap();

        assert_eq!(provenance.repository_id, 88);
        assert_eq!(provenance.source_distro.as_deref(), Some("fedora"));
        assert_eq!(
            provenance.version_scheme,
            conary_core::repository::versioning::VersionScheme::Rpm
        );
        assert_eq!(provenance.source_kind, RepositorySourceKind::Static);
    }
}
