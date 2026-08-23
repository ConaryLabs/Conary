// crates/conary-core/src/repository/catalog/parity/resolution_contract.rs

//! Strict native dependency-resolution parity contracts.

use serde::{Deserialize, Serialize};

use super::super::contract::{validate_identity, validate_sha256};
use super::super::{ProfileRevisionV2, ProfileSourceMemberV2};
use super::contract::{NativeParityImplementationV1, NativeParityOracleV1};
use crate::error::{Error, Result};

pub const NATIVE_RESOLUTION_ORACLE_SCHEMA_V1: u32 = 1;

/// The fixed solver policy whose output may become release evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionPolicyV1 {
    pub architecture: String,
    pub installed_state: NativeResolutionInstalledStateV1,
    pub roots: NativeResolutionRootPolicyV1,
    pub positive_requirements: NativeResolutionRequirementPolicyV1,
    pub provider_selection: NativeResolutionProviderPolicyV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionInstalledStateV1 {
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionRootPolicyV1 {
    EveryExactPackage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionRequirementPolicyV1 {
    RequiredOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeResolutionProviderPolicyV1 {
    NativePrecedence,
}

impl NativeResolutionPolicyV1 {
    pub fn validate(&self) -> Result<()> {
        validate_identity(&self.architecture, "native resolution architecture")
    }
}

/// Counts for one complete per-root resolution stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionCountsV1 {
    pub roots: u64,
    pub resolved_roots: u64,
    pub unresolved_roots: u64,
    pub closure_package_references: u64,
    pub unresolved_dependencies: u64,
}

/// Exact content-addressed line-oriented resolution artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionArtifactV1 {
    pub sha256: String,
    pub size: u64,
    pub counts: NativeResolutionCountsV1,
}

impl NativeResolutionArtifactV1 {
    fn validate(&self) -> Result<()> {
        validate_sha256(&self.sha256, "native resolution artifact SHA-256")?;
        if self.counts.roots
            != self
                .counts
                .resolved_roots
                .checked_add(self.counts.unresolved_roots)
                .ok_or_else(|| {
                    Error::ConfigError(
                        "native resolution root outcome counts exceed u64".to_string(),
                    )
                })?
        {
            return Err(Error::ConfigError(
                "native resolution root counts do not match outcome counts".to_string(),
            ));
        }
        Ok(())
    }
}

/// Strict manifest binding native solver output to one exact package oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionOracleV1 {
    pub schema_version: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub profile_logical_digest_sha256: String,
    pub members: Vec<ProfileSourceMemberV2>,
    pub package_oracle_manifest_sha256: String,
    pub implementation: NativeParityImplementationV1,
    pub policy: NativeResolutionPolicyV1,
    pub artifact: NativeResolutionArtifactV1,
}

impl NativeResolutionOracleV1 {
    pub fn bind(
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleV1,
        implementation: NativeParityImplementationV1,
        policy: NativeResolutionPolicyV1,
        artifact: NativeResolutionArtifactV1,
    ) -> Result<Self> {
        package_oracle.validate_profile(profile)?;
        if package_oracle.implementation.ecosystem != implementation.ecosystem {
            return Err(Error::ConfigError(
                "native resolution and package oracles use different ecosystems".to_string(),
            ));
        }
        let manifest = Self {
            schema_version: NATIVE_RESOLUTION_ORACLE_SCHEMA_V1,
            profile: profile.profile.clone(),
            profile_revision_sha256: profile.manifest_sha256()?,
            profile_logical_digest_sha256: profile.logical_digest_sha256.clone(),
            members: profile.members.clone(),
            package_oracle_manifest_sha256: package_oracle.manifest_sha256()?,
            implementation,
            policy,
            artifact,
        };
        manifest.validate_binding(profile, package_oracle)?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != NATIVE_RESOLUTION_ORACLE_SCHEMA_V1 {
            return Err(Error::ConfigError(format!(
                "native resolution oracle schema {} is unsupported; expected {}",
                self.schema_version, NATIVE_RESOLUTION_ORACLE_SCHEMA_V1
            )));
        }
        validate_identity(&self.profile, "native resolution profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "native resolution profile revision SHA-256",
        )?;
        validate_sha256(
            &self.profile_logical_digest_sha256,
            "native resolution profile logical digest",
        )?;
        validate_sha256(
            &self.package_oracle_manifest_sha256,
            "native resolution package oracle manifest SHA-256",
        )?;
        self.implementation.validate()?;
        self.policy.validate()?;
        self.artifact.validate()?;
        super::contract::validate_members(&self.members)
    }

