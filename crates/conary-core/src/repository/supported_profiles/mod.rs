// conary-core/src/repository/supported_profiles/mod.rs

mod types;

#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::distro::ReplayTargetOwned;
use crate::repository::versioning::VersionScheme;

pub use types::{LifecyclePolicyMode, ProfilePackageFormat, SupportedProfile, SupportedRoute};

use types::{CatalogDocument, SupportedProfile as Profile};

const CATALOG_TOML: &str = include_str!("catalog.toml");

static PUBLIC_PROFILES: LazyLock<Vec<SupportedProfile>> = LazyLock::new(|| {
    let parsed: CatalogDocument =
        toml::from_str(CATALOG_TOML).expect("embedded supported profile catalog must parse");
    validate_catalog(parsed.profiles)
});

fn validate_catalog(profiles: Vec<types::ProfileDocument>) -> Vec<SupportedProfile> {
    let supported = profiles
        .into_iter()
        .map(Profile::new)
        .collect::<Vec<SupportedProfile>>();

    let ids = supported
        .iter()
        .map(SupportedProfile::id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["fedora-44", "ubuntu-26.04", "arch"]);

    for profile in &supported {
        assert!(
            !profile.id().trim().is_empty(),
            "profile id must not be empty"
        );
        assert!(
            !profile.remi_route_slug().trim().is_empty(),
            "profile route slug must not be empty"
        );
        assert!(
            !profile.repository_name_patterns().is_empty(),
            "profile must include repository hints"
        );
    }

    supported
}

#[must_use]
pub fn public_profiles() -> &'static [SupportedProfile] {
    PUBLIC_PROFILES.as_slice()
}

#[must_use]
pub fn profile_by_public_id(id: &str) -> Option<&'static SupportedProfile> {
    let id = id.trim();
    public_profiles().iter().find(|profile| profile.id() == id)
}

#[must_use]
pub fn profile_by_family_slug(slug: &str) -> Option<&'static SupportedProfile> {
    let slug = slug.trim();
    public_profiles()
        .iter()
        .find(|profile| profile.family_slug() == slug)
}

#[must_use]
pub fn route_by_slug(slug: &str) -> Option<SupportedRoute> {
    let slug = slug.trim();
    let public_profile_ids = public_profiles()
        .iter()
        .filter(|profile| profile.remi_route_slug() == slug)
        .map(|profile| profile.id().to_string())
        .collect::<Vec<_>>();
    if public_profile_ids.is_empty() {
        None
    } else {
        Some(SupportedRoute::new(slug.to_string(), public_profile_ids))
    }
}

#[must_use]
pub fn dependency_flavor_for_name(name: &str) -> Option<RepositoryDependencyFlavor> {
    profile_by_public_id(name)
        .or_else(|| profile_by_family_slug(name))
        .map(SupportedProfile::dependency_flavor)
}

#[must_use]
pub fn version_scheme_for_name(name: &str) -> Option<VersionScheme> {
    profile_by_public_id(name)
        .or_else(|| profile_by_family_slug(name))
        .map(SupportedProfile::version_scheme)
}

#[must_use]
pub fn replay_target_for_public_id(id: &str, arch: &str) -> Option<ReplayTargetOwned> {
    if arch.trim().is_empty() {
        return None;
    }
    profile_by_public_id(id).map(|profile| profile.replay_target_for_arch(arch))
}
