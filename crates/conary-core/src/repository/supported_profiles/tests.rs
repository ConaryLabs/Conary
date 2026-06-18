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
    assert_eq!(
        profile.replay_target_for_arch("x86_64").to_id(),
        "deb/ubuntu/26.04/x86_64"
    );
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
fn repository_hints_are_profile_owned() {
    assert_eq!(
        profile_by_public_id("fedora-44")
            .unwrap()
            .repository_name_patterns(),
        &["fedora%"]
    );
    assert_eq!(
        profile_by_public_id("ubuntu-26.04")
            .unwrap()
            .repository_name_patterns(),
        &["ubuntu%"]
    );
    assert_eq!(
        profile_by_public_id("arch")
            .unwrap()
            .repository_name_patterns(),
        &["arch%"]
    );
}

#[test]
fn profile_backed_lifecycle_query_accepts_only_explicit_entries() {
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

    let profile = profile_by_public_id("fedora-44").unwrap();

    assert_eq!(
        profile.service_status("example.service"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.service_status("anything.service"),
        ProfileConstraintStatus::Unsupported
    );
    assert_eq!(
        profile.tmpfiles_status("example.conf"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.sysctl_status("kernel.example"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.user_status("example"),
        ProfileConstraintStatus::Unsupported
    );
    assert_eq!(
        profile.alternative_status("editor"),
        ProfileConstraintStatus::Unsupported
    );
}
