// conary-core/src/repository/supported_profiles/mod.rs

mod types;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::LazyLock;

pub use types::{
    ProfilePackageFormat, ProfileSourceMemberContract, ProfileSourceRole, SupportTier,
    SupportedProfile, SupportedRoute,
};

use types::{CatalogDocument, SupportedProfile as Profile};

const CATALOG_TOML: &str = include_str!("catalog.toml");

static PROFILES: LazyLock<Vec<SupportedProfile>> = LazyLock::new(|| {
    let parsed: CatalogDocument =
        toml::from_str(CATALOG_TOML).expect("embedded supported profile catalog must parse");
    validate_catalog(parsed.profiles)
});

static PUBLIC_PROFILES: LazyLock<Vec<SupportedProfile>> = LazyLock::new(|| {
    profiles()
        .iter()
        .filter(|profile| profile.support_tier().is_public())
        .cloned()
        .collect()
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
    assert_eq!(ids, ["fedora-44", "ubuntu-26.04", "arch", "solus"]);

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
        assert!(
            !profile.members().is_empty(),
            "profile {} must declare its exact repository members",
            profile.id()
        );
        let mut repository_identities = std::collections::BTreeSet::new();
        let mut precedences = std::collections::BTreeSet::new();
        for member in profile.members() {
            assert!(
                !member.repository_identity.trim().is_empty()
                    && member.repository_identity.trim() == member.repository_identity,
                "profile {} has an invalid repository identity",
                profile.id()
            );
            assert!(
                repository_identities.insert(member.repository_identity.as_str()),
                "profile {} repeats repository identity {}",
                profile.id(),
                member.repository_identity
            );
            assert!(
                precedences.insert(member.precedence),
                "profile {} repeats member precedence {}",
                profile.id(),
                member.precedence
            );
        }
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

/// Return every known profile, including non-public implementation and
/// lifecycle states.
#[must_use]
pub fn profiles() -> &'static [SupportedProfile] {
    PROFILES.as_slice()
}

#[must_use]
pub fn public_profiles() -> &'static [SupportedProfile] {
    PUBLIC_PROFILES.as_slice()
}

/// Resolve any exact known profile without granting public authority.
#[must_use]
pub fn profile_by_id(id: &str) -> Option<&'static SupportedProfile> {
    let id = id.trim();
    profiles().iter().find(|profile| profile.id() == id)
}

#[must_use]
pub fn profile_by_public_id(id: &str) -> Option<&'static SupportedProfile> {
    let id = id.trim();
    public_profiles().iter().find(|profile| profile.id() == id)
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

/// Resolve an exact public ALPM source profile whose build-time scriptlet
/// shell owns a package's `.INSTALL` interpreter ABI.
#[must_use]
pub fn alpm_source_profile(source_profile_id: &str) -> Option<&'static SupportedProfile> {
    profile_by_public_id(source_profile_id)
        .filter(|profile| profile.package_format() == ProfilePackageFormat::Arch)
}
