// crates/conary-core/src/repository/remi_authority.rs

//! Release-tracked CCS package authority for Conary's canonical Remi service.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

use super::static_repo::{PackageKeyEntry, PackageKeyStatus};
use super::supported_profiles::{profile_by_public_id, public_profiles};

const CATALOG_TOML: &str = include_str!("remi_authority/catalog.toml");
const CATALOG_SCHEMA_VERSION: u64 = 1;

static CATALOG: LazyLock<CanonicalRemiAuthorityCatalog> = LazyLock::new(|| {
    let catalog: CanonicalRemiAuthorityCatalog =
        toml::from_str(CATALOG_TOML).expect("embedded canonical Remi authority catalog must parse");
    catalog
        .validate()
        .expect("embedded canonical Remi authority catalog must be valid");
    catalog
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalRemiAuthorityCatalog {
    schema_version: u64,
    endpoint: String,
    profiles: Vec<ProfileAuthority>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileAuthority {
    profile: String,
    keys: Vec<PackageKeyEntry>,
}

impl CanonicalRemiAuthorityCatalog {
    fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_SCHEMA_VERSION {
            bail!(
                "canonical Remi authority schema {} is unsupported; expected {}",
                self.schema_version,
                CATALOG_SCHEMA_VERSION
            );
        }
        require_https_root_endpoint(&self.endpoint)?;

        let expected_profiles = public_profiles()
            .iter()
            .map(|profile| profile.id())
            .collect::<BTreeSet<_>>();
        let mut observed_profiles = BTreeSet::new();
        for authority in &self.profiles {
            let profile = profile_by_public_id(&authority.profile).with_context(|| {
                format!(
                    "canonical Remi authority names unknown exact profile '{}'",
                    authority.profile
                )
            })?;
            if !observed_profiles.insert(profile.id()) {
                bail!(
                    "canonical Remi authority repeats exact profile '{}'",
                    profile.id()
                );
            }
            if authority.keys.is_empty() {
                bail!(
                    "canonical Remi authority for exact profile '{}' has no package keys",
                    profile.id()
                );
            }

            let mut public_keys = BTreeSet::new();
            let mut has_active = false;
            for key in &authority.keys {
                key.validate().with_context(|| {
                    format!(
                        "invalid canonical Remi package key for exact profile '{}'",
                        profile.id()
                    )
                })?;
                if !public_keys.insert(key.public_key.as_str()) {
                    bail!(
                        "canonical Remi authority repeats a package key for exact profile '{}'",
                        profile.id()
                    );
                }
                has_active |= matches!(key.status, PackageKeyStatus::Active);
            }
            if !has_active {
                bail!(
                    "canonical Remi authority for exact profile '{}' has no active package key",
                    profile.id()
                );
            }
        }

        if observed_profiles != expected_profiles {
            bail!(
                "canonical Remi authority profiles {:?} do not match supported profiles {:?}",
                observed_profiles,
                expected_profiles
            );
        }
        Ok(())
    }
}

/// Exact canonical Remi endpoint whose package keys ship with this release.
#[must_use]
pub fn canonical_remi_endpoint() -> &'static str {
    &CATALOG.endpoint
}

/// Return release-tracked package keys only for an exact canonical
/// endpoint/profile pair.
#[must_use]
pub fn canonical_remi_package_keys(
    endpoint: &str,
    profile_id: &str,
) -> Option<&'static [PackageKeyEntry]> {
    if !same_root_endpoint(endpoint, canonical_remi_endpoint()) {
        return None;
    }
    CATALOG
        .profiles
        .iter()
        .find(|authority| authority.profile == profile_id)
        .map(|authority| authority.keys.as_slice())
}

fn same_root_endpoint(observed: &str, expected: &str) -> bool {
    let Ok(observed) = root_endpoint_identity(observed) else {
        return false;
    };
    let Ok(expected) = root_endpoint_identity(expected) else {
        return false;
    };
    observed == expected
}

fn require_https_root_endpoint(endpoint: &str) -> Result<()> {
    let parsed = Url::parse(endpoint)
        .with_context(|| format!("canonical Remi endpoint '{endpoint}' is not a valid URL"))?;
    if parsed.scheme() != "https" {
        bail!("canonical Remi endpoint must use HTTPS");
    }
    root_endpoint_identity(endpoint)?;
    Ok(())
}

fn root_endpoint_identity(endpoint: &str) -> Result<(String, String, Option<u16>)> {
    let parsed =
        Url::parse(endpoint).with_context(|| format!("invalid Remi endpoint '{endpoint}'"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Remi endpoint must use HTTP or HTTPS");
    }
    let host = parsed
        .host_str()
        .with_context(|| format!("Remi endpoint '{endpoint}' has no host"))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        bail!("Remi endpoint must be an origin URL without credentials, path, query, or fragment");
    }
    Ok((
        parsed.scheme().to_string(),
        host.to_ascii_lowercase(),
        parsed.port_or_known_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_every_exact_public_profile_with_valid_active_keys() {
        assert_eq!(canonical_remi_endpoint(), "https://remi.conary.io");
        for profile in public_profiles() {
            let keys = canonical_remi_package_keys(canonical_remi_endpoint(), profile.id())
                .expect("canonical profile authority");
            assert!(!keys.is_empty());
            assert!(
                keys.iter()
                    .any(|key| matches!(key.status, PackageKeyStatus::Active))
            );
            for key in keys {
                key.validate().unwrap();
            }
        }
    }

    #[test]
    fn canonical_match_is_exact_to_endpoint_origin_and_profile() {
        assert!(canonical_remi_package_keys("https://remi.conary.io/", "fedora-44").is_some());
        for endpoint in [
            "http://remi.conary.io",
            "https://remi.conary.io.evil.example",
            "https://remi.conary.io/v1",
            "https://user@remi.conary.io",
            "https://remi.conary.io?profile=fedora-44",
        ] {
            assert!(
                canonical_remi_package_keys(endpoint, "fedora-44").is_none(),
                "{endpoint} must not inherit canonical package authority"
            );
        }
        assert!(canonical_remi_package_keys(canonical_remi_endpoint(), "fedora").is_none());
    }

    #[test]
    fn exact_profiles_have_distinct_signing_authority() {
        let keys = public_profiles()
            .iter()
            .map(|profile| {
                canonical_remi_package_keys(canonical_remi_endpoint(), profile.id()).unwrap()[0]
                    .public_key
                    .as_str()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), public_profiles().len());
    }
}
