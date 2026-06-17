// conary-core/src/ccs/v2/schema.rs

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_VERSION_V2: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorityDocumentV2 {
    pub format_version: u16,
    pub identity: PackageIdentityV2,
    pub kind: PackageKindV2,
    #[serde(default)]
    pub provides: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub requires: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub components: BTreeMap<String, ComponentAuthorityV2>,
    #[serde(default)]
    pub lifecycle: LifecycleAuthorityV2,
    #[serde(default)]
    pub provenance: ProvenanceAuthorityV2,
    #[serde(default)]
    pub debug_toml_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageIdentityV2 {
    pub name: String,
    pub version: String,
    pub release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    pub kind: PackageKindTagV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKindTagV2 {
    Package,
    Group,
    Redirect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "type", content = "data")]
pub enum PackageKindV2 {
    Package(PackageDataV2),
    Group(GroupDataV2),
    Redirect(RedirectDataV2),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PackageDataV2 {
    #[serde(default)]
    pub files: Vec<FileAuthorityV2>,
    #[serde(default)]
    pub config: Vec<ConfigAuthorityV2>,
    #[serde(default)]
    pub policy: PackagePolicyV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupDataV2 {
    pub members: Vec<GroupMemberV2>,
    #[serde(default)]
    pub provides: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub conflicts: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub policy: PackagePolicyV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedirectDataV2 {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMemberV2 {
    pub requirement: DependencyEntryV2,
    pub strength: GroupMemberStrengthV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GroupMemberStrengthV2 {
    Required,
    Recommended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyEntryV2 {
    pub kind: DependencyKindV2,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKindV2 {
    Package,
    Capability,
    File,
    Path,
    Binary,
    Soname,
    PkgConfig,
    Conflict,
    Replace,
    Obsolete,
    Break,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileAuthorityV2 {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub file_type: FileTypeV2,
    pub mode: u32,
    pub owner: String,
    pub group: String,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    #[serde(default)]
    pub config: Option<ConfigPolicyV2>,
    #[serde(default)]
    pub conflict: ConflictPolicyV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FileTypeV2 {
    Regular,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentAuthorityV2 {
    pub name: String,
    pub default: bool,
    pub file_count: u32,
    pub total_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigAuthorityV2 {
    pub path: String,
    pub policy: ConfigPolicyV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigPolicyV2 {
    Replace,
    NoReplace,
    Merge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictPolicyV2 {
    #[default]
    Error,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LifecycleAuthorityV2 {
    /// M4a carries opaque install-time authority references. Structured
    /// lifecycle authoring and target-specific validation are deferred to M4b
    /// and M4d; these strings must still be signed v2 authority.
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub tmpfiles: Vec<String>,
    #[serde(default)]
    pub sysctl: Vec<String>,
    #[serde(default)]
    pub alternatives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProvenanceAuthorityV2 {
    /// Install-time provenance facts. Build attestation envelopes live in
    /// MANIFEST.attestation.json; the build-time classifier version remains
    /// in that attestation envelope, not in v2 package authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardening_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_input_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermetic_evidence_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_conversion_boundary_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PackagePolicyV2 {
    /// M4a carries only host-mutation policy. Required-capability,
    /// public-serving, and trust-metadata policy fields are reserved for M4b+.
    #[serde(default)]
    pub allow_host_mutation: bool,
}

impl DependencyEntryV2 {
    pub fn package(name: impl Into<String>) -> Self {
        Self {
            kind: DependencyKindV2::Package,
            name: name.into(),
            version_constraint: None,
            target: None,
            component: None,
        }
    }
}

impl AuthorityDocumentV2 {
    pub fn to_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self, ciborium::de::Error<std::io::Error>> {
        ciborium::from_reader(bytes)
    }

    #[cfg(test)]
    pub(crate) fn package_for_tests(name: &str) -> Self {
        let mut authority = Self::empty_package_for_tests(name);
        authority.components = BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV2 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: 12,
            },
        )]);
        if let PackageKindV2::Package(data) = &mut authority.kind {
            data.files.push(FileAuthorityV2 {
                path: "/usr/bin/hello".to_string(),
                sha256: crate::hash::sha256(b"hello world\n"),
                size: 12,
                file_type: FileTypeV2::Regular,
                mode: 0o755,
                owner: "root".to_string(),
                group: "root".to_string(),
                component: "main".to_string(),
                symlink_target: None,
                config: None,
                conflict: ConflictPolicyV2::Error,
            });
        }
        authority
    }

    #[cfg(test)]
    pub(crate) fn empty_package_for_tests(name: &str) -> Self {
        Self {
            format_version: FORMAT_VERSION_V2,
            identity: PackageIdentityV2 {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                platform: Some("linux".to_string()),
                kind: PackageKindTagV2::Package,
            },
            kind: PackageKindV2::Package(PackageDataV2::default()),
            provides: Vec::new(),
            requires: Vec::new(),
            components: BTreeMap::new(),
            lifecycle: LifecycleAuthorityV2::default(),
            provenance: ProvenanceAuthorityV2 {
                origin_class: Some("native-built".to_string()),
                hardening_level: Some("hermetic".to_string()),
                build_input_identity: Some("sha256:build-input".to_string()),
                hermetic_evidence_hash: Some("sha256:evidence".to_string()),
                foreign_conversion_boundary_hash: None,
            },
            debug_toml_sha256: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_kind_serializes_as_tagged_enum() {
        let authority = AuthorityDocumentV2::package_for_tests("hello");
        let bytes = authority.to_cbor().unwrap();
        let decoded = AuthorityDocumentV2::from_cbor(&bytes).unwrap();
        assert_eq!(decoded.format_version, FORMAT_VERSION_V2);
        assert!(matches!(decoded.kind, PackageKindV2::Package(_)));
    }

    #[test]
    fn group_requires_members_and_has_no_payload_fields() {
        let group = PackageKindV2::Group(GroupDataV2 {
            members: vec![GroupMemberV2 {
                requirement: DependencyEntryV2::package("hello"),
                strength: GroupMemberStrengthV2::Required,
            }],
            provides: Vec::new(),
            conflicts: Vec::new(),
            policy: PackagePolicyV2::default(),
        });
        assert!(matches!(group, PackageKindV2::Group(_)));
    }

    #[test]
    fn redirect_has_minimum_authority_fields() {
        let redirect = RedirectDataV2 {
            to: "new-name".to_string(),
            version_constraint: Some(">=1.0".to_string()),
            reason: Some("package renamed".to_string()),
        };
        assert_eq!(redirect.to, "new-name");
        assert_eq!(redirect.version_constraint.as_deref(), Some(">=1.0"));
    }
}
