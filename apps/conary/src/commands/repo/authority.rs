// apps/conary/src/commands/repo/authority.rs

//! CCS package-authority resolution for Remi-backed repositories.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use conary_core::ccs::signing::load_signing_public_key;
use conary_core::db::models::RepositoryPackageKeyStatus;
use conary_core::repository::PackageKeyStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedCcsPackageKey {
    pub(super) public_key: String,
    pub(super) key_id: Option<String>,
    pub(super) status: RepositoryPackageKeyStatus,
}

pub(super) fn resolve_ccs_package_authority(
    default_strategy: Option<&str>,
    remi_endpoint: Option<&str>,
    profile_id: &str,
    explicit_key_paths: &[PathBuf],
) -> Result<Vec<ResolvedCcsPackageKey>> {
    if default_strategy != Some("remi") {
        return Ok(Vec::new());
    }
    let endpoint = remi_endpoint.context("--remi-endpoint is required for Remi authority")?;

    let keys = if explicit_key_paths.is_empty() {
        let catalog = conary_core::repository::remi_authority::canonical_remi_package_keys(
            endpoint, profile_id,
        )
        .with_context(|| {
            format!(
                "Remi endpoint '{endpoint}' has no release-tracked CCS package authority for \
                     exact profile '{profile_id}'; provide its authenticated targets.public with \
                     --ccs-package-key"
            )
        })?;
        catalog
            .iter()
            .map(|key| ResolvedCcsPackageKey {
                public_key: key.public_key.clone(),
                key_id: key.key_id.clone(),
                status: match key.status {
                    PackageKeyStatus::Active => RepositoryPackageKeyStatus::Active,
                    PackageKeyStatus::Retired => RepositoryPackageKeyStatus::Retired,
                },
            })
            .collect::<Vec<_>>()
    } else {
        explicit_key_paths
            .iter()
            .map(|path| {
                let key = load_signing_public_key(path).with_context(|| {
                    format!(
                        "load authenticated CCS package authority {}",
                        path.display()
                    )
                })?;
                Ok(ResolvedCcsPackageKey {
                    public_key: key.public_key_base64().to_string(),
                    key_id: key.key_id().map(str::to_string),
                    status: RepositoryPackageKeyStatus::Active,
                })
            })
            .collect::<Result<Vec<_>>>()?
    };

    let mut public_keys = BTreeSet::new();
    for key in &keys {
        if !public_keys.insert(key.public_key.as_str()) {
            anyhow::bail!(
                "Remi CCS package authority repeats public key {}",
                key.public_key
            );
        }
    }
    if !keys
        .iter()
        .any(|key| key.status == RepositoryPackageKeyStatus::Active)
    {
        anyhow::bail!(
            "Remi CCS package authority for exact profile '{profile_id}' has no active key"
        );
    }
    Ok(keys)
}
