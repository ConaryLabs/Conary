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
fn remi_target_lookup_normalizes_legacy_route_slugs_to_public_ids() {
    assert_eq!(
        profile_for_remi_target("fedora-44").map(SupportedProfile::id),
        Some("fedora-44")
    );
    assert_eq!(
        profile_for_remi_target("fedora").map(SupportedProfile::id),
        Some("fedora-44")
    );
    assert_eq!(
        profile_for_remi_target("ubuntu").map(SupportedProfile::id),
        Some("ubuntu-26.04")
    );
    assert!(profile_for_remi_target("debian").is_none());
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
fn repository_name_matching_stays_profile_owned() {
    let fedora = profile_by_public_id("fedora-44").unwrap();
    assert!(fedora.matches_repository_name("fedora-44"));
    assert!(!fedora.matches_repository_name("ubuntu-26.04"));

    let ubuntu = profile_by_public_id("ubuntu-26.04").unwrap();
    assert!(ubuntu.matches_repository_name("ubuntu-26.04"));
    assert!(!ubuntu.matches_repository_name("arch-core"));

    let arch = profile_by_public_id("arch").unwrap();
    assert!(arch.matches_repository_name("arch-core"));
    assert!(arch.matches_repository_name("arch-multilib"));
    assert!(!arch.matches_repository_name("fedora-44"));
}

#[test]
fn profile_backed_lifecycle_query_accepts_only_explicit_entries() {
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

    let profile = profile_by_public_id("fedora-44").unwrap();

    assert_eq!(
        profile.service_status("conary-example.service"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.service_status("anything.service"),
        ProfileConstraintStatus::Unsupported
    );
    assert_eq!(
        profile.tmpfiles_status("/var/lib/conary-example"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.sysctl_status("kernel.example"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.user_status("conary-example"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.alternative_status("editor"),
        ProfileConstraintStatus::Unsupported
    );
}

#[test]
fn m4e_profiles_accept_exact_proof_corpus_lifecycle_entries() {
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

    for id in ["fedora-44", "ubuntu-26.04", "arch"] {
        let profile = profile_by_public_id(id).expect(id);
        assert_eq!(
            profile.service_status("conary-example.service"),
            ProfileConstraintStatus::Accepted
        );
        assert_eq!(
            profile.tmpfiles_status("/var/lib/conary-example"),
            ProfileConstraintStatus::Accepted
        );
        assert_eq!(
            profile.user_status("conary-example"),
            ProfileConstraintStatus::Accepted
        );
        assert_eq!(
            profile.group_status("conary-example"),
            ProfileConstraintStatus::Accepted
        );
        assert_eq!(
            profile.directory_status("/var/lib/conary-example"),
            ProfileConstraintStatus::Accepted
        );
    }
}

#[test]
fn m4e_profiles_reject_old_placeholder_lifecycle_entries() {
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

    for id in ["fedora-44", "ubuntu-26.04", "arch"] {
        let profile = profile_by_public_id(id).expect(id);
        assert_eq!(
            profile.service_status("example.service"),
            ProfileConstraintStatus::Unsupported
        );
        assert_eq!(
            profile.tmpfiles_status("example.conf"),
            ProfileConstraintStatus::Unsupported
        );
    }
}
