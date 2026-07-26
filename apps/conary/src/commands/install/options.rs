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
    /// Exact signed CCS build release to install. Internal restore/update
    /// callers use this after an exact candidate has already been selected.
    pub package_release: Option<String>,
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
    /// Install from an exact supported source profile.
    pub from_profile: Option<String>,
    /// Repository provenance supplied by an internal caller that already
    /// selected and downloaded the package before calling `cmd_install`.
    pub(crate) repository_provenance: Option<RepositoryInstallProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositoryInstallProvenance {
    pub repository_id: i64,
    pub source_profile: Option<String>,
    pub version_scheme: conary_core::repository::versioning::VersionScheme,
    pub source_kind: RepositorySourceKind,
}

/// Exact authority used to verify a CCS envelope.
///
/// Repository provenance remains separate because a locally converted native
/// package keeps its source repository identity while its generated CCS
/// envelope is signed by an exact local conversion key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CcsEnvelopeAuthority {
    LocalDev,
    Repository,
    ExactKey(String),
}

pub(crate) fn repository_install_provenance_from_package(
    package: &RepositoryPackage,
    repository: &Repository,
) -> Result<RepositoryInstallProvenance> {
    let repository_id = repository
        .id
        .ok_or_else(|| anyhow::anyhow!("Selected repository has no database ID"))?;
    let version_scheme = resolve_package_version_scheme(package);
    let source_profile =
        conary_core::repository::selector::candidate_source_profile(package, repository)?
            .map(str::to_string);
    if source_profile.is_none()
        && version_scheme != conary_core::repository::versioning::VersionScheme::Conary
    {
        anyhow::bail!(
            "selected repository package '{}-{}' has no exact source profile",
            package.name,
            package.version
        );
    }

    Ok(RepositoryInstallProvenance {
        repository_id,
        source_profile,
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
    envelope_authority: &CcsEnvelopeAuthority,
    repository_provenance: Option<&RepositoryInstallProvenance>,
) -> Result<conary_core::ccs::VerifiedCcsArchive> {
    let keys = match envelope_authority {
        CcsEnvelopeAuthority::Repository => {
            let provenance = repository_provenance.context(
                "Repository CCS envelope authority requires exact repository provenance",
            )?;
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
        }
        CcsEnvelopeAuthority::LocalDev => crate::commands::ccs::local_dev_trust_policy()?
            .context(
                "Local CCS installation requires an initialized local-dev signing key or explicit envelope authority",
            )?
            .trusted_keys()
            .to_vec(),
        CcsEnvelopeAuthority::ExactKey(key) => vec![key.clone()],
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
        repository.source_profile = Some("fedora-44".to_string());
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
        assert_eq!(provenance.source_profile.as_deref(), Some("fedora-44"));
        assert_eq!(
            provenance.version_scheme,
            conary_core::repository::versioning::VersionScheme::Rpm
        );
        assert_eq!(provenance.source_kind, RepositorySourceKind::Static);
    }
}
