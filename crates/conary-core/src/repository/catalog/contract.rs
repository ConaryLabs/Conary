// crates/conary-core/src/repository/catalog/contract.rs

//! Strict content-addressed catalog manifest contracts.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::repository::{RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy};

pub const SOURCE_SNAPSHOT_SCHEMA_V1: u32 = 1;
pub const PROFILE_REVISION_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEcosystemV1 {
    Rpm,
    Deb,
    Alpm,
    Eopkg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStreamKindV1 {
    Release,
    Channel,
    Rolling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceStreamV1 {
    pub kind: SourceStreamKindV1,
    pub identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenanceV1 {
    pub ecosystem: SourceEcosystemV1,
    pub metadata_url: String,
    pub content_url: Option<String>,
    pub parser_config: RepositoryParserConfig,
    pub parser_config_sha256: String,
    pub trust_policy: RepositoryTrustPolicy,
    pub trust_policy_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMetadataObjectRoleV1 {
    RpmPrimary,
    RpmFilelists,
    DebianPackages,
    ArchDatabase,
    EopkgIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadataObjectV1 {
    pub role: SourceMetadataObjectRoleV1,
    pub source_path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCountsV1 {
    pub packages: u64,
    pub provides: u64,
    pub requirement_groups: u64,
    pub requirement_atoms: u64,
    pub source_evidence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogArtifactV1 {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshotV1 {
    pub schema_version: u32,
    pub source_profile: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub stream: SourceStreamV1,
    pub stream_binding_sha256: String,
    pub parser_projection_version: u32,
    pub provenance: SourceProvenanceV1,
    pub authenticated_root: CatalogArtifactV1,
    pub authenticated_objects: Vec<SourceMetadataObjectV1>,
    pub catalog: CatalogArtifactV1,
    pub logical_digest_sha256: String,
    pub counts: CatalogCountsV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSourceMemberV1 {
    pub ordinal: u32,
    pub source_identity: String,
    pub repository_identity: String,
    pub stream: SourceStreamV1,
    pub priority: i32,
    pub required: bool,
    pub source_snapshot_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRevisionV1 {
    pub schema_version: u32,
    pub profile: String,
    pub projection_version: u32,
    pub members: Vec<ProfileSourceMemberV1>,
    pub catalog: CatalogArtifactV1,
    pub logical_digest_sha256: String,
    pub counts: CatalogCountsV1,
}

impl SourceMetadataObjectV1 {
    fn validate(&self) -> Result<()> {
        validate_relative_source_path(&self.source_path)?;
        validate_sha256(&self.sha256, "authenticated metadata object SHA-256")
    }
}

impl CatalogArtifactV1 {
    fn validate(&self, label: &str) -> Result<()> {
        validate_sha256(&self.sha256, label)
    }
}

impl SourceSnapshotV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SOURCE_SNAPSHOT_SCHEMA_V1 {
            return Err(Error::ConfigError(format!(
                "source snapshot schema {} is unsupported; expected {}",
                self.schema_version, SOURCE_SNAPSHOT_SCHEMA_V1
            )));
        }
        validate_storage_component(&self.source_profile, "source snapshot profile")?;
        validate_identity(&self.source_identity, "source snapshot source identity")?;
        validate_identity(
            &self.repository_identity,
            "source snapshot repository identity",
        )?;
        validate_sha256(
            &self.stream_binding_sha256,
            "source snapshot stream binding",
        )?;
        validate_identity(&self.stream.identity, "source snapshot stream identity")?;
        if self.parser_projection_version == 0 {
            return Err(Error::ConfigError(
                "source snapshot parser projection version must be positive".to_string(),
            ));
        }
        validate_sha256(
            &self.provenance.parser_config_sha256,
            "source snapshot parser configuration digest",
        )?;
        validate_sha256(
            &self.provenance.trust_policy_sha256,
            "source snapshot trust policy digest",
        )?;
        self.provenance.validate()?;
        self.authenticated_root
            .validate("authenticated source root SHA-256")?;
        self.catalog.validate("source catalog SHA-256")?;
        validate_sha256(
            &self.logical_digest_sha256,
            "source snapshot logical digest",
        )?;

        let mut previous_key = None;
        let mut roles = BTreeSet::new();
        for object in &self.authenticated_objects {
            object.validate()?;
            let key = (&object.role, object.source_path.as_str());
            if previous_key
                .as_ref()
                .is_some_and(|previous| previous >= &key)
            {
                return Err(Error::ConfigError(
                    "source snapshot authenticated objects must be strictly ordered by role and source path"
                        .to_string(),
                ));
            }
            if !roles.insert(&object.role) {
                return Err(Error::ConfigError(format!(
                    "source snapshot repeats authenticated object role {:?}",
                    object.role
                )));
            }
            previous_key = Some(key);
        }
        Ok(())
    }

    pub fn manifest_sha256(&self) -> Result<String> {
        self.validate()?;
        canonical_manifest_sha256(self)
    }
}

impl ProfileRevisionV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PROFILE_REVISION_SCHEMA_V1 {
            return Err(Error::ConfigError(format!(
                "profile revision schema {} is unsupported; expected {}",
                self.schema_version, PROFILE_REVISION_SCHEMA_V1
            )));
        }
        validate_storage_component(&self.profile, "profile revision profile")?;
        if self.projection_version == 0 {
            return Err(Error::ConfigError(
                "profile revision projection version must be positive".to_string(),
            ));
        }
        if self.members.is_empty() {
            return Err(Error::ConfigError(
                "profile revision must contain at least one source member".to_string(),
            ));
        }
        self.catalog.validate("profile catalog SHA-256")?;
        validate_sha256(
            &self.logical_digest_sha256,
            "profile revision logical digest",
        )?;

        let mut repository_identities = BTreeSet::new();
        for (index, member) in self.members.iter().enumerate() {
            let expected_ordinal = u32::try_from(index).map_err(|_| {
                Error::ConfigError("profile revision contains too many source members".to_string())
            })?;
            if member.ordinal != expected_ordinal {
                return Err(Error::ConfigError(format!(
                    "profile revision member ordinal {} is noncanonical; expected {}",
                    member.ordinal, expected_ordinal
                )));
            }
            validate_identity(
                &member.repository_identity,
                "profile revision repository identity",
            )?;
            validate_identity(&member.source_identity, "profile revision source identity")?;
            validate_identity(&member.stream.identity, "profile revision stream identity")?;
            if !repository_identities.insert(&member.repository_identity) {
                return Err(Error::ConfigError(format!(
                    "profile revision repeats repository identity '{}'",
                    member.repository_identity
                )));
            }
            validate_sha256(
                &member.source_snapshot_sha256,
                "profile member source snapshot SHA-256",
            )?;
        }
        Ok(())
    }

    pub fn manifest_sha256(&self) -> Result<String> {
        self.validate()?;
        canonical_manifest_sha256(self)
    }
}

fn canonical_manifest_sha256(value: &impl Serialize) -> Result<String> {
    let bytes = crate::json::canonical_json(value).map_err(|error| {
        Error::ParseError(format!(
            "failed to serialize immutable catalog manifest: {error}"
        ))
    })?;
    Ok(crate::hash::sha256(&bytes))
}

impl SourceProvenanceV1 {
    fn validate(&self) -> Result<()> {
        self.parser_config.validate()?;
        self.trust_policy.validate()?;
        let parser_format = self.parser_config.format();
        let trust_format = self.trust_policy.format();
        let ecosystem_format = match self.ecosystem {
            SourceEcosystemV1::Rpm => RepositoryFormat::Fedora,
            SourceEcosystemV1::Deb => RepositoryFormat::Debian,
            SourceEcosystemV1::Alpm => RepositoryFormat::Arch,
            SourceEcosystemV1::Eopkg => RepositoryFormat::Eopkg,
        };
        if parser_format != ecosystem_format || trust_format != ecosystem_format {
            return Err(Error::ConfigError(format!(
                "source provenance ecosystem {:?}, parser '{}', and trust '{}' must describe one native format",
                self.ecosystem,
                parser_format.as_str(),
                trust_format.as_str()
            )));
        }
        validate_source_url(&self.metadata_url, "source metadata URL")?;
        if let Some(content_url) = &self.content_url {
            validate_source_url(content_url, "source content URL")?;
        }
        let parser_digest = canonical_value_sha256(&self.parser_config)?;
        if parser_digest != self.parser_config_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: parser_digest,
                actual: self.parser_config_sha256.clone(),
            });
        }
        let trust_digest = canonical_value_sha256(&self.trust_policy)?;
        if trust_digest != self.trust_policy_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: trust_digest,
                actual: self.trust_policy_sha256.clone(),
            });
        }
        Ok(())
    }
}

