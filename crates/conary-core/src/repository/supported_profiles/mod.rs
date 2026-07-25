// conary-core/src/repository/supported_profiles/mod.rs

mod types;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::LazyLock;

use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::versioning::VersionScheme;

pub use types::{ProfilePackageFormat, SupportedProfile, SupportedRoute};

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
            !profile.repology_repo().trim().is_empty(),
            "profile Repology repository id must not be empty"
        );
        match (profile.package_format(), profile.scriptlet_shell()) {
            (ProfilePackageFormat::Arch, Some(shell)) => assert!(
                Path::new(shell).is_absolute(),
                "Arch profile scriptlet shell must be absolute"
            ),
            (ProfilePackageFormat::Arch, None) => {
                panic!("Arch profile must declare its build-time scriptlet shell")
            }
            (_, Some(_)) => {
                panic!("only ALPM source profiles may declare a scriptlet shell")
            }
            (_, None) => {}
        }
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

/// Resolve an exact repository identity declared by a canonical mapping
/// contract. Public profile IDs, family/route slugs, and Repology repository
/// IDs are aliases owned by the embedded profile catalog.
#[must_use]
pub fn profile_by_repository_identity(identity: &str) -> Option<&'static SupportedProfile> {
    let identity = identity.trim();
    let mut matches = public_profiles().iter().filter(|profile| {
        profile.id() == identity
            || profile.family_slug() == identity
            || profile.remi_route_slug() == identity
            || profile.repology_repo() == identity
    });
    let profile = matches.next()?;
    matches.next().is_none().then_some(profile)
}

/// Resolve a configured Remi target to one exact public profile.
#[must_use]
pub fn profile_for_remi_target(target: &str) -> Option<&'static SupportedProfile> {
    profile_by_public_id(target)
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

/// Resolve a public Remi route only when it has one unambiguous profile.
#[must_use]
pub fn profile_for_remi_route(slug: &str) -> Option<&'static SupportedProfile> {
    let route = route_by_slug(slug)?;
    let [profile_id] = route.public_profile_ids() else {
        return None;
    };
    profile_by_public_id(profile_id)
}

/// Resolve the exact ALPM source profile whose build-time scriptlet shell owns
/// a package's `.INSTALL` interpreter ABI.
///
/// Repository conversion supplies a public profile ID or unambiguous route.
/// Direct local ALPM installation is accepted only while exactly one supported
/// ALPM source profile exists.
pub fn arch_source_profile(
    source_profile_or_route: Option<&str>,
) -> Option<&'static SupportedProfile> {
    if let Some(source) = source_profile_or_route {
        return profile_by_public_id(source)
            .or_else(|| profile_for_remi_route(source))
            .filter(|profile| profile.package_format() == ProfilePackageFormat::Arch);
    }

    let mut profiles = public_profiles()
        .iter()
        .filter(|profile| profile.package_format() == ProfilePackageFormat::Arch);
    let profile = profiles.next()?;
    profiles.next().is_none().then_some(profile)
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
