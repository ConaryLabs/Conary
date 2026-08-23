// conary-core/src/repository/universe/contract.rs

//! Strict content-addressed wire contract for one complete Remi universe.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::canonical::CANONICAL_MAP_SCHEMA_VERSION;
use crate::error::{Error, Result};
use crate::repository::catalog::{CATALOG_CONTENT_SCHEMA_V1, ProfileRevisionV2};
use crate::repository::supported_profiles::profile_by_public_id;

use super::super::catalog::PROFILE_REVISION_SCHEMA_V2;

pub const REMI_UNIVERSE_SCHEMA_V2: u32 = 2;

/// One immutable profile-catalog object authorized by the universe manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiUniverseCatalogObjectV2 {
    pub schema_version: u32,
    pub sha256: String,
    pub size: u64,
    pub logical_digest_sha256: String,
}

impl RemiUniverseCatalogObjectV2 {
    #[must_use]
    pub fn target_path(&self) -> String {
        object_target_path(&self.sha256)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_CONTENT_SCHEMA_V1 {
            return Err(Error::ConfigError(format!(
                "universe catalog schema {} is unsupported; expected {}",
                self.schema_version, CATALOG_CONTENT_SCHEMA_V1
            )));
        }
        validate_sha256(&self.sha256, "universe catalog object")?;
        validate_sha256(
            &self.logical_digest_sha256,
            "universe catalog logical digest",
        )
    }
}

/// One strict canonical-map object authorized by the universe manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiUniverseCanonicalMapObjectV2 {
    pub schema_version: u32,
    pub sha256: String,
    pub size: u64,
    pub revision: u64,
    pub entry_count: u64,
}

impl RemiUniverseCanonicalMapObjectV2 {
    #[must_use]
    pub fn target_path(&self) -> String {
        object_target_path(&self.sha256)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != CANONICAL_MAP_SCHEMA_VERSION {
            return Err(Error::ConfigError(format!(
                "universe canonical-map schema {} is unsupported; expected {}",
                self.schema_version, CANONICAL_MAP_SCHEMA_VERSION
            )));
        }
        validate_sha256(&self.sha256, "universe canonical-map object")?;
        if self.revision == 0 && self.entry_count != 0 {
            return Err(Error::ConfigError(
                "universe canonical-map revision zero must be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// One ordered public profile and its exact immutable resolution object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiUniverseProfileV2 {
    pub ordinal: u32,
    pub profile_revision_sha256: String,
    pub revision: ProfileRevisionV2,
    pub catalog: RemiUniverseCatalogObjectV2,
}

impl RemiUniverseProfileV2 {
    fn validate(&self) -> Result<()> {
        self.revision.validate()?;
        if self.revision.schema_version != PROFILE_REVISION_SCHEMA_V2 {
            return Err(Error::ConfigError(format!(
                "universe profile revision schema {} is unsupported; expected {}",
                self.revision.schema_version, PROFILE_REVISION_SCHEMA_V2
            )));
        }
        if profile_by_public_id(&self.revision.profile).is_none() {
            return Err(Error::ConfigError(format!(
                "universe names unsupported public profile '{}'",
                self.revision.profile
            )));
        }
        self.revision.validate_member_contract()?;
        validate_sha256(&self.profile_revision_sha256, "universe profile revision")?;
        let actual_revision = self.revision.manifest_sha256()?;
        if actual_revision != self.profile_revision_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: actual_revision,
                actual: self.profile_revision_sha256.clone(),
            });
        }
        self.catalog.validate()?;
        if self.catalog.sha256 != self.revision.catalog.sha256
            || self.catalog.size != self.revision.catalog.size
            || self.catalog.logical_digest_sha256 != self.revision.logical_digest_sha256
        {
            return Err(Error::ConflictError(format!(
                "universe profile '{}' catalog descriptor disagrees with its profile revision",
                self.revision.profile
            )));
        }
        Ok(())
    }
}

/// One signed endpoint-wide repository universe.
///
/// The manifest is a TUF target. Every catalog and canonical-map object listed
/// here must also be an exact target in the same verified targets metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiUniverseManifestV2 {
    pub schema_version: u32,
    pub sequence: u64,
    pub metadata_root_sha256: String,
    pub generated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub profiles: Vec<RemiUniverseProfileV2>,
    pub canonical_map: RemiUniverseCanonicalMapObjectV2,
}

