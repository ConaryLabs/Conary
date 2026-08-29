// apps/remi/src/server/universe_validation.rs

//! Cross-object validation for exact canonical implementations in Remi universes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::canonical::CanonicalMapSnapshot;
use conary_core::db::models::RemiCatalogPhysicalAttestation;
use conary_core::repository::catalog::{
    ProfileRevisionV2, verify_registered_profile_catalog_bundle_complete,
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

/// Reopen every candidate profile and require every exact canonical
/// implementation to name a package actually present in its profile catalog.
///
/// Presence checks use the catalog's exact package-name index. The working set
/// is one reader per public profile and scalar canonical implementation facts;
/// package variants and relations are never accumulated.
pub(crate) fn validate_canonical_candidate<'a>(
    catalog_dir: &Path,
    canonical: &CanonicalMapSnapshot,
    profiles: impl IntoIterator<Item = (&'a ProfileRevisionV2, &'a RemiCatalogPhysicalAttestation)>,
) -> Result<()> {
    let mut verified = BTreeMap::new();
    for (revision, physical_attestation) in profiles {
        let profile_revision_sha256 = revision.manifest_sha256()?;
        let bundle = profile_bundle_path(catalog_dir, revision, &profile_revision_sha256);
        let reader = verify_registered_profile_catalog_bundle_complete(
            &bundle,
            revision,
            &physical_attestation.portable_manifest,
        )
        .with_context(|| {
            format!(
                "reopen canonical candidate profile '{}' revision {}",
                revision.profile, profile_revision_sha256
            )
        })?;
        if verified.insert(revision.profile.clone(), reader).is_some() {
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
            if !candidate.contains_package_name(package)? {
                return Err(CanonicalCandidateValidationError::MissingPackage {
                    canonical: entry.canonical.clone(),
                    profile: profile.clone(),
                    package: package.clone(),
                }
                .into());
            }
        }
    }

    Ok(())
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

    fn active_revision(
        fixture: &ActiveCatalogFixture,
        profile: &str,
    ) -> (ProfileRevisionV2, RemiCatalogPhysicalAttestation) {
        let pinned = fixture
            .authority()
            .open_active_profile(profile)
            .expect("open active fixture profile");
        (
            pinned.manifest().clone(),
            pinned.physical_attestation().clone(),
        )
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
        let (revision, physical_attestation) = active_revision(&fixture, "fedora-44");

        validate_canonical_candidate(
            fixture.catalog_dir(),
            &canonical("fedora-44", "bash"),
            [(&revision, &physical_attestation)],
        )
        .expect("validate exact package presence");
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
        let (revision, physical_attestation) = active_revision(&fixture, "fedora-44");

        let error = match validate_canonical_candidate(
            fixture.catalog_dir(),
            &canonical("fedora-44", "shell-provider"),
            [(&revision, &physical_attestation)],
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
        let (revision, physical_attestation) = active_revision(&fixture, "fedora-44");

        let error = match validate_canonical_candidate(
            fixture.catalog_dir(),
            &canonical("arch", "bash"),
            [(&revision, &physical_attestation)],
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
