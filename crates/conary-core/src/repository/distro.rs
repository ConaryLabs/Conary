// conary-core/src/repository/distro.rs

//! Shared source-feed catalog and persisted version-scheme parsing.

use crate::error::{Error, Result};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_feed_names_come_from_exact_public_profile_catalog() {
        let catalog_ids: Vec<_> = source_feeds().into_iter().map(|distro| distro.id).collect();
        assert_eq!(catalog_ids, vec!["fedora-44", "ubuntu-26.04", "arch"]);
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