    pub fn validate_binding(
        &self,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleV1,
    ) -> Result<()> {
        self.validate()?;
        package_oracle.validate_profile(profile)?;
        if self.profile != profile.profile
            || self.profile_revision_sha256 != profile.manifest_sha256()?
            || self.profile_logical_digest_sha256 != profile.logical_digest_sha256
            || self.members != profile.members
            || self.package_oracle_manifest_sha256 != package_oracle.manifest_sha256()?
            || self.implementation.ecosystem != package_oracle.implementation.ecosystem
        {
            return Err(Error::ConflictError(format!(
                "native resolution oracle does not bind exact package oracle for profile '{}'",
                profile.profile
            )));
        }
        Ok(())
    }

    pub fn manifest_sha256(&self) -> Result<String> {
        self.validate()?;
        let bytes = crate::json::canonical_json(self).map_err(|error| {
            Error::ParseError(format!("serialize native resolution manifest: {error}"))
        })?;
        Ok(crate::hash::sha256(&bytes))
    }
}

/// One unresolved typed requirement reached while resolving an exact root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeUnresolvedDependencyV1 {
    pub requiring_package_key_sha256: String,
    pub requirement_group_sha256: String,
}

impl NativeUnresolvedDependencyV1 {
    fn validate(&self) -> Result<()> {
        validate_sha256(
            &self.requiring_package_key_sha256,
            "native resolution requiring package key",
        )?;
        validate_sha256(
            &self.requirement_group_sha256,
            "native resolution requirement group digest",
        )
    }
}

/// Exact native solver outcome for one exact root package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeResolutionOutcomeV1 {
    Resolved {
        closure_package_keys_sha256: Vec<String>,
    },
    Unresolved {
        dependencies: Vec<NativeUnresolvedDependencyV1>,
    },
}

/// One canonical row for every package in the bound package oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResolutionRootV1 {
    pub root_package_key_sha256: String,
    pub outcome: NativeResolutionOutcomeV1,
}

impl NativeResolutionRootV1 {
    pub fn validate(&self) -> Result<()> {
        validate_sha256(
            &self.root_package_key_sha256,
            "native resolution root package key",
        )?;
        match &self.outcome {
            NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256,
            } => {
                if closure_package_keys_sha256.is_empty() {
                    return Err(Error::ConfigError(
                        "native resolution closure cannot be empty".to_string(),
                    ));
                }
                validate_strict_sha256_order(
                    closure_package_keys_sha256,
                    "native resolution closure",
                )?;
                if closure_package_keys_sha256
                    .binary_search(&self.root_package_key_sha256)
                    .is_err()
                {
                    return Err(Error::ConfigError(
                        "native resolution closure does not contain its exact root".to_string(),
                    ));
                }
            }
            NativeResolutionOutcomeV1::Unresolved { dependencies } => {
                if dependencies.is_empty() {
                    return Err(Error::ConfigError(
                        "native unresolved outcome cannot be empty".to_string(),
                    ));
                }
                let mut previous = None;
                for dependency in dependencies {
                    dependency.validate()?;
                    if previous.is_some_and(|value| value >= dependency) {
                        return Err(Error::ConfigError(
                            "native unresolved dependencies are duplicated or noncanonical"
                                .to_string(),
                        ));
                    }
                    previous = Some(dependency);
                }
            }
        }
        Ok(())
    }
}

fn validate_strict_sha256_order(values: &[String], label: &str) -> Result<()> {
    let mut previous = None;
    for value in values {
        validate_sha256(value, label)?;
        if previous.is_some_and(|previous| previous >= value.as_str()) {
            return Err(Error::ConfigError(format!(
                "{label} package keys are duplicated or noncanonical"
            )));
        }
        previous = Some(value.as_str());
    }
    Ok(())
}