fn canonical_value_sha256(value: &impl Serialize) -> Result<String> {
    let bytes = crate::json::canonical_json(value).map_err(|error| {
        Error::ParseError(format!("serialize source provenance authority: {error}"))
    })?;
    Ok(crate::hash::sha256(&bytes))
}

fn validate_source_url(value: &str, label: &str) -> Result<()> {
    let parsed = url::Url::parse(value)
        .map_err(|error| Error::ConfigError(format!("{label} is invalid: {error}")))?;
    if !matches!(parsed.scheme(), "https" | "http" | "file") {
        return Err(Error::ConfigError(format!(
            "{label} must use an explicit https, http, or file scheme"
        )));
    }
    if matches!(parsed.scheme(), "https" | "http") && parsed.host_str().is_none() {
        return Err(Error::ConfigError(format!("{label} has no host")));
    }
    Ok(())
}

pub(super) fn validate_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must contain 1 to 255 printable ASCII characters without surrounding whitespace"
        )));
    }
    Ok(())
}

pub(super) fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

pub(super) fn validate_storage_component(value: &str, label: &str) -> Result<()> {
    validate_identity(value, label)?;
    if value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be a safe ASCII storage-path component"
        )));
    }
    Ok(())
}

pub(super) fn validate_relative_source_path(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 4096 || value.starts_with('/') || value.contains('\\') {
        return Err(Error::ConfigError(
            "authenticated metadata source path must be a non-empty relative slash path"
                .to_string(),
        ));
    }
    if value.split('/').any(|component| {
        component.is_empty()
            || component == "."
            || component == ".."
            || !component.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    }) {
        return Err(Error::ConfigError(
            "authenticated metadata source path contains a noncanonical component".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn artifact(byte: char, size: u64) -> CatalogArtifactV1 {
        CatalogArtifactV1 {
            sha256: digest(byte),
            size,
        }
    }

    fn source_snapshot() -> SourceSnapshotV1 {
        let parser_config = RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        };
        let trust_policy = RepositoryTrustPolicy::Rpm {
            metadata: crate::repository::RpmMetadataAuthority::Metalink {
                url: "https://example.test/metalink".to_string(),
            },
            package_keys: vec![
                crate::repository::OpenPgpTrustRoot::new(
                    "https://example.test/fedora.gpg".to_string(),
                    "A".repeat(40),
                )
                .unwrap(),
            ],
        };
        SourceSnapshotV1 {
            schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-44-everything-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            stream_binding_sha256: digest('a'),
            parser_projection_version: 1,
            provenance: SourceProvenanceV1 {
                ecosystem: SourceEcosystemV1::Rpm,
                metadata_url: "https://example.test/repository".to_string(),
                content_url: Some("https://content.example.test/repository".to_string()),
                parser_config_sha256: canonical_value_sha256(&parser_config).unwrap(),
                parser_config,
                trust_policy_sha256: canonical_value_sha256(&trust_policy).unwrap(),
                trust_policy,
            },
            authenticated_root: artifact('b', 128),
            authenticated_objects: vec![
                SourceMetadataObjectV1 {
                    role: SourceMetadataObjectRoleV1::RpmPrimary,
                    source_path: "repodata/primary.xml.zst".to_string(),
                    sha256: digest('c'),
                    size: 256,
                },
                SourceMetadataObjectV1 {
                    role: SourceMetadataObjectRoleV1::RpmFilelists,
                    source_path: "repodata/filelists.xml.zst".to_string(),
                    sha256: digest('d'),
                    size: 512,
                },
            ],
            catalog: artifact('e', 4096),
            logical_digest_sha256: digest('f'),
            counts: CatalogCountsV1 {
                packages: 2,
                provides: 8,
                requirement_groups: 3,
                requirement_atoms: 4,
                source_evidence: 2,
            },
        }
    }

    #[test]
    fn source_snapshot_is_strict_and_content_addressed() {
        let snapshot = source_snapshot();
        snapshot.validate().unwrap();
        let first = snapshot.manifest_sha256().unwrap();
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: SourceSnapshotV1 = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.manifest_sha256().unwrap(), first);

        let mut changed = snapshot;
        changed.counts.provides += 1;
        assert_ne!(changed.manifest_sha256().unwrap(), first);

        let unknown = br#"{"schema_version":1,"extra":true}"#;
        assert!(serde_json::from_slice::<SourceSnapshotV1>(unknown).is_err());
    }

    #[test]
    fn source_snapshot_rejects_noncanonical_object_authority() {
        let mut snapshot = source_snapshot();
        snapshot.authenticated_objects.swap(0, 1);
        assert!(snapshot.validate().is_err());

        let mut snapshot = source_snapshot();
        snapshot.authenticated_objects[0].source_path = "../primary.xml".to_string();
        assert!(snapshot.validate().is_err());

        let mut snapshot = source_snapshot();
        snapshot.authenticated_objects[1].role = SourceMetadataObjectRoleV1::RpmPrimary;
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn profile_revision_requires_declared_member_order_and_unique_identity() {
        let snapshot_id = source_snapshot().manifest_sha256().unwrap();
        let mut revision = ProfileRevisionV1 {
            schema_version: PROFILE_REVISION_SCHEMA_V1,
            profile: "arch".to_string(),
            projection_version: 1,
            members: vec![
                ProfileSourceMemberV1 {
                    ordinal: 0,
                    source_identity: "archlinux".to_string(),
                    repository_identity: "arch-core-x86_64".to_string(),
                    stream: SourceStreamV1 {
                        kind: SourceStreamKindV1::Rolling,
                        identity: "archlinux".to_string(),
                    },
                    priority: 80,
                    required: true,
                    source_snapshot_sha256: snapshot_id.clone(),
                },
                ProfileSourceMemberV1 {
                    ordinal: 1,
                    source_identity: "archlinux".to_string(),
                    repository_identity: "arch-extra-x86_64".to_string(),
                    stream: SourceStreamV1 {
                        kind: SourceStreamKindV1::Rolling,
                        identity: "archlinux".to_string(),
                    },
                    priority: 70,
                    required: true,
                    source_snapshot_sha256: snapshot_id,
                },
            ],
            catalog: artifact('1', 8192),
            logical_digest_sha256: digest('2'),
            counts: CatalogCountsV1 {
                packages: 3,
                ..CatalogCountsV1::default()
            },
        };
        revision.validate().unwrap();
        let first = revision.manifest_sha256().unwrap();
        revision.members[1].ordinal = 2;
        assert!(revision.validate().is_err());
        revision.members[1].ordinal = 1;
        revision.members[1].repository_identity = "arch-core-x86_64".to_string();
        assert!(revision.validate().is_err());
        revision.members[1].repository_identity = "arch-extra-x86_64".to_string();
        revision.members[1].priority = 60;
        assert_ne!(revision.manifest_sha256().unwrap(), first);
    }
}
