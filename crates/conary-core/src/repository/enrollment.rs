// conary-core/src/repository/enrollment.rs

//! Shared repository authority and signed package-enrollment intents.

use crate::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
    RepositoryOwnership, RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
};
use crate::error::{Error, Result};
use crate::repository::{RepositoryParserConfig, RepositoryTrustPolicy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

pub const PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RepositoryPolicyScopeInput {
    Repository { identity: String },
    Group { identity: String },
}

impl RepositoryPolicyScopeInput {
    fn materialize(&self) -> Result<RepositoryPolicyScope> {
        match self {
            Self::Repository { identity } => RepositoryPolicyScope::repository(identity.clone()),
            Self::Group { identity } => RepositoryPolicyScope::group(identity.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RepositorySourceStreamInput {
    Release { identity: String },
    Channel { identity: String },
    Rolling { identity: String },
}

impl RepositorySourceStreamInput {
    fn materialize(&self) -> Result<NativeSourceStream> {
        match self {
            Self::Release { identity } => NativeSourceStream::release(identity.clone()),
            Self::Channel { identity } => NativeSourceStream::channel(identity.clone()),
            Self::Rolling { identity } => NativeSourceStream::rolling(identity.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RepositoryUpdatePolicyInput {
    Follow,
    Pin { snapshot_sha256: String },
}

/// One complete repository definition independent of its declaration source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryDefinition {
    pub name: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub scope: RepositoryPolicyScopeInput,
    pub stream: RepositorySourceStreamInput,
    pub update: RepositoryUpdatePolicyInput,
    pub metadata_url: String,
    pub content_url: Option<String>,
    pub parser: RepositoryParserConfig,
    pub trust: RepositoryTrustPolicy,
    pub enabled: bool,
    pub priority: i32,
    pub metadata_expire: i32,
    pub source_profile: Option<String>,
}

impl RepositoryDefinition {
    pub fn materialize(&self) -> Result<Repository> {
        let ecosystem = NativeSourceEcosystem::from_repository_format(self.parser.format())?;
        let (mode, pin) = match &self.update {
            RepositoryUpdatePolicyInput::Follow => (RepositoryUpdateMode::Follow, None),
            RepositoryUpdatePolicyInput::Pin { snapshot_sha256 } => (
                RepositoryUpdateMode::Pin,
                Some(AuthenticatedSnapshotIdentity::from_sha256(
                    snapshot_sha256.clone(),
                )?),
            ),
        };
        let policy = RepositorySourcePolicy::new(
            self.source_identity.clone(),
            self.scope.materialize()?,
            ecosystem,
            self.stream.materialize()?,
            mode,
        )?;
        if self.trust.format() != self.parser.format() {
            return Err(Error::ConfigError(format!(
                "repository '{}' parser format '{}' disagrees with trust format '{}'",
                self.name,
                self.parser.format().as_str(),
                self.trust.format().as_str()
            )));
        }
        let mut repository = Repository::new(self.name.clone(), self.metadata_url.clone());
        repository.content_url = self.content_url.clone();
        repository.enabled = self.enabled;
        repository.priority = self.priority;
        repository.metadata_expire = self.metadata_expire;
        repository.source_profile = self.source_profile.clone();
        repository.managed_by = RepositoryOwnership::NativeProjection;
        repository.set_parser_config(self.parser.clone())?;
        repository.set_trust_policy(self.trust.clone())?;
        repository.set_native_source_policy(policy, self.repository_identity.clone(), pin)?;
        Ok(repository)
    }

    pub fn identity_key(&self) -> (&str, &str) {
        (&self.source_identity, &self.repository_identity)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageProjectionRole {
    Declaration,
    TrustRoot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageProjectionReference {
    pub path: String,
    pub sha256: String,
    pub mode: u32,
    pub role: PackageProjectionRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LastOwnerDisposition {
    RemoveWhenUnowned,
    Retain,
}

/// Signed desired state emitted before a package transaction mutates its root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRepositoryEnrollmentIntent {
    pub schema: u32,
    pub intent_id: String,
    pub repositories: Vec<RepositoryDefinition>,
    pub projections: Vec<PackageProjectionReference>,
    pub last_owner: LastOwnerDisposition,
}

impl PackageRepositoryEnrollmentIntent {
    pub fn validate(&self) -> Result<()> {
        if self.schema != PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA {
            return Err(Error::ConfigError(format!(
                "package repository enrollment schema {} is unsupported; expected {}",
                self.schema, PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA
            )));
        }
        require_identity("intent_id", &self.intent_id)?;
        if self.repositories.is_empty() {
            return Err(Error::ConfigError(format!(
                "package repository enrollment '{}' has no repositories",
                self.intent_id
            )));
        }
        if self.projections.is_empty() {
            return Err(Error::ConfigError(format!(
                "package repository enrollment '{}' has no projections",
                self.intent_id
            )));
        }

        let mut names = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for definition in &self.repositories {
            definition.materialize()?;
            if !names.insert(definition.name.as_str()) {
                return Err(Error::ConfigError(format!(
                    "package repository enrollment '{}' repeats repository name '{}'",
                    self.intent_id, definition.name
                )));
            }
            if !identities.insert(definition.identity_key()) {
                return Err(Error::ConfigError(format!(
                    "package repository enrollment '{}' repeats repository identity '{}'",
                    self.intent_id, definition.repository_identity
                )));
            }
        }

        let mut paths = BTreeSet::new();
        for projection in &self.projections {
            validate_guest_path(&projection.path)?;
            validate_sha256("projection", &projection.sha256)?;
            if projection.mode > 0o7777 {
                return Err(Error::ConfigError(format!(
                    "package repository projection '{}' mode {:o} exceeds 07777",
                    projection.path, projection.mode
                )));
            }
            if !paths.insert(projection.path.as_str()) {
                return Err(Error::ConfigError(format!(
                    "package repository enrollment '{}' repeats projection '{}'",
                    self.intent_id, projection.path
                )));
            }
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| {
            Error::ParseError(format!(
                "failed to serialize package repository enrollment '{}': {error}",
                self.intent_id
            ))
        })?;
        Ok(crate::hash::sha256(&bytes))
    }
}

fn require_identity(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 255 || value.trim() != value {
        return Err(Error::ConfigError(format!(
            "package repository enrollment {field} must be 1 to 255 trimmed bytes"
        )));
    }
    Ok(())
}

fn validate_guest_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
        || path == Path::new("/")
    {
        return Err(Error::ConfigError(format!(
            "package repository projection path '{value}' is not a normalized absolute guest path"
        )));
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    {
        return Err(Error::ConfigError(format!(
            "package repository {label} SHA-256 must be 64 lowercase hexadecimal digits"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::{OpenPgpTrustRoot, RepositoryParserConfig, RpmMetadataAuthority};

    fn definition() -> RepositoryDefinition {
        let root = OpenPgpTrustRoot {
            url: "data:application/pgp-keys;base64,Y2VydGlmaWNhdGU=".to_string(),
            fingerprint: "A".repeat(40),
        };
        RepositoryDefinition {
            name: "vendor-browser".to_string(),
            source_identity: "vendor-browser-rpm".to_string(),
            repository_identity: "vendor-browser-stable".to_string(),
            scope: RepositoryPolicyScopeInput::Repository {
                identity: "vendor-browser-stable".to_string(),
            },
            stream: RepositorySourceStreamInput::Channel {
                identity: "stable".to_string(),
            },
            update: RepositoryUpdatePolicyInput::Follow,
            metadata_url: "https://packages.example.test/rpm/stable/x86_64".to_string(),
            content_url: None,
            parser: RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            },
            trust: RepositoryTrustPolicy::Rpm {
                metadata: RpmMetadataAuthority::OpenPgp {
                    keys: vec![root.clone()],
                },
                package_keys: vec![root],
            },
            enabled: true,
            priority: 0,
            metadata_expire: 3600,
            source_profile: None,
        }
    }

    fn intent() -> PackageRepositoryEnrollmentIntent {
        PackageRepositoryEnrollmentIntent {
            schema: PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA,
            intent_id: "vendor-browser-repository".to_string(),
            repositories: vec![definition()],
            projections: vec![PackageProjectionReference {
                path: "/etc/yum.repos.d/vendor-browser.repo".to_string(),
                sha256: "a".repeat(64),
                mode: 0o644,
                role: PackageProjectionRole::Declaration,
            }],
            last_owner: LastOwnerDisposition::RemoveWhenUnowned,
        }
    }

    #[test]
    fn signed_intent_is_deterministic_and_closed() {
        let intent = intent();
        assert_eq!(intent.sha256().unwrap(), intent.sha256().unwrap());

        let mut value = serde_json::to_value(&intent).unwrap();
        value["invented"] = serde_json::json!(true);
        assert!(serde_json::from_value::<PackageRepositoryEnrollmentIntent>(value).is_err());
    }

    #[test]
    fn signed_intent_rejects_path_and_identity_ambiguity() {
        let mut invalid = intent();
        invalid.projections[0].path = "/etc/../escape".to_string();
        assert!(invalid.validate().is_err());

        let mut duplicate = intent();
        duplicate
            .repositories
            .push(duplicate.repositories[0].clone());
        assert!(duplicate.validate().is_err());
    }
}
