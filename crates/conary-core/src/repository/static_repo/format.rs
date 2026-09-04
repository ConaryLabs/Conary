// crates/conary-core/src/repository/static_repo/format.rs

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::VerifyingKey;

use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, ProvidedCapability, RepositoryCapabilityKind,
    RepositoryProvide, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::versioning::{VersionScheme, parse_repo_constraint};

use super::paths::validate_repo_relative_path;

/// Document major for every static repository document a client parses.
///
/// Major 2 carries capability provenance without its duplicated path. A major-1
/// index cannot be read as major 2 and a major-1 client cannot read a major-2
/// index -- the provenance shapes are incompatible in both directions -- so the
/// major is what clients gate on, exactly as the format specification requires.
/// The publisher stamps this constant rather than a literal so a published
/// document and the parser that admits it cannot drift apart.
pub const SCHEMA_VERSION: u64 = 2;
const SHA256_HEX_LEN: usize = 64;
const ED25519_PUBLIC_KEY_LEN: usize = 32;
const MAX_REPO_NAME_LEN: usize = 64;

impl RepoIdentity {
    pub fn parse(input: &str) -> Result<Self> {
        let parsed: Self = toml::from_str(input)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<()> {
        validate_schema(self.schema, "repo identity")?;
        validate_repo_name(&self.repo.name, "repo.name")?;

        if self.trust.root_key_ids.is_empty() {
            bail!("repo identity trust.root_key_ids must not be empty");
        }

        for key_id in &self.trust.root_key_ids {
            validate_lower_hex(key_id, SHA256_HEX_LEN, "trust.root_key_ids")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoIdentity {
    pub schema: u64,
    pub repo: RepoIdentityRepo,
    pub trust: RepoIdentityTrust,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoIdentityRepo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoIdentityTrust {
    pub root_key_ids: Vec<String>,
}

impl StaticIndex {
    pub fn parse(input: &str) -> Result<Self> {
        let parsed: Self = serde_json::from_str(input)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<()> {
        validate_schema(self.schema, "static index")?;
        validate_repo_name(&self.name, "index.name")?;

        let mut seen_name_version_release_arch = HashSet::new();
        for package in &self.packages {
            package.validate()?;

            let identity = (
                package.name.as_str(),
                package.version.as_str(),
                package.release.as_str(),
                package.arch.as_str(),
            );
            if !seen_name_version_release_arch.insert(identity) {
                bail!(
                    "duplicate static package identity {}-{}-{}-{}",
                    package.name,
                    package.version,
                    package.release,
                    package.arch
                );
            }
        }

        Ok(())
    }

    pub fn validate_with_keys(&self, keys: &PackageKeysFile) -> Result<()> {
        keys.validate_for_index(self)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StaticIndex {
    pub schema: u64,
    pub name: String,
    pub index_version: u64,
    pub generated: chrono::DateTime<chrono::Utc>,
    pub packages: Vec<StaticPackageEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StaticPackageEntry {
    pub name: String,
    pub version: String,
    pub version_scheme: VersionScheme,
    pub release: String,
    pub arch: String,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub description: Option<String>,
    /// Exact positive capability contracts declared by the package.
    pub provides: Vec<RepositoryProvide>,
    /// Authoritative typed positive requirement expressions.
    pub requirements: Vec<RepositoryRequirementGroup>,
    /// Authoritative typed negative and replacement relation expressions.
    pub relations: Vec<RepositoryRequirementGroup>,
}

impl StaticPackageEntry {
    pub fn validate(&self) -> Result<()> {
        validate_non_empty(&self.name, "package.name")?;
        validate_non_empty(&self.version, "package.version")?;
        crate::repository::versioning::validate_repo_version(self.version_scheme, &self.version)
            .context("package.version does not satisfy package.version_scheme")?;
        crate::repository::versioning::validate_package_release(&self.release)
            .context("package.release is not a positive CCS release number")?;
        validate_non_empty(&self.arch, "package.arch")?;
        validate_non_empty(&self.path, "package.path")?;
        validate_lower_hex(&self.sha256, SHA256_HEX_LEN, "package.sha256")?;
        validate_repo_relative_path(&self.path)?;

        if self.size > i64::MAX as u64 {
            bail!("package.size {} exceeds i64::MAX", self.size);
        }

        self.validate_provides()?;
        self.validate_requirements()?;
        self.validate_relations()?;

        let expected_prefix = format!("packages/{}/", self.name);
        if !self.path.starts_with(&expected_prefix) {
            bail!("package path must start with `{expected_prefix}`");
        }

        let expected_filename = format!(
            "{}-{}-{}-{}.ccs",
            self.name, self.version, self.release, self.arch
        );
        let actual_filename = self.path.rsplit('/').next().unwrap_or("");
        if actual_filename != expected_filename {
            bail!(
                "package path filename `{actual_filename}` does not match expected `{expected_filename}`"
            );
        }

        Ok(())
    }

    fn validate_provides(&self) -> Result<()> {
        let mut seen = HashSet::new();
        let mut exact_identities = 0;
        for provide in &self.provides {
            ProvidedCapability {
                kind: provide.kind,
                name: provide.name.clone(),
                version: provide.version.clone(),
                version_relation: provide.version_relation,
                version_scheme: self.version_scheme,
                architecture_qualifier: provide.architecture_qualifier.clone(),
                provenance: provide.provenance.clone(),
            }
            .validate()
            .context("package.provides[] violates the shared capability grammar")?;
            if !seen.insert((
                provide.kind,
                provide.name.as_str(),
                provide.version.as_deref(),
                provide.version_relation,
                &provide.architecture_qualifier,
                &provide.provenance,
            )) {
                bail!(
                    "duplicate package provide {:?} {} {:?}",
                    provide.kind,
                    provide.name,
                    provide.version
                );
            }
            if provide.provenance
                == crate::repository::dependency_model::CapabilityProvenance::ExactIdentity
            {
                if provide.kind != RepositoryCapabilityKind::PackageName
                    || provide.name != self.name
                    || provide.version.as_deref() != Some(self.version.as_str())
                    || provide.version_relation
                        != Some(
                            crate::repository::dependency_model::ProvideVersionRelation::Equal,
                        )
                    || provide.architecture_qualifier
                        != crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit
                {
                    bail!(
                        "exact package identity provider does not match package '{}-{}'",
                        self.name,
                        self.version
                    );
                }
                exact_identities += 1;
            }
        }
        if exact_identities != 1 {
            bail!(
                "package '{}' must declare exactly one matching exact package identity",
                self.name
            );
        }
        Ok(())
    }

    fn validate_requirements(&self) -> Result<()> {
        for group in &self.requirements {
            if matches!(
                group.kind,
                RepositoryRequirementKind::Conflict
                    | RepositoryRequirementKind::Breaks
                    | RepositoryRequirementKind::Replace
                    | RepositoryRequirementKind::Obsolete
            ) {
                bail!(
                    "package '{}' static index contains unsupported negative relation {:?}",
                    self.name,
                    group.kind
                );
            }
            if group.alternatives.is_empty() {
                bail!(
                    "package '{}' requirement group must contain at least one clause",
                    self.name
                );
            }
            let expression_atoms = group.expression.atoms();
            if expression_atoms.len() != group.alternatives.len()
                || expression_atoms
                    .iter()
                    .zip(&group.alternatives)
                    .any(|(expression, indexed)| *expression != indexed)
            {
                bail!(
                    "package '{}' requirement clause index disagrees with its authoritative expression",
                    self.name
                );
            }
            let expected_behavior = if group.expression.is_conditional() {
                ConditionalRequirementBehavior::Conditional
            } else {
                ConditionalRequirementBehavior::Hard
            };
            if group.behavior != expected_behavior {
                bail!(
                    "package '{}' requirement behavior {:?} disagrees with its expression",
                    self.name,
                    group.behavior
                );
            }
            for clause in expression_atoms {
                validate_non_empty(&clause.name, "package.requirements[].expression.name")?;
                if let Some(constraint) = clause.version_constraint.as_deref() {
                    parse_repo_constraint(self.version_scheme, constraint).with_context(|| {
                        format!(
                            "package '{}' requirement '{}' has invalid {:?} constraint '{}'",
                            self.name, clause.name, self.version_scheme, constraint
                        )
                    })?;
                }
            }
        }
        Ok(())
    }

    fn validate_relations(&self) -> Result<()> {
        for relation in &self.relations {
            crate::repository::package_relation::validate_native_relation(
                relation,
                self.version_scheme,
            )
            .map_err(|error| {
                anyhow!(
                    "package '{}' has invalid relation authority: {error}",
                    self.name
                )
            })?;
        }
        Ok(())
    }
}

impl PackageKeysFile {
    pub fn parse(input: &str) -> Result<Self> {
        let parsed: Self = serde_json::from_str(input)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<()> {
        validate_schema(self.schema, "package keys")?;

        for key in &self.keys {
            key.validate()?;
        }

        Ok(())
    }

    pub fn validate_for_index(&self, index: &StaticIndex) -> Result<()> {
        self.validate()?;
        index.validate()?;

        if !index.packages.is_empty() && self.keys.is_empty() {
            bail!("package keys must not be empty for a non-empty static index");
        }

        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageKeysFile {
    pub schema: u64,
    pub keys: Vec<PackageKeyEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKeyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageKeyEntry {
    pub algorithm: String,
    pub public_key: String,
    #[serde(default)]
    pub key_id: Option<String>,
    pub status: PackageKeyStatus,
    #[serde(default)]
    pub comment: Option<String>,
}

impl PackageKeyEntry {
    pub fn validate(&self) -> Result<()> {
        if self.algorithm != "ed25519" {
            bail!(
                "package key algorithm `{}` is unsupported; expected ed25519",
                self.algorithm
            );
        }

        let decoded = BASE64
            .decode(&self.public_key)
            .map_err(|error| anyhow!("package key public_key is not valid base64: {error}"))?;
        if decoded.len() != ED25519_PUBLIC_KEY_LEN {
            bail!(
                "package key public_key decoded to {} bytes; expected {ED25519_PUBLIC_KEY_LEN}",
                decoded.len()
            );
        }

        let key_bytes: [u8; ED25519_PUBLIC_KEY_LEN] = decoded
            .try_into()
            .map_err(|_| anyhow!("package key public_key must be 32 bytes"))?;
        VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
            anyhow!("package key public_key is not a valid Ed25519 key: {error}")
        })?;

        Ok(())
    }
}

fn validate_schema(schema: u64, document: &str) -> Result<()> {
    if schema != SCHEMA_VERSION {
        bail!("{document} schema {schema} is unsupported; expected {SCHEMA_VERSION}");
    }

    Ok(())
}

fn validate_repo_name(name: &str, field: &str) -> Result<()> {
    validate_non_empty(name, field)?;

    if name.len() > MAX_REPO_NAME_LEN {
        bail!("{field} must be at most {MAX_REPO_NAME_LEN} bytes");
    }

    let mut bytes = name.bytes();
    let first = bytes.next().expect("name is non-empty");
    if !matches!(first, b'a'..=b'z' | b'0'..=b'9') {
        bail!("{field} must start with a lowercase ASCII letter or digit");
    }

    if !bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-')) {
        bail!("{field} must contain only lowercase ASCII letters, digits, or hyphens");
    }

    Ok(())
}

fn validate_non_empty(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{field} must not be empty");
    }

    Ok(())
}

fn validate_lower_hex(value: &str, expected_len: usize, field: &str) -> Result<()> {
    if value.len() != expected_len {
        bail!("{field} must be {expected_len} lowercase hex characters");
    }

    if !value
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        bail!("{field} must be lowercase hex");
    }

    Ok(())
}

#[cfg(test)]
mod tests;
