// conary-core/src/repository/static_repo/format.rs

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::VerifyingKey;

use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryCapabilityKind, RepositoryProvide,
    RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::versioning::{VersionScheme, parse_repo_constraint};

use super::paths::validate_repo_relative_path;

const SCHEMA_VERSION: u64 = 1;
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
        let mut self_provides = 0;
        for provide in &self.provides {
            validate_non_empty(&provide.name, "package.provides[].name")?;
            if provide.version.as_deref().is_some_and(str::is_empty) {
                bail!("package.provides[].version must not be empty");
            }
            if !seen.insert((
                provide.kind,
                provide.name.as_str(),
                provide.version.as_deref(),
            )) {
                bail!(
                    "duplicate package provide {:?} {} {:?}",
                    provide.kind,
                    provide.name,
                    provide.version
                );
            }
            if provide.kind == RepositoryCapabilityKind::PackageName && provide.name == self.name {
                if provide.version.as_deref() != Some(self.version.as_str()) {
                    bail!(
                        "package-name provide '{}' version {:?} does not match package version '{}'",
                        provide.name,
                        provide.version,
                        self.version
                    );
                }
                self_provides += 1;
            }
        }
        if self_provides != 1 {
            bail!(
                "package '{}' must declare exactly one matching package-name self-provide",
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
mod tests {
    use super::{PackageKeysFile, RepoIdentity, StaticIndex};
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    const VALID_ROOT_KEY_ID: &str =
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const VALID_SHA256: &str = "30e14955ebf1352266dc2ff8067e68104607e750abb9d3b36582b8af909fcb58";

    #[test]
    fn repo_identity_rejects_bad_name() {
        let input = r#"
schema = 1
[repo]
name = "Bad_Name"
description = "bad"
[trust]
root_key_ids = ["9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"]
"#;
        assert!(RepoIdentity::parse(input).is_err());
    }

    #[test]
    fn repo_identity_rejects_bad_root_key_id() {
        let input = r#"
schema = 1
[repo]
name = "good-name"
[trust]
root_key_ids = ["not-a-key"]
"#;
        assert!(RepoIdentity::parse(input).is_err());
    }

    #[test]
    fn repo_identity_rejects_unknown_schema() {
        let input = format!(
            r#"
schema = 2
[repo]
name = "good-name"
[trust]
root_key_ids = ["{VALID_ROOT_KEY_ID}"]
"#
        );
        assert!(RepoIdentity::parse(&input).is_err());
    }

    #[test]
    fn static_index_rejects_unknown_schema() {
        let input = valid_index_json(
            2,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        );
        assert!(StaticIndex::parse(&input).is_err());
    }

    #[test]
    fn static_index_rejects_package_filename_mismatch() {
        let input = valid_index_json(
            1,
            "packages/acme-widget/wrong-name-1.4.2-1-x86_64.ccs",
            1048576,
        );
        assert!(StaticIndex::parse(&input).is_err());
    }

    #[test]
    fn static_index_rejects_package_path_outside_name_directory() {
        let input = valid_index_json(1, "packages/other/acme-widget-1.4.2-1-x86_64.ccs", 1048576);
        assert!(StaticIndex::parse(&input).is_err());
    }

    #[test]
    fn static_index_rejects_package_size_above_i64_max() {
        let input = valid_index_json(
            1,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            i64::MAX as u64 + 1,
        );
        assert!(StaticIndex::parse(&input).is_err());
    }

    #[test]
    fn static_index_rejects_duplicate_package_identity() {
        let input = format!(
            r#"{{
  "schema": 1,
  "name": "acme-tools",
  "index_version": 7,
  "generated": "2026-06-10T18:00:00Z",
  "packages": [
    {{
      "name": "acme-widget",
      "version": "1.4.2",
      "version_scheme": "conary",
      "release": "1",
      "arch": "x86_64",
      "path": "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
      "sha256": "{VALID_SHA256}",
      "size": 1048576,
      "provides": [{{
        "name": "acme-widget",
        "kind": "PackageName",
        "version": "1.4.2",
        "architecture_qualifier": {{ "kind": "implicit" }},
        "native_text": null
      }}],
      "requirements": [],

    "relations": []
    }},
    {{
      "name": "acme-widget",
      "version": "1.4.2",
      "version_scheme": "conary",
      "release": "1",
      "arch": "x86_64",
      "path": "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
      "sha256": "{VALID_SHA256}",
      "size": 1048576,
      "provides": [{{
        "name": "acme-widget",
        "kind": "PackageName",
        "version": "1.4.2",
        "architecture_qualifier": {{ "kind": "implicit" }},
        "native_text": null
      }}],
      "requirements": [],

    "relations": []
    }}
  ]
}}"#
        );
        assert!(StaticIndex::parse(&input).is_err());
    }

    #[test]
    fn static_index_rejects_string_dependency_contract() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
            1,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        ))
        .unwrap();
        let package = value["packages"][0].as_object_mut().unwrap();
        package.remove("provides");
        package.remove("requirements");
        package.insert(
            "dependencies".to_string(),
            serde_json::json!(["libfoo >= 2.0"]),
        );

        let error = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert!(error.to_string().contains("provides"), "{error}");
    }

    #[test]
    fn static_index_rejects_negative_requirement_groups() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
            1,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        ))
        .unwrap();
        value["packages"][0]["requirements"] = serde_json::json!([{
            "kind": "Conflict",
            "behavior": "Hard",
            "expression": {
                "operator": "atom",
                "operands": {
                    "name": "incompatible-widget",
                    "capability_kind": null,
                    "version_constraint": null,
                    "native_text": null
                }
            },
            "alternatives": [{
                "name": "incompatible-widget",
                "capability_kind": null,
                "version_constraint": null,
                "native_text": null
            }],
            "description": null,
            "native_text": "incompatible-widget"
        }]);

        let error = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("unsupported negative relation"),
            "{error}"
        );
    }

    #[test]
    fn static_index_round_trips_typed_relation_authority() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
            1,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        ))
        .unwrap();
        let relation = crate::repository::package_relation::parse_native_relation(
            crate::repository::dependency_model::RepositoryRequirementKind::Conflict,
            crate::repository::versioning::VersionScheme::Conary,
            "incompatible-widget",
        )
        .unwrap();
        value["packages"][0]["relations"] = serde_json::to_value([&relation]).unwrap();

        let index = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap();

        assert_eq!(index.packages[0].relations, vec![relation]);
    }

    #[test]
    fn static_index_rejects_malformed_typed_relation_constraint() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
            1,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        ))
        .unwrap();
        let mut relation = crate::repository::package_relation::parse_native_relation(
            crate::repository::dependency_model::RepositoryRequirementKind::Replace,
            crate::repository::versioning::VersionScheme::Conary,
            "old-widget<2.0.0",
        )
        .unwrap();
        relation.alternatives[0].version_constraint = Some(">=".to_string());
        if let crate::repository::dependency_model::RepositoryRequirementExpression::Atom(clause) =
            &mut relation.expression
        {
            clause.version_constraint = Some(">=".to_string());
        }
        value["packages"][0]["relations"] = serde_json::to_value([relation]).unwrap();

        let error = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap_err();

        assert!(error.to_string().contains("invalid"), "{error}");
    }

    #[test]
    fn package_keys_reject_unknown_schema() {
        let input = valid_package_keys_json(2, &BASE64.encode([0_u8; 32]));
        assert!(PackageKeysFile::parse(&input).is_err());
    }

    #[test]
    fn package_keys_reject_malformed_public_key() {
        let input = valid_package_keys_json(1, "not base64!");
        assert!(PackageKeysFile::parse(&input).is_err());
    }

    #[test]
    fn package_keys_reject_wrong_public_key_length() {
        let input = valid_package_keys_json(1, &BASE64.encode([0_u8; 31]));
        assert!(PackageKeysFile::parse(&input).is_err());
    }

    #[test]
    fn package_keys_reject_invalid_ed25519_public_key_bytes() {
        let input = valid_package_keys_json(
            1,
            &BASE64.encode([
                0x43, 0xc3, 0x14, 0x30, 0xfa, 0xa7, 0x7c, 0xac, 0x28, 0x60, 0x59, 0x5d, 0x4c, 0xf0,
                0x25, 0x69, 0x2d, 0x65, 0x12, 0x36, 0xec, 0xaf, 0xce, 0xf2, 0xcc, 0xe9, 0x1c, 0xd8,
                0x6e, 0xcf, 0x7d, 0xae,
            ]),
        );
        assert!(PackageKeysFile::parse(&input).is_err());
    }

    #[test]
    fn package_keys_reject_empty_keys_for_non_empty_index() {
        let index = StaticIndex::parse(&valid_index_json(
            1,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        ))
        .unwrap();
        let keys = PackageKeysFile::parse(r#"{"schema":1,"keys":[]}"#).unwrap();
        assert!(keys.validate_for_index(&index).is_err());
    }

    fn valid_index_json(schema: u64, path: &str, size: u64) -> String {
        format!(
            r#"{{
  "schema": {schema},
  "name": "acme-tools",
  "index_version": 7,
  "generated": "2026-06-10T18:00:00Z",
  "packages": [
    {{
      "name": "acme-widget",
      "version": "1.4.2",
      "version_scheme": "conary",
      "release": "1",
      "arch": "x86_64",
      "path": "{path}",
      "sha256": "{VALID_SHA256}",
      "size": {size},
      "provides": [{{
        "name": "acme-widget",
        "kind": "PackageName",
        "version": "1.4.2",
        "architecture_qualifier": {{ "kind": "implicit" }},
        "native_text": null
      }}],
      "requirements": [],

    "relations": []
    }}
  ]
}}"#
        )
    }

    fn valid_package_keys_json(schema: u64, public_key: &str) -> String {
        format!(
            r#"{{
  "schema": {schema},
  "keys": [
    {{
      "algorithm": "ed25519",
      "public_key": "{public_key}",
      "key_id": "publish",
      "status": "active"
    }}
  ]
}}"#
        )
    }
}
