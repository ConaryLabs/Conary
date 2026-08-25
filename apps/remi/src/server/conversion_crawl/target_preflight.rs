// apps/remi/src/server/conversion_crawl/target_preflight.rs
//! Per-artifact proof against every supported static target contract.

use super::validate_sha256;
use anyhow::{Result, ensure};
use conary_core::ccs::v3::AuthorityDocumentV3;
use conary_core::ccs::{StaticTargetCompatibilityProofV1, supported_target_contracts};
use serde::{Deserialize, Serialize};

pub const CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CcsTargetCompatibilityProofV1 {
    pub schema_version: u32,
    pub ccs_sha256: String,
    pub compatibility: StaticTargetCompatibilityProofV1,
}

impl CcsTargetCompatibilityProofV1 {
    fn validate(
        &self,
        expected_ccs_sha256: &str,
        contract: &conary_core::ccs::TargetCapabilityContractV1,
    ) -> Result<()> {
        ensure!(
            self.schema_version == CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
            "unsupported CCS target compatibility proof schema {}",
            self.schema_version
        );
        validate_sha256(&self.ccs_sha256, "target-preflight CCS")?;
        ensure!(
            self.ccs_sha256 == expected_ccs_sha256,
            "target-preflight proof names a different CCS artifact"
        );
        self.compatibility
            .validate_for_contract(contract)
            .map_err(anyhow::Error::msg)
    }
}

pub(super) fn preflight_all_targets(
    authority: &AuthorityDocumentV3,
    ccs_sha256: &str,
) -> Result<Vec<CcsTargetCompatibilityProofV1>> {
    validate_sha256(ccs_sha256, "target-preflight CCS")?;
    let proofs = supported_target_contracts()
        .iter()
        .map(|contract| {
            Ok(CcsTargetCompatibilityProofV1 {
                schema_version: CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                ccs_sha256: ccs_sha256.to_string(),
                compatibility: contract.preflight_authority(authority)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    validate_complete_target_proofs(&proofs, ccs_sha256)?;
    Ok(proofs)
}

pub(super) fn validate_complete_target_proofs(
    proofs: &[CcsTargetCompatibilityProofV1],
    ccs_sha256: &str,
) -> Result<()> {
    let contracts = supported_target_contracts();
    ensure!(
        proofs.len() == contracts.len(),
        "CCS target-preflight evidence names {} targets but {} supported contracts are required",
        proofs.len(),
        contracts.len()
    );
    for (proof, contract) in proofs.iter().zip(contracts) {
        proof.validate(ccs_sha256, contract)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proofs(ccs: &str) -> Vec<CcsTargetCompatibilityProofV1> {
        supported_target_contracts()
            .iter()
            .map(|contract| CcsTargetCompatibilityProofV1 {
                schema_version: CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                ccs_sha256: ccs.to_string(),
                compatibility: StaticTargetCompatibilityProofV1 {
                    schema_version: conary_core::ccs::STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                    target_profile: contract.target_profile,
                    target_contract_sha256: contract.sha256().expect("contract digest"),
                    required_capabilities: Vec::new(),
                    required_systemd_operations: Vec::new(),
                    required_linux_process_capabilities: Vec::new(),
                },
            })
            .collect()
    }

    #[test]
    fn preflight_requires_exact_ordered_supported_target_set() {
        let ccs = "a".repeat(64);
        let proofs = proofs(&ccs);
        assert_eq!(
            proofs
                .iter()
                .map(|proof| proof.compatibility.target_profile.as_str())
                .collect::<Vec<_>>(),
            vec!["fedora-44", "ubuntu-26.04", "arch"]
        );
        validate_complete_target_proofs(&proofs, &ccs).expect("complete target proofs");

        let mut missing = proofs.clone();
        missing.pop();
        assert!(validate_complete_target_proofs(&missing, &ccs).is_err());

        let mut reordered = proofs.clone();
        reordered.swap(0, 1);
        assert!(validate_complete_target_proofs(&reordered, &ccs).is_err());

        let mut drifted = proofs;
        drifted[0].ccs_sha256 = "b".repeat(64);
        assert!(validate_complete_target_proofs(&drifted, &ccs).is_err());
    }

    #[test]
    fn target_preflight_proof_is_strict() {
        let mut value =
            serde_json::to_value(&proofs(&"a".repeat(64))[0]).expect("target proof JSON");
        value
            .as_object_mut()
            .expect("target proof object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<CcsTargetCompatibilityProofV1>(value).is_err());
    }
}