impl RemiUniverseManifestV2 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REMI_UNIVERSE_SCHEMA_V2 {
            return Err(Error::ConfigError(format!(
                "Remi universe schema {} is unsupported; expected {}",
                self.schema_version, REMI_UNIVERSE_SCHEMA_V2
            )));
        }
        if self.sequence == 0 {
            return Err(Error::ConfigError(
                "Remi universe sequence must be positive".to_string(),
            ));
        }
        validate_sha256(&self.metadata_root_sha256, "universe metadata root")?;
        if self.expires_at <= self.generated_at {
            return Err(Error::ConfigError(
                "Remi universe expiry must be later than its generation time".to_string(),
            ));
        }
        if self.profiles.is_empty() {
            return Err(Error::ConfigError(
                "Remi universe must contain at least one public profile".to_string(),
            ));
        }

        let mut previous_profile = None;
        let mut revisions = BTreeSet::new();
        let mut objects = BTreeSet::new();
        for (index, profile) in self.profiles.iter().enumerate() {
            let expected_ordinal = u32::try_from(index).map_err(|_| {
                Error::ConfigError("Remi universe contains too many profiles".to_string())
            })?;
            if profile.ordinal != expected_ordinal {
                return Err(Error::ConfigError(format!(
                    "Remi universe profile ordinal {} is noncanonical; expected {}",
                    profile.ordinal, expected_ordinal
                )));
            }
            profile.validate()?;
            let profile_id = profile.revision.profile.as_str();
            if previous_profile.is_some_and(|previous| previous >= profile_id) {
                return Err(Error::ConfigError(
                    "Remi universe profiles must be strictly ordered by public profile ID"
                        .to_string(),
                ));
            }
            if !revisions.insert(&profile.profile_revision_sha256) {
                return Err(Error::ConflictError(format!(
                    "Remi universe repeats profile revision {}",
                    profile.profile_revision_sha256
                )));
            }
            if !objects.insert(&profile.catalog.sha256) {
                return Err(Error::ConflictError(format!(
                    "Remi universe repeats catalog object {}",
                    profile.catalog.sha256
                )));
            }
            previous_profile = Some(profile_id);
        }

        self.canonical_map.validate()?;
        if !objects.insert(&self.canonical_map.sha256) {
            return Err(Error::ConflictError(
                "Remi universe canonical map aliases a profile catalog object".to_string(),
            ));
        }
        Ok(())
    }

    pub fn manifest_sha256(&self) -> Result<String> {
        self.validate()?;
        let bytes = crate::json::canonical_json(self).map_err(|error| {
            Error::ParseError(format!("serialize Remi universe manifest: {error}"))
        })?;
        Ok(crate::hash::sha256(&bytes))
    }

    pub fn target_path(&self) -> Result<String> {
        Ok(format!("universe/{}.json", self.manifest_sha256()?))
    }

    #[must_use]
    pub fn object_target_paths(&self) -> Vec<String> {
        self.profiles
            .iter()
            .map(|profile| profile.catalog.target_path())
            .chain(std::iter::once(self.canonical_map.target_path()))
            .collect()
    }
}

