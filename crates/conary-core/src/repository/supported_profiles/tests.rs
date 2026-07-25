// conary-core/src/repository/supported_profiles/tests.rs

use super::*;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::versioning::VersionScheme;

#[test]
fn catalog_contains_exact_public_profiles() {
    let ids: Vec<_> = public_profiles()
        .iter()
        .map(|profile| profile.id())
        .collect();
    assert_eq!(ids, vec!["fedora-44", "ubuntu-26.04", "arch"]);
}

#[test]
fn catalog_rejects_unsupported_public_ids() {
    for id in [
        "debian",
        "debian-13",
        "linux-mint",
        "ubuntu-noble",
        "fedora-45",
        "fedora",
    ] {
        assert!(
            profile_by_public_id(id).is_none(),
            "{id} must not be public"
        );
    }
}

#[test]
fn ubuntu_profile_uses_deb_flavor_and_debian_version_scheme() {
    let profile = profile_by_public_id("ubuntu-26.04").expect("ubuntu profile");
    assert_eq!(profile.package_format(), ProfilePackageFormat::Deb);
    assert_eq!(profile.dependency_flavor(), RepositoryDependencyFlavor::Deb);
    assert_eq!(profile.version_scheme(), VersionScheme::Debian);
}

#[test]
fn route_lookup_returns_route_metadata_and_matching_profile_ids() {
    let fedora = route_by_slug("fedora").expect("fedora route");
    assert_eq!(fedora.slug(), "fedora");
    assert_eq!(fedora.public_profile_ids(), &["fedora-44"]);

    let ubuntu = route_by_slug("ubuntu").expect("ubuntu route");
    assert_eq!(ubuntu.public_profile_ids(), &["ubuntu-26.04"]);

    let arch = route_by_slug("arch").expect("arch route");
    assert_eq!(arch.public_profile_ids(), &["arch"]);

    assert!(route_by_slug("debian").is_none());
    assert_eq!(
        profile_for_remi_route("fedora").map(SupportedProfile::id),
        Some("fedora-44")
    );
    assert!(profile_for_remi_route("debian").is_none());
}

#[test]
fn family_slug_lookup_does_not_accept_public_ids() {
    assert!(profile_by_family_slug("fedora-44").is_none());
    assert!(profile_by_family_slug("ubuntu-26.04").is_none());
    assert!(profile_by_family_slug("fedora").is_some());
    assert!(profile_by_family_slug("ubuntu").is_some());
    assert!(profile_by_family_slug("arch").is_some());
}

#[test]
fn remi_target_lookup_requires_exact_public_ids() {
    assert_eq!(
        profile_for_remi_target("fedora-44").map(SupportedProfile::id),
        Some("fedora-44")
    );
    assert!(profile_for_remi_target("fedora").is_none());
    assert!(profile_for_remi_target("ubuntu").is_none());
    assert!(profile_for_remi_target("debian").is_none());
}

#[test]
fn arch_profile_owns_exact_build_time_scriptlet_shell() {
    let profile = arch_source_profile(Some("arch")).expect("Arch source profile");
    assert_eq!(profile.scriptlet_shell(), Some("/usr/bin/bash"));
    assert_eq!(
        arch_source_profile(None).map(SupportedProfile::id),
        Some("arch")
    );
    assert!(arch_source_profile(Some("ubuntu")).is_none());
}
