// conary-core/src/repository/distro.rs

//! Shared source-feed family and repository version-scheme inference.

use crate::error::{Error, Result};
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::versioning::VersionScheme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFeed {
    pub id: String,
    pub display_name: String,
}

/// Return the configured upstream source-feed catalog.
#[must_use]
pub fn source_feeds() -> Vec<SourceFeed> {
    crate::repository::supported_profiles::public_profiles()
        .iter()
        .map(|profile| SourceFeed {
            id: profile.id().to_string(),
            display_name: profile.display_name().to_string(),
        })
        .collect()
}

/// Infer the dependency flavor from a supported distro name or internal family
/// label.
#[must_use]
pub fn flavor_from_distro_name(name: &str) -> Option<RepositoryDependencyFlavor> {
    crate::repository::supported_profiles::dependency_flavor_for_name(name)
}

/// Infer a version comparison scheme from a supported distro name or internal
/// family label.
#[must_use]
pub fn version_scheme_from_distro_name(name: &str) -> Option<VersionScheme> {
    crate::repository::supported_profiles::version_scheme_for_name(name)
}

/// Parse a stored DB version-scheme string.
#[must_use]
pub fn version_scheme_from_db(value: Option<&str>) -> Option<VersionScheme> {
    value?.parse().ok()
}

/// Require an exact persisted DB version scheme.
pub fn require_version_scheme_from_db(
    value: Option<&str>,
    owner: impl std::fmt::Display,
) -> Result<VersionScheme> {
    let raw = value.ok_or_else(|| {
        Error::ConfigError(format!("{owner} has no persisted native version scheme"))
    })?;
    version_scheme_from_db(Some(raw)).ok_or_else(|| {
        Error::ConfigError(format!(
            "{owner} has unsupported persisted native version scheme '{raw}'"
        ))
    })
}

/// Check whether a distro name/family label maps to a dependency flavor.
#[must_use]
pub fn flavor_matches_distro_name(name: &str, flavor: RepositoryDependencyFlavor) -> bool {
    flavor_from_distro_name(name) == Some(flavor)
}

/// Convert a dependency flavor to its version comparison scheme.
#[must_use]
pub fn flavor_to_version_scheme(flavor: RepositoryDependencyFlavor) -> VersionScheme {
    flavor.version_scheme()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_feed_names_map_to_flavors_and_schemes() {
        let catalog_ids: Vec<_> = source_feeds().into_iter().map(|distro| distro.id).collect();
        assert_eq!(catalog_ids, vec!["fedora-44", "ubuntu-26.04", "arch"]);

        for (name, flavor, scheme) in [
            (
                "fedora-44",
                RepositoryDependencyFlavor::Rpm,
                VersionScheme::Rpm,
            ),
            (
                "ubuntu-26.04",
                RepositoryDependencyFlavor::Deb,
                VersionScheme::Debian,
            ),
            (
                "arch",
                RepositoryDependencyFlavor::Arch,
                VersionScheme::Arch,
            ),
        ] {
            assert_eq!(flavor_from_distro_name(name), Some(flavor));
            assert_eq!(version_scheme_from_distro_name(name), Some(scheme));
            assert!(flavor_matches_distro_name(name, flavor));
            assert_eq!(flavor_to_version_scheme(flavor), scheme);
        }
    }

    #[test]
    fn internal_family_labels_map_to_flavors_and_schemes() {
        for (name, flavor, scheme) in [
            (
                "fedora",
                RepositoryDependencyFlavor::Rpm,
                VersionScheme::Rpm,
            ),
            (
                "ubuntu",
                RepositoryDependencyFlavor::Deb,
                VersionScheme::Debian,
            ),
            (
                "arch",
                RepositoryDependencyFlavor::Arch,
                VersionScheme::Arch,
            ),
        ] {
            assert_eq!(flavor_from_distro_name(name), Some(flavor));
            assert_eq!(version_scheme_from_distro_name(name), Some(scheme));
        }
    }

    #[test]
    fn unknown_distro_names_have_no_name_only_inference() {
        for name in ["nixos", "debian", "linux-mint", "ubuntu-noble"] {
            assert_eq!(flavor_from_distro_name(name), None);
            assert_eq!(version_scheme_from_distro_name(name), None);
        }
    }

    #[test]
    fn explicit_db_version_scheme_strings_parse_without_fallback() {
        assert_eq!(
            version_scheme_from_db(Some("rpm")),
            Some(VersionScheme::Rpm)
        );
        assert_eq!(
            version_scheme_from_db(Some("debian")),
            Some(VersionScheme::Debian)
        );
        assert_eq!(
            version_scheme_from_db(Some("arch")),
            Some(VersionScheme::Arch)
        );
        assert_eq!(version_scheme_from_db(Some("bogus")), None);
        assert_eq!(version_scheme_from_db(Some(" RPM ")), None);
        assert_eq!(version_scheme_from_db(Some("RPM")), None);
        assert_eq!(version_scheme_from_db(None), None);
        assert!(require_version_scheme_from_db(Some("bogus"), "fixture").is_err());
        assert!(require_version_scheme_from_db(None, "fixture").is_err());
    }
}
