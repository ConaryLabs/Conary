// apps/remi/src/server/universe_validation.rs

//! Cross-object validation for exact canonical implementations in Remi universes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::canonical::CanonicalMapSnapshot;
use conary_core::repository::catalog::{
    CatalogReader, ProfileRevisionV2, verify_profile_catalog_bundle,
};
use thiserror::Error;

/// Stable semantic failures produced by canonical candidate validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum CanonicalCandidateValidationError {
    #[error(
        "canonical candidate implementation '{canonical}' names profile '{profile}' absent from the candidate universe"
    )]
    MissingProfile { canonical: String, profile: String },
    #[error(
        "canonical candidate implementation '{canonical}' names exact package '{package}' absent from profile '{profile}'"
    )]
    MissingPackage {
        canonical: String,
        profile: String,
        package: String,
    },
    #[error("candidate universe repeats exact profile '{profile}'")]
    DuplicateProfile { profile: String },
}

/// One independently reopened profile catalog ready for serving-cache reuse.
pub(crate) struct VerifiedCanonicalProfile {
    pub(crate) profile: String,
    pub(crate) profile_revision_sha256: String,
    pub(crate) reader: CatalogReader,
}

/// Reopen every candidate profile and require every exact canonical
/// implementation to name a package actually present in its profile catalog.
///
/// Presence checks use the catalog's exact package-name index. The working set
/// is one reader per public profile and scalar canonical implementation facts;
/// package variants and relations are never accumulated.
pub(crate) fn validate_canonical_candidate<'a>(
    catalog_dir: &Path,
    canonical: &CanonicalMapSnapshot,
    profiles: impl IntoIterator<Item = &'a ProfileRevisionV2>,
) -> Result<Vec<VerifiedCanonicalProfile>> {
    let mut verified = BTreeMap::new();
    for revision in profiles {
        let profile_revision_sha256 = revision.manifest_sha256()?;
        let bundle = profile_bundle_path(catalog_dir, revision, &profile_revision_sha256);
        let reader = verify_profile_catalog_bundle(&bundle, revision).with_context(|| {
            format!(
                "reopen canonical candidate profile '{}' revision {}",
                revision.profile, profile_revision_sha256
            )
        })?;
        if verified
            .insert(
                revision.profile.clone(),
                VerifiedCanonicalProfile {
                    profile: revision.profile.clone(),
                    profile_revision_sha256,
                    reader,
                },
            )
            .is_some()
        {
            return Err(CanonicalCandidateValidationError::DuplicateProfile {
                profile: revision.profile.clone(),
            }
            .into());
        }
    }

    for entry in &canonical.entries {
        for (profile, package) in &entry.implementations {
            let candidate = verified.get(profile).ok_or_else(|| {
                CanonicalCandidateValidationError::MissingProfile {
                    canonical: entry.canonical.clone(),
                    profile: profile.clone(),
                }
            })?;
            if !candidate.reader.contains_package_name(package)? {
                return Err(CanonicalCandidateValidationError::MissingPackage {
                    canonical: entry.canonical.clone(),
                    profile: profile.clone(),
                    package: package.clone(),
                }
                .into());
            }
        }
    }

    Ok(verified.into_values().collect())
}

fn profile_bundle_path(
    catalog_dir: &Path,
    revision: &ProfileRevisionV2,
    profile_revision_sha256: &str,
) -> PathBuf {
    catalog_dir
        .join("profiles")
        .join(&revision.profile)
        .join(profile_revision_sha256)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use conary_core::canonical::{CanonicalMapEntry, CanonicalMapSnapshot};

    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};

    fn canonical(profile: &str, package: &str) -> CanonicalMapSnapshot {
        CanonicalMapSnapshot {
            schema_version: conary_core::canonical::CANONICAL_MAP_SCHEMA_VERSION,
            revision: 1,
            generated_at: Some("2026-08-23T00:00:00Z".to_string()),
            entries: vec![CanonicalMapEntry {
                canonical: "shell".to_string(),
                kind: "package".to_string(),
                category: None,
                implementations: BTreeMap::from([(profile.to_string(), package.to_string())]),
            }],
        }
    }

    fn active_revision(fixture: &ActiveCatalogFixture, profile: &str) -> ProfileRevisionV2 {
        fixture
            .authority()
            .open_active_profile(profile)
            .expect("open active fixture profile")
            .manifest()
            .clone()
    }

    #[test]
    fn exact_package_name_in_any_variant_satisfies_the_contract() {
        let fixture = ActiveCatalogFixture::new();
        fixture.activate(
            "fedora-44",
            1,
            vec![
                package(
                    "fedora-44",
                    "bash",
                    "5.3",
                    "1.fc44",
                    Some("x86_64"),
                    100,
                    "bash-v1",
                ),
                package(
                    "fedora-44",
                    "bash",
                    "5.3",
                    "2.fc44",
                    Some("x86_64"),
                    101,
                    "bash-v2",
                ),
            ],
        );
        let revision = active_revision(&fixture, "fedora-44");

        let verified = validate_canonical_candidate(
            fixture.catalog_dir(),
            &canonical("fedora-44", "bash"),
            [&revision],
        )
        .expect("validate exact package presence");

        assert_eq!(verified.len(), 1);
        assert_eq!(verified[0].profile, "fedora-44");
    }

    #[test]
    fn missing_exact_package_is_a_typed_failure() {
        let fixture = ActiveCatalogFixture::new();
        fixture.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "bash",
            )],
        );
        let revision = active_revision(&fixture, "fedora-44");

        let error = match validate_canonical_candidate(
            fixture.catalog_dir(),
            &canonical("fedora-44", "shell-provider"),
            [&revision],
        ) {
            Ok(_) => panic!("provides and aliases must not satisfy package presence"),
            Err(error) => error,
        };

        assert_eq!(
            error.downcast_ref::<CanonicalCandidateValidationError>(),
            Some(&CanonicalCandidateValidationError::MissingPackage {
                canonical: "shell".to_string(),
                profile: "fedora-44".to_string(),
                package: "shell-provider".to_string(),
            })
        );
    }

    #[test]
    fn missing_candidate_profile_is_a_typed_failure() {
        let fixture = ActiveCatalogFixture::new();
        fixture.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "bash",
            )],
        );
        let revision = active_revision(&fixture, "fedora-44");

        let error = match validate_canonical_candidate(
            fixture.catalog_dir(),
            &canonical("arch", "bash"),
            [&revision],
        ) {
            Ok(_) => panic!("canonical profile outside the candidate must fail"),
            Err(error) => error,
        };

        assert_eq!(
            error.downcast_ref::<CanonicalCandidateValidationError>(),
            Some(&CanonicalCandidateValidationError::MissingProfile {
                canonical: "shell".to_string(),
                profile: "arch".to_string(),
            })
        );
    }
}
