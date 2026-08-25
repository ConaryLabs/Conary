// conary-core/src/ccs/target_contract.rs
//! Static CCS compatibility contracts for supported installation targets.
//!
//! These declarations describe product support and converter/runtime schema
//! capability. They deliberately do not impersonate [`super::HostCapabilityInventory`],
//! which remains live-host authority for executable identity and handshakes.

use crate::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleEntryKind,
    SourceFormat,
};
use crate::ccs::v3::schema::LifecycleAuthorityV3;
use crate::ccs::v3::{AuthorityDocumentV3, FORMAT_VERSION_V3};
use crate::repository::selector::PackageSelector;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::LazyLock;

mod linux_capability;
pub use linux_capability::LinuxProcessCapabilityV1;

pub const TARGET_CAPABILITY_CONTRACT_SCHEMA_V1: u32 = 1;
pub const STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1: u32 = 1;

/// Exact supported target identities. Identity is attribution; the contract's
/// typed fields, rather than this enum, select compatibility behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TargetProfileV1 {
    #[serde(rename = "fedora-44")]
    Fedora44,
    #[serde(rename = "ubuntu-26.04")]
    Ubuntu2604,
    #[serde(rename = "arch")]
    Arch,
}

impl TargetProfileV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fedora44 => "fedora-44",
            Self::Ubuntu2604 => "ubuntu-26.04",
            Self::Arch => "arch",
        }
    }
}

/// Source-independent runtime interfaces that an artifact may require from a
/// supported target contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StaticTargetCapabilityV1 {
    SelectedRootLifecycle,
    SystemdServiceManager,
    Sysusers,
    Tmpfiles,
    Sysctl,
    Ldconfig,
    SandboxedLifecycle,
    LinuxFileCapabilities,
    RepositoryEnrollment,
}

/// One exact native-lifecycle schema and source-engine contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLifecycleTargetContractV1 {
    pub source_format: SourceFormat,
    pub schema: String,
    pub schema_revision: u16,
}

/// Canonical static product contract for one supported target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetCapabilityContractV1 {
    pub schema_version: u32,
    pub target_profile: TargetProfileV1,
    pub machine_architecture: String,
    pub ccs_format_version: u16,
    pub capability_declaration_schema_version: u32,
    pub native_lifecycle_contracts: Vec<NativeLifecycleTargetContractV1>,
    pub capabilities: Vec<StaticTargetCapabilityV1>,
    pub linux_process_capabilities: Vec<LinuxProcessCapabilityV1>,
}

/// Successful static compatibility evidence for one exact target contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticTargetCompatibilityProofV1 {
    pub schema_version: u32,
    pub target_profile: TargetProfileV1,
    pub target_contract_sha256: String,
    pub required_capabilities: Vec<StaticTargetCapabilityV1>,
    pub required_linux_process_capabilities: Vec<LinuxProcessCapabilityV1>,
}

impl StaticTargetCompatibilityProofV1 {
    pub fn validate_for_contract(
        &self,
        contract: &TargetCapabilityContractV1,
    ) -> Result<(), String> {
        if self.schema_version != STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1 {
            return Err(format!(
                "unsupported static target compatibility proof schema {}",
                self.schema_version
            ));
        }
        if self.target_profile != contract.target_profile {
            return Err("static target proof names a different target contract".to_string());
        }
        let expected_digest = contract.sha256()?;
        if self.target_contract_sha256 != expected_digest {
            return Err("static target proof contract digest has drifted".to_string());
        }
        validate_canonical_capabilities(&self.required_capabilities, "static target proof")?;
        if self
            .required_capabilities
            .iter()
            .any(|required| !contract.capabilities.contains(required))
        {
            return Err("static target proof requires an undeclared target capability".to_string());
        }
        linux_capability::validate_canonical(
            &self.required_linux_process_capabilities,
            "static target proof",
        )?;
        if self
            .required_linux_process_capabilities
            .iter()
            .any(|required| !contract.linux_process_capabilities.contains(required))
        {
            return Err(
                "static target proof requires an undeclared Linux process capability".to_string(),
            );
        }
        Ok(())
    }
}