fn object_target_path(sha256: &str) -> String {
    format!("objects/sha256/{sha256}")
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} SHA-256 must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::{
        CatalogArtifactV1, CatalogCountsV1, ProfileSourceMemberV2, SourceStreamKindV1,
        SourceStreamV1,
    };

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn profile(profile: &str, byte: char, ordinal: u32) -> RemiUniverseProfileV2 {
        let mut declared = crate::repository::supported_profiles::profile_by_public_id(profile)
            .unwrap()
            .members()
            .iter()
            .collect::<Vec<_>>();
        declared.sort_by_key(|member| std::cmp::Reverse(member.precedence));
        let members = declared
            .into_iter()
            .enumerate()
            .map(|(member_ordinal, member)| ProfileSourceMemberV2 {
                ordinal: member_ordinal as u32,
                source_identity: format!("{profile}-source"),
                repository_identity: member.repository_identity.clone(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "current".to_string(),
                },
                role: member.role,
                precedence: member.precedence,
                required: true,
                source_snapshot_sha256: digest(byte),
            })
            .collect::<Vec<_>>();
        let source_evidence = members.len() as u64;
        let revision = ProfileRevisionV2 {
            schema_version: PROFILE_REVISION_SCHEMA_V2,
            profile: profile.to_string(),
            projection_version: 1,
            members,
            catalog: CatalogArtifactV1 {
                sha256: digest(byte),
                size: 4096,
            },
            logical_digest_sha256: digest(char::from_u32(byte as u32 + 1).unwrap()),
            counts: CatalogCountsV1 {
                packages: 1,
                provides: 1,
                requirement_groups: 0,
                requirement_atoms: 0,
                source_evidence,
            },
        };
        RemiUniverseProfileV2 {
            ordinal,
            profile_revision_sha256: revision.manifest_sha256().unwrap(),
            catalog: RemiUniverseCatalogObjectV2 {
                schema_version: CATALOG_CONTENT_SCHEMA_V1,
                sha256: revision.catalog.sha256.clone(),
                size: revision.catalog.size,
                logical_digest_sha256: revision.logical_digest_sha256.clone(),
            },
            revision,
        }
    }

    fn manifest() -> RemiUniverseManifestV2 {
        let generated_at = "2026-08-22T10:00:00Z".parse().unwrap();
        RemiUniverseManifestV2 {
            schema_version: REMI_UNIVERSE_SCHEMA_V2,
            sequence: 7,
            metadata_root_sha256: digest('a'),
            generated_at,
            expires_at: generated_at + chrono::Duration::days(7),
            profiles: vec![profile("arch", 'b', 0), profile("fedora-44", 'd', 1)],
            canonical_map: RemiUniverseCanonicalMapObjectV2 {
                schema_version: CANONICAL_MAP_SCHEMA_VERSION,
                sha256: digest('f'),
                size: 128,
                revision: 4,
                entry_count: 2,
            },
        }
    }

    #[test]
    fn universe_manifest_is_strict_ordered_and_content_addressed() {
        let manifest = manifest();
        manifest.validate().unwrap();
        let digest = manifest.manifest_sha256().unwrap();
        assert_eq!(
            manifest.target_path().unwrap(),
            format!("universe/{digest}.json")
        );
        assert_eq!(
            manifest.object_target_paths(),
            vec![
                format!("objects/sha256/{}", "b".repeat(64)),
                format!("objects/sha256/{}", "d".repeat(64)),
                format!("objects/sha256/{}", "f".repeat(64)),
            ]
        );

        let encoded = crate::json::canonical_json(&manifest).unwrap();
        let decoded: RemiUniverseManifestV2 = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.manifest_sha256().unwrap(), digest);

        let mut value = serde_json::to_value(&manifest).unwrap();
        value["invented_authority"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RemiUniverseManifestV2>(value).is_err());
    }

    #[test]
    fn universe_rejects_reordered_or_mixed_profile_authority() {
        let mut reordered = manifest();
        reordered.profiles.swap(0, 1);
        assert!(reordered.validate().is_err());

        let mut mixed = manifest();
        mixed.profiles[0].catalog.sha256 = digest('9');
        assert!(mixed.validate().is_err());

        let mut repeated = manifest();
        repeated.canonical_map.sha256 = repeated.profiles[0].catalog.sha256.clone();
        assert!(repeated.validate().is_err());
    }

    #[test]
    fn universe_rejects_expiry_and_sequence_regressions_at_contract_boundary() {
        let mut invalid = manifest();
        invalid.sequence = 0;
        assert!(invalid.validate().is_err());

        let mut invalid = manifest();
        invalid.expires_at = invalid.generated_at;
        assert!(invalid.validate().is_err());
    }
}
