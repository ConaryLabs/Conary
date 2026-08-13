// conary-core/src/ccs/v3/schema.rs

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::capability::CapabilityDeclaration;
use crate::ccs::manifest::FileCapability;
use crate::packages::config_authority::SourceConfigDeclaration;
use crate::payload::{PayloadContentAuthority, PayloadNode};
use crate::repository::dependency_model::{
    CapabilityProvenance, DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryRequirementGroup,
};
use crate::repository::versioning::VersionScheme;

pub const FORMAT_VERSION_V3: u16 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorityDocumentV3 {
    pub format_version: u16,
    pub identity: PackageIdentityV3,
    pub kind: PackageKindV3,
    #[serde(default)]
    #[serde(rename = "capabilities")]
    pub provided_capabilities: Vec<ProvidedCapabilityV3>,
    #[serde(default)]
    pub requirements: Vec<RepositoryRequirementGroup>,
    #[serde(default)]
    pub relations: Vec<RepositoryRequirementGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_capabilities: Option<CapabilityDeclaration>,
    /// Exact Linux file capability declarations bound to signed package files.
    ///
    /// Debug TOML is never install authority; authored declarations are
    /// canonicalized into this signed field and verified before projection
    /// back into the install manifest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_capabilities: Vec<FileCapability>,
    #[serde(default)]
    pub components: BTreeMap<String, ComponentAuthorityV3>,
    #[serde(default)]
    pub lifecycle: LifecycleAuthorityV3,
    #[serde(default)]
    pub provenance: ProvenanceAuthorityV3,
    #[serde(default)]
    pub debug_toml_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageIdentityV3 {
    pub name: String,
    pub version: String,
    pub version_scheme: VersionScheme,
    pub release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debian_multi_arch: Option<DebianMultiArch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    pub kind: PackageKindTagV3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKindTagV3 {
    Package,
    Group,
    Redirect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "type", content = "data")]
pub enum PackageKindV3 {
    Package(PackageDataV3),
    Group(GroupDataV3),
    Redirect(RedirectDataV3),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PackageDataV3 {
    #[serde(default)]
    pub files: Vec<FileAuthorityV3>,
    #[serde(default)]
    pub config: Vec<SourceConfigDeclaration>,
    #[serde(default)]
    pub policy: PackagePolicyV3,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupDataV3 {
    pub members: Vec<GroupMemberV3>,
    #[serde(default)]
    pub provides: Vec<DependencyEntryV3>,
    #[serde(default)]
    pub conflicts: Vec<DependencyEntryV3>,
    #[serde(default)]
    pub policy: PackagePolicyV3,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedirectDataV3 {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMemberV3 {
    pub requirement: DependencyEntryV3,
    pub strength: GroupMemberStrengthV3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GroupMemberStrengthV3 {
    Required,
    Recommended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyEntryV3 {
    pub kind: DependencyKindV3,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

/// Exact capability supplied by this signed package authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvidedCapabilityV3 {
    pub kind: DependencyKindV3,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_relation: Option<ProvideVersionRelation>,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier: ProvideArchitectureQualifier,
    pub provenance: CapabilityProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKindV3 {
    Package,
    Capability,
    File,
    Path,
    Binary,
    Soname,
    PkgConfig,
    PkgConfig32,
    Comar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileAuthorityV3 {
    pub path: String,
    pub node: PayloadNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<PayloadContentAuthority>,
    /// Required signed storage/reconstruction authority for regular files.
    ///
    /// This field intentionally has no serde default: current v3 is hard-cut,
    /// so an older document that omitted layout authority is not current v3.
    pub content_layout: FileContentLayoutV3,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigSemanticsV3>,
    #[serde(default)]
    pub conflict: ConflictPolicyV3,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum FileContentLayoutV3 {
    NoContent,
    WholeObject,
    FastCdcV2020 {
        min_size: u32,
        average_size: u32,
        max_size: u32,
        chunks: Vec<crate::ccs::chunking::ChunkReference>,
    },
}

impl FileContentLayoutV3 {
    pub fn chunks(&self) -> Option<&[crate::ccs::chunking::ChunkReference]> {
        match self {
            Self::FastCdcV2020 { chunks, .. } => Some(chunks),
            Self::NoContent | Self::WholeObject => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentAuthorityV3 {
    pub name: String,
    pub default: bool,
    pub file_count: u32,
    pub total_size: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigSemanticsV3 {
    pub noreplace: bool,
    pub ghost: bool,
    pub remove_on_upgrade: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictPolicyV3 {
    #[default]
    Error,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct LifecycleAuthorityV3 {
    #[serde(default)]
    pub users: Vec<LifecycleUserV3>,
    #[serde(default)]
    pub groups: Vec<LifecycleGroupV3>,
    #[serde(default)]
    pub directories: Vec<LifecycleDirectoryV3>,
    #[serde(default)]
    pub services: Vec<LifecycleServiceV3>,
    #[serde(default)]
    pub systemd: Vec<LifecycleSystemdV3>,
    #[serde(default)]
    pub tmpfiles: Vec<LifecycleTmpfilesV3>,
    #[serde(default)]
    pub sysctl: Vec<LifecycleSysctlV3>,
    #[serde(default)]
    pub alternatives: Vec<LifecycleAlternativeV3>,
    /// Package-scoped capability declarations from `ccs.toml`. Each
    /// executable hook repeats this exact set so its standalone execution
    /// contract remains complete.
    #[serde(default)]
    pub script_capabilities: Vec<LifecycleScriptCapabilityV3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_install: Option<LifecycleScriptV3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_remove: Option<LifecycleScriptV3>,
    /// Exact package-manager-native lifecycle contract carried by converted
    /// packages. This is signed install authority, not debug-TOML evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lifecycle: Option<crate::ccs::native_lifecycle::NativeLifecycleBundle>,
    /// Exact repository declarations and trust projections installed by this
    /// package. These values are signed lifecycle authority and are applied in
    /// the same transaction as the package payload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository_enrollments:
        Vec<crate::repository::enrollment::PackageRepositoryEnrollmentIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleUserV3 {
    pub name: String,
    pub system: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleGroupV3 {
    pub name: String,
    pub system: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleDirectoryV3 {
    pub path: String,
    pub mode: String,
    pub owner: String,
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleServiceV3 {
    pub name: String,
    pub action: LifecycleServiceActionV3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleServiceActionV3 {
    Enable,
    Disable,
    Start,
    Stop,
    Reload,
    Restart,
    TryRestart,
    ReloadOrRestart,
    ReloadOrTryRestart,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleSystemdV3 {
    pub unit: String,
    pub enable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleTmpfilesV3 {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub path: String,
    pub mode: String,
    pub user: String,
    pub group: String,
    pub age: String,
    pub argument: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleSysctlV3 {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleAlternativeV3 {
    pub link: String,
    pub name: String,
    pub path: String,
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// Executable native CCS lifecycle authority.
///
/// `interpreter` and `execution` are explicit even though v3 currently admits
/// only the contract implemented by `HookExecutor`. This prevents an
/// interpreter or execution-root guess from entering install authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleScriptV3 {
    pub interpreter: String,
    pub body: String,
    #[serde(default)]
    pub capabilities: Vec<LifecycleScriptCapabilityV3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
    pub execution: LifecycleScriptExecutionV3,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleScriptCapabilityV3 {
    pub name: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleScriptExecutionV3 {
    SandboxedTargetRoot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProvenanceAuthorityV3 {
    /// Install-time provenance facts. Build attestation envelopes live in
    /// MANIFEST.attestation.json; the build-time classifier version remains
    /// in that attestation envelope, not in v3 package authority.
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
pub struct PackagePolicyV3 {
    /// M4a carries only host-mutation policy. Required-capability,
    /// public-serving, and trust-metadata policy fields are reserved for M4b+.
    #[serde(default)]
    pub allow_host_mutation: bool,
}

impl DependencyEntryV3 {
    pub fn package(name: impl Into<String>) -> Self {
        Self {
            kind: DependencyKindV3::Package,
            name: name.into(),
            version_constraint: None,
            target: None,
            component: None,
        }
    }
}

impl AuthorityDocumentV3 {
    pub fn to_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    /// Decode signed authority under the canonical structural budget.
    ///
    /// There is no unbounded decode entry point: every consumer goes through
    /// `crate::ccs::budget`, so a hostile nesting depth or declared length
    /// fails before allocation.
    pub fn from_cbor(bytes: &[u8]) -> anyhow::Result<Self> {
        crate::ccs::budget::CCS_BUDGET.decode_authority(bytes)
    }

    #[cfg(test)]
    pub(crate) fn package_for_tests(name: &str) -> Self {
        let mut authority = Self::empty_package_for_tests(name);
        authority.components = BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV3 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: 12,
            },
        )]);
        if let PackageKindV3::Package(data) = &mut authority.kind {
            data.files.push(FileAuthorityV3 {
                path: "/usr/bin/hello".to_string(),
                node: PayloadNode::regular(0o755),
                content: Some(PayloadContentAuthority {
                    sha256: crate::hash::sha256(b"hello world\n"),
                    size: 12,
                }),
                content_layout: FileContentLayoutV3::WholeObject,
                component: "main".to_string(),
                config: None,
                conflict: ConflictPolicyV3::Error,
            });
        }
        authority
    }

    #[cfg(test)]
    pub(crate) fn empty_package_for_tests(name: &str) -> Self {
        Self {
            format_version: FORMAT_VERSION_V3,
            identity: PackageIdentityV3 {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                version_scheme: VersionScheme::Conary,
                release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                debian_multi_arch: None,
                platform: Some("linux".to_string()),
                kind: PackageKindTagV3::Package,
            },
            kind: PackageKindV3::Package(PackageDataV3::default()),
            provided_capabilities: Vec::new(),
            requirements: Vec::new(),
            relations: Vec::new(),
            execution_capabilities: None,
            file_capabilities: Vec::new(),
            components: BTreeMap::new(),
            lifecycle: LifecycleAuthorityV3::default(),
            provenance: ProvenanceAuthorityV3 {
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
        let authority = AuthorityDocumentV3::package_for_tests("hello");
        let bytes = authority.to_cbor().unwrap();
        let decoded = AuthorityDocumentV3::from_cbor(&bytes).unwrap();
        assert_eq!(decoded.format_version, FORMAT_VERSION_V3);
        assert!(matches!(decoded.kind, PackageKindV3::Package(_)));
    }

    #[test]
    fn typed_relations_round_trip_in_signed_v3_authority() {
        let mut authority = AuthorityDocumentV3::package_for_tests("replacement");
        authority.identity.version_scheme = VersionScheme::Rpm;
        authority.relations.push(
            crate::repository::package_relation::parse_native_relation(
                crate::repository::dependency_model::RepositoryRequirementKind::Obsolete,
                VersionScheme::Rpm,
                "old-package < 2",
            )
            .unwrap(),
        );

        let bytes = authority.to_cbor().unwrap();
        let decoded = AuthorityDocumentV3::from_cbor(&bytes).unwrap();

        assert_eq!(decoded.relations, authority.relations);
        super::super::validation::validate_authority(&decoded).unwrap();
    }

    #[test]
    fn package_capabilities_round_trip_in_signed_v3_authority() {
        let mut authority = AuthorityDocumentV3::package_for_tests("capable");
        authority.execution_capabilities = Some(crate::capability::CapabilityDeclaration {
            rationale: Some("needs repository access".to_string()),
            network: crate::capability::NetworkCapabilities {
                connect_tcp: vec![443],
                ..Default::default()
            },
            ..Default::default()
        });

        let bytes = authority.to_cbor().unwrap();
        let decoded = AuthorityDocumentV3::from_cbor(&bytes).unwrap();

        assert_eq!(
            decoded.execution_capabilities,
            authority.execution_capabilities
        );
        super::super::validation::validate_authority(&decoded).unwrap();
    }

    #[test]
    fn file_capabilities_round_trip_in_signed_v3_authority() {
        let mut authority = AuthorityDocumentV3::package_for_tests("file-capable");
        authority.file_capabilities = vec![FileCapability {
            path: "/usr/bin/hello".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }];

        let bytes = authority.to_cbor().unwrap();
        let decoded = AuthorityDocumentV3::from_cbor(&bytes).unwrap();

        assert_eq!(decoded.file_capabilities, authority.file_capabilities);
        super::super::validation::validate_authority(&decoded).unwrap();
    }

    #[test]
    fn tmpfiles_authority_round_trips_exact_seven_columns() {
        let tmpfiles = LifecycleTmpfilesV3 {
            entry_type: "x!$".to_string(),
            path: "/var/cache/example/*".to_string(),
            mode: "-".to_string(),
            user: "-".to_string(),
            group: "-".to_string(),
            age: "~am:30d".to_string(),
            argument: "exact argument with spaces".to_string(),
            reversible: Some(true),
        };
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&tmpfiles, &mut encoded).unwrap();
        let decoded: LifecycleTmpfilesV3 = ciborium::from_reader(encoded.as_slice()).unwrap();

        assert_eq!(decoded, tmpfiles);
    }

    #[test]
    fn tmpfiles_authority_rejects_removed_partial_shape() {
        let removed_shape = serde_json::json!({
            "type": "d",
            "path": "/run/example",
            "mode": "0755",
            "owner": "root",
            "group": "root"
        });

        assert!(serde_json::from_value::<LifecycleTmpfilesV3>(removed_shape).is_err());
    }

    #[test]
    fn group_requires_members_and_has_no_payload_fields() {
        let group = PackageKindV3::Group(GroupDataV3 {
            members: vec![GroupMemberV3 {
                requirement: DependencyEntryV3::package("hello"),
                strength: GroupMemberStrengthV3::Required,
            }],
            provides: Vec::new(),
            conflicts: Vec::new(),
            policy: PackagePolicyV3::default(),
        });
        assert!(matches!(group, PackageKindV3::Group(_)));
    }

    #[test]
    fn redirect_has_minimum_authority_fields() {
        let redirect = RedirectDataV3 {
            to: "new-name".to_string(),
            version_constraint: Some(">=1.0.0".to_string()),
            reason: Some("package renamed".to_string()),
        };
        assert_eq!(redirect.to, "new-name");
        assert_eq!(redirect.version_constraint.as_deref(), Some(">=1.0.0"));
    }
}