impl TargetCapabilityContractV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != TARGET_CAPABILITY_CONTRACT_SCHEMA_V1 {
            return Err(format!(
                "unsupported target capability contract schema {}",
                self.schema_version
            ));
        }
        if self.machine_architecture != "x86_64" {
            return Err(format!(
                "unsupported target machine architecture '{}'",
                self.machine_architecture
            ));
        }
        if self.ccs_format_version != FORMAT_VERSION_V3 {
            return Err(format!(
                "unsupported target CCS format version {}",
                self.ccs_format_version
            ));
        }
        if self.capability_declaration_schema_version
            != crate::capability::CAPABILITY_SCHEMA_VERSION
        {
            return Err(format!(
                "unsupported target capability declaration schema {}",
                self.capability_declaration_schema_version
            ));
        }
        let expected_formats = [
            SourceFormat::Rpm,
            SourceFormat::Deb,
            SourceFormat::Arch,
            SourceFormat::Eopkg,
        ];
        if self.native_lifecycle_contracts.len() != expected_formats.len() {
            return Err(
                "target contract does not declare every native lifecycle engine".to_string(),
            );
        }
        for (native, expected_format) in
            self.native_lifecycle_contracts.iter().zip(expected_formats)
        {
            if native.source_format != expected_format
                || native.schema != NATIVE_LIFECYCLE_SCHEMA_V1
                || native.schema_revision != NATIVE_LIFECYCLE_SCHEMA_REVISION
            {
                return Err(
                    "target native lifecycle contracts are incomplete or drifted".to_string(),
                );
            }
        }
        validate_canonical_capabilities(&self.capabilities, "target contract")?;
        linux_capability::validate_canonical(&self.linux_process_capabilities, "target contract")?;
        Ok(())
    }

    pub fn sha256(&self) -> Result<String, String> {
        self.validate()?;
        crate::json::canonical_json(self).map(|bytes| crate::hash::sha256(&bytes))
    }

    pub fn preflight_authority(
        &self,
        authority: &AuthorityDocumentV3,
    ) -> Result<StaticTargetCompatibilityProofV1, StaticTargetCompatibilityError> {
        self.validate()
            .map_err(StaticTargetCompatibilityError::InvalidContract)?;
        if authority.format_version != self.ccs_format_version {
            return Err(StaticTargetCompatibilityError::UnsupportedCcsFormat {
                got: authority.format_version,
                supported: self.ccs_format_version,
            });
        }
        if !PackageSelector::is_architecture_compatible(
            authority.identity.version_scheme,
            authority.identity.architecture.as_deref(),
            &self.machine_architecture,
        ) {
            return Err(StaticTargetCompatibilityError::Architecture {
                package: authority.identity.architecture.clone(),
                target: self.machine_architecture.clone(),
            });
        }
        if let Some(declaration) = authority.execution_capabilities.as_ref() {
            if declaration.version != self.capability_declaration_schema_version {
                return Err(
                    StaticTargetCompatibilityError::UnsupportedCapabilityDeclarationSchema {
                        got: declaration.version,
                        supported: self.capability_declaration_schema_version,
                    },
                );
            }
            declaration
                .validate_for_machine_architecture(&self.machine_architecture)
                .map_err(
                    |error| StaticTargetCompatibilityError::CapabilityDeclaration {
                        detail: error.to_string(),
                    },
                )?;
        }

        let required = required_capabilities(authority, self)?;
        for capability in &required {
            if !self.capabilities.contains(capability) {
                return Err(StaticTargetCompatibilityError::MissingCapability {
                    capability: *capability,
                });
            }
        }
        let required_linux_process_capabilities = authority
            .execution_capabilities
            .as_ref()
            .map(|declaration| {
                declaration
                    .linux
                    .required
                    .iter()
                    .map(|capability| {
                        LinuxProcessCapabilityV1::from_name(capability).ok_or_else(|| {
                            StaticTargetCompatibilityError::LinuxProcessCapability {
                                capability: capability.clone(),
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        for capability in &required_linux_process_capabilities {
            if !self.linux_process_capabilities.contains(capability) {
                return Err(
                    StaticTargetCompatibilityError::MissingLinuxProcessCapability {
                        capability: *capability,
                    },
                );
            }
        }
        let proof = StaticTargetCompatibilityProofV1 {
            schema_version: STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
            target_profile: self.target_profile,
            target_contract_sha256: self
                .sha256()
                .map_err(StaticTargetCompatibilityError::InvalidContract)?,
            required_capabilities: required,
            required_linux_process_capabilities,
        };
        proof
            .validate_for_contract(self)
            .map_err(StaticTargetCompatibilityError::InvalidProof)?;
        Ok(proof)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StaticTargetCompatibilityError {
    #[error("invalid target capability contract: {0}")]
    InvalidContract(String),
    #[error("invalid static target compatibility proof: {0}")]
    InvalidProof(String),
    #[error("CCS format {got} is incompatible with target format {supported}")]
    UnsupportedCcsFormat { got: u16, supported: u16 },
    #[error("package architecture {package:?} is incompatible with target architecture {target}")]
    Architecture {
        package: Option<String>,
        target: String,
    },
    #[error("capability declaration schema {got} is incompatible with target schema {supported}")]
    UnsupportedCapabilityDeclarationSchema { got: u32, supported: u32 },
    #[error("capability declaration is incompatible with the target syscall ABI: {detail}")]
    CapabilityDeclaration { detail: String },
    #[error("native lifecycle authority is incompatible with the target contract: {detail}")]
    NativeLifecycle { detail: String },
    #[error("target contract does not provide required capability {capability:?}")]
    MissingCapability {
        capability: StaticTargetCapabilityV1,
    },
    #[error(
        "Linux process capability declaration is outside the typed target contract: {capability}"
    )]
    LinuxProcessCapability { capability: String },
    #[error("target contract does not provide required Linux process capability {capability:?}")]
    MissingLinuxProcessCapability {
        capability: LinuxProcessCapabilityV1,
    },
}

static SUPPORTED_TARGET_CONTRACTS: LazyLock<Vec<TargetCapabilityContractV1>> =
    LazyLock::new(|| {
        [
            TargetProfileV1::Fedora44,
            TargetProfileV1::Ubuntu2604,
            TargetProfileV1::Arch,
        ]
        .into_iter()
        .map(canonical_contract)
        .collect()
    });

/// Return the exact ordered public target capability contract set.
#[must_use]
pub fn supported_target_contracts() -> &'static [TargetCapabilityContractV1] {
    SUPPORTED_TARGET_CONTRACTS.as_slice()
}

fn canonical_contract(target_profile: TargetProfileV1) -> TargetCapabilityContractV1 {
    TargetCapabilityContractV1 {
        schema_version: TARGET_CAPABILITY_CONTRACT_SCHEMA_V1,
        target_profile,
        machine_architecture: "x86_64".to_string(),
        ccs_format_version: FORMAT_VERSION_V3,
        capability_declaration_schema_version: crate::capability::CAPABILITY_SCHEMA_VERSION,
        native_lifecycle_contracts: [
            SourceFormat::Rpm,
            SourceFormat::Deb,
            SourceFormat::Arch,
            SourceFormat::Eopkg,
        ]
        .into_iter()
        .map(|source_format| NativeLifecycleTargetContractV1 {
            source_format,
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        })
        .collect(),
        capabilities: all_static_capabilities(),
        linux_process_capabilities: LinuxProcessCapabilityV1::all().to_vec(),
    }
}

fn all_static_capabilities() -> Vec<StaticTargetCapabilityV1> {
    vec![
        StaticTargetCapabilityV1::SelectedRootLifecycle,
        StaticTargetCapabilityV1::SystemdServiceManager,
        StaticTargetCapabilityV1::Sysusers,
        StaticTargetCapabilityV1::Tmpfiles,
        StaticTargetCapabilityV1::Sysctl,
        StaticTargetCapabilityV1::Ldconfig,
        StaticTargetCapabilityV1::SandboxedLifecycle,
        StaticTargetCapabilityV1::LinuxFileCapabilities,
        StaticTargetCapabilityV1::RepositoryEnrollment,
    ]
}

fn required_capabilities(
    authority: &AuthorityDocumentV3,
    contract: &TargetCapabilityContractV1,
) -> Result<Vec<StaticTargetCapabilityV1>, StaticTargetCompatibilityError> {
    let lifecycle = &authority.lifecycle;
    let mut required = BTreeSet::new();
    if lifecycle != &LifecycleAuthorityV3::default() {
        required.insert(StaticTargetCapabilityV1::SelectedRootLifecycle);
    }
    if !lifecycle.services.is_empty() || !lifecycle.systemd.is_empty() {
        required.insert(StaticTargetCapabilityV1::SystemdServiceManager);
    }
    if !lifecycle.tmpfiles.is_empty() {
        required.insert(StaticTargetCapabilityV1::Tmpfiles);
    }
    if !lifecycle.sysctl.is_empty() {
        required.insert(StaticTargetCapabilityV1::Sysctl);
    }
    if lifecycle.post_install.is_some() || lifecycle.pre_remove.is_some() {
        required.insert(StaticTargetCapabilityV1::SandboxedLifecycle);
    }
    if !authority.file_capabilities.is_empty() {
        required.insert(StaticTargetCapabilityV1::LinuxFileCapabilities);
    }
    if !lifecycle.repository_enrollments.is_empty() {
        required.insert(StaticTargetCapabilityV1::RepositoryEnrollment);
    }
    if let Some(native) = lifecycle.native_lifecycle.as_ref() {
        native
            .validate()
            .map_err(|error| StaticTargetCompatibilityError::NativeLifecycle {
                detail: error.to_string(),
            })?;
        let supported = contract.native_lifecycle_contracts.iter().any(|candidate| {
            candidate.source_format == native.source_format
                && candidate.schema == native.schema
                && candidate.schema_revision == native.schema_revision
        });
        if !supported {
            return Err(StaticTargetCompatibilityError::NativeLifecycle {
                detail: format!(
                    "unsupported {} schema {} revision {}",
                    native.source_format.as_str(),
                    native.schema,
                    native.schema_revision
                ),
            });
        }
        if native
            .entries
            .iter()
            .any(|entry| entry.kind == NativeLifecycleEntryKind::Executable)
        {
            required.insert(StaticTargetCapabilityV1::SandboxedLifecycle);
        }
        if native
            .entries
            .iter()
            .any(|entry| entry.rpm_sysusers.is_some())
        {
            required.insert(StaticTargetCapabilityV1::Sysusers);
        }
        if native.source_format == SourceFormat::Arch {
            required.insert(StaticTargetCapabilityV1::Ldconfig);
        }
    }
    Ok(required.into_iter().collect())
}

fn validate_canonical_capabilities(
    capabilities: &[StaticTargetCapabilityV1],
    owner: &str,
) -> Result<(), String> {
    if capabilities.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!(
            "{owner} capabilities are repeated or not canonically ordered"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityDeclaration, LinuxCapabilities, SyscallCapabilities};
    use crate::ccs::v3::AuthorityDocumentV3;
    use crate::ccs::v3::schema::{LifecycleSystemdV3, LifecycleTmpfilesV3};
    use crate::repository::versioning::VersionScheme;

    #[test]
    fn supported_target_contracts_are_exact_ordered_and_digest_bound() {
        let contracts = supported_target_contracts();
        assert_eq!(
            contracts
                .iter()
                .map(|contract| contract.target_profile.as_str())
                .collect::<Vec<_>>(),
            vec!["fedora-44", "ubuntu-26.04", "arch"]
        );
        for contract in contracts {
            contract.validate().expect("valid target contract");
            let digest = contract.sha256().expect("target contract digest");
            assert_eq!(digest.len(), 64);
            assert_eq!(
                serde_json::from_value::<TargetCapabilityContractV1>(
                    serde_json::to_value(contract).expect("serialize target contract")
                )
                .expect("strict target contract"),
                *contract
            );
            assert_eq!(
                serde_json::to_string(&contract.target_profile).expect("serialize target id"),
                format!("\"{}\"", contract.target_profile.as_str())
            );
        }
    }

    #[test]
    fn preflight_proves_every_supported_target_without_source_distro_routing() {
        let mut authority = AuthorityDocumentV3::empty_package_for_tests("demo");
        authority.identity.version_scheme = VersionScheme::Rpm;
        authority.identity.architecture = Some("noarch".to_string());
        authority.execution_capabilities = Some(CapabilityDeclaration {
            syscalls: SyscallCapabilities {
                allow: vec!["read".to_string()],
                deny: Vec::new(),
            },
            linux: LinuxCapabilities {
                required: vec!["cap-bpf".to_string()],
            },
            ..CapabilityDeclaration::default()
        });
        authority.lifecycle.systemd.push(LifecycleSystemdV3 {
            unit: "demo.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
        authority.lifecycle.tmpfiles.push(LifecycleTmpfilesV3 {
            entry_type: "d".to_string(),
            path: "/run/demo".to_string(),
            mode: "0755".to_string(),
            user: "root".to_string(),
            group: "root".to_string(),
            age: "-".to_string(),
            argument: "-".to_string(),
            reversible: Some(true),
        });

        for contract in supported_target_contracts() {
            let proof = contract
                .preflight_authority(&authority)
                .expect("static target compatibility");
            proof
                .validate_for_contract(contract)
                .expect("proof revalidates");
            assert_eq!(proof.target_profile, contract.target_profile);
            assert_eq!(
                proof.required_capabilities,
                vec![
                    StaticTargetCapabilityV1::SelectedRootLifecycle,
                    StaticTargetCapabilityV1::SystemdServiceManager,
                    StaticTargetCapabilityV1::Tmpfiles,
                ]
            );
            assert_eq!(
                proof.required_linux_process_capabilities,
                vec![LinuxProcessCapabilityV1::Bpf]
            );
        }
    }

    #[test]
    fn preflight_rejects_architecture_syscall_schema_and_interface_drift() {
        let contract = &supported_target_contracts()[0];
        let mut architecture = AuthorityDocumentV3::empty_package_for_tests("demo");
        architecture.identity.version_scheme = VersionScheme::Rpm;
        architecture.identity.architecture = Some("aarch64".to_string());
        assert!(matches!(
            contract.preflight_authority(&architecture),
            Err(StaticTargetCompatibilityError::Architecture { .. })
        ));

        let mut syscall = AuthorityDocumentV3::empty_package_for_tests("demo");
        syscall.execution_capabilities = Some(CapabilityDeclaration {
            syscalls: SyscallCapabilities {
                allow: vec!["definitely_not_a_syscall".to_string()],
                deny: Vec::new(),
            },
            ..CapabilityDeclaration::default()
        });
        assert!(matches!(
            contract.preflight_authority(&syscall),
            Err(StaticTargetCompatibilityError::CapabilityDeclaration { .. })
        ));

        let mut drifted = contract.clone();
        drifted.capabilities.remove(1);
        let mut requires_systemd = AuthorityDocumentV3::empty_package_for_tests("demo");
        requires_systemd.lifecycle.systemd.push(LifecycleSystemdV3 {
            unit: "demo.service".to_string(),
            enable: true,
            reversible: None,
        });
        drifted
            .validate()
            .expect("structurally valid limited contract");
        assert!(matches!(
            drifted.preflight_authority(&requires_systemd),
            Err(StaticTargetCompatibilityError::MissingCapability {
                capability: StaticTargetCapabilityV1::SystemdServiceManager
            })
        ));

        let mut no_bpf = contract.clone();
        no_bpf
            .linux_process_capabilities
            .retain(|capability| *capability != LinuxProcessCapabilityV1::Bpf);
        let mut requires_bpf = AuthorityDocumentV3::empty_package_for_tests("demo");
        requires_bpf.execution_capabilities = Some(CapabilityDeclaration {
            linux: LinuxCapabilities {
                required: vec!["cap-bpf".to_string()],
            },
            ..CapabilityDeclaration::default()
        });
        assert!(matches!(
            no_bpf.preflight_authority(&requires_bpf),
            Err(
                StaticTargetCompatibilityError::MissingLinuxProcessCapability {
                    capability: LinuxProcessCapabilityV1::Bpf
                }
            )
        ));
    }

    #[test]
    fn strict_contract_and_proof_reject_unknown_and_drifted_input() {
        let contract = &supported_target_contracts()[0];
        let mut value = serde_json::to_value(contract).expect("target contract JSON");
        value
            .as_object_mut()
            .expect("target contract object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<TargetCapabilityContractV1>(value).is_err());

        let mut proof = contract
            .preflight_authority(&AuthorityDocumentV3::empty_package_for_tests("demo"))
            .expect("target proof");
        proof.target_contract_sha256 = "0".repeat(64);
        assert!(proof.validate_for_contract(contract).is_err());
    }
}
