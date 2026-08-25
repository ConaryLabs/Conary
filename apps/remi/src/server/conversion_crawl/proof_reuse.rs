// apps/remi/src/server/conversion_crawl/proof_reuse.rs
//! Exact artifact-and-contract identity for reusable conversion proof.

use super::validate_sha256;
use anyhow::{Result, ensure};
use conary_core::ccs::{TargetProfileV1, supported_target_contracts};
use conary_core::repository::catalog::CatalogPackageRecordV1;
use serde::{Deserialize, Serialize};

pub const CONVERSION_PROOF_KEY_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionProofTargetContractV1 {
    pub target_profile: TargetProfileV1,
    pub target_contract_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionProofKeyV1 {
    pub schema_version: u32,
    pub source_profile: String,
    pub package_key_sha256: String,
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
    pub package_architecture: Option<String>,
    pub source_artifact_sha256: String,
    pub converter_schema_version: i32,
    pub converter_version: String,
    pub ccs_format_version: u16,
    pub targets_signer_public_key_sha256: String,
    pub target_contracts: Vec<ConversionProofTargetContractV1>,
}

impl ConversionProofKeyV1 {
    pub(super) fn current(
        package: &CatalogPackageRecordV1,
        source_artifact_sha256: String,
        targets_signer_public_key_sha256: String,
    ) -> Result<Self> {
        let key = Self {
            schema_version: CONVERSION_PROOF_KEY_SCHEMA_V1,
            source_profile: package.source_profile.clone(),
            package_key_sha256: package.package_key_sha256.clone(),
            package_name: package.name.clone(),
            package_version: package.version.clone(),
            package_release: package.package_release.clone(),
            package_architecture: package.architecture.clone(),
            source_artifact_sha256,
            converter_schema_version: conary_core::db::models::CONVERSION_VERSION,
            converter_version: env!("CARGO_PKG_VERSION").to_string(),
            ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
            targets_signer_public_key_sha256,
            target_contracts: supported_target_contracts()
                .iter()
                .map(|contract| {
                    Ok(ConversionProofTargetContractV1 {
                        target_profile: contract.target_profile,
                        target_contract_sha256: contract.sha256().map_err(anyhow::Error::msg)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };
        key.validate_current()?;
        Ok(key)
    }

    pub fn validate_current(&self) -> Result<()> {
        ensure!(
            self.schema_version == CONVERSION_PROOF_KEY_SCHEMA_V1,
            "unsupported conversion proof key schema {}",
            self.schema_version
        );
        ensure!(
            conary_core::repository::supported_profiles::profile_by_public_id(&self.source_profile)
                .is_some(),
            "conversion proof key source profile is not public"
        );
        validate_sha256(&self.package_key_sha256, "conversion proof package key")?;
        validate_sha256(
            &self.source_artifact_sha256,
            "conversion proof source artifact",
        )?;
        validate_sha256(
            &self.targets_signer_public_key_sha256,
            "conversion proof targets signer",
        )?;
        ensure!(
            !self.package_name.is_empty()
                && !self.package_version.is_empty()
                && !self.package_release.is_empty(),
            "conversion proof package identity is incomplete"
        );
        ensure!(
            self.package_architecture
                .as_ref()
                .is_some_and(|architecture| !architecture.is_empty()),
            "conversion proof package architecture is incomplete"
        );
        ensure!(
            self.converter_schema_version == conary_core::db::models::CONVERSION_VERSION,
            "conversion proof converter schema has drifted"
        );
        ensure!(
            self.converter_version == env!("CARGO_PKG_VERSION"),
            "conversion proof converter version has drifted"
        );
        ensure!(
            self.ccs_format_version == conary_core::ccs::v3::FORMAT_VERSION_V3,
            "conversion proof CCS schema has drifted"
        );

        let contracts = supported_target_contracts();
        ensure!(
            self.target_contracts.len() == contracts.len(),
            "conversion proof target contract set is incomplete"
        );
        for (identity, contract) in self.target_contracts.iter().zip(contracts) {
            validate_sha256(
                &identity.target_contract_sha256,
                "conversion proof target contract",
            )?;
            let expected_digest = contract.sha256().map_err(anyhow::Error::msg)?;
            ensure!(
                identity.target_profile == contract.target_profile
                    && identity.target_contract_sha256 == expected_digest,
                "conversion proof target contract order or digest has drifted"
            );
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String> {
        self.validate_current()?;
        let canonical = conary_core::json::canonical_json(self).map_err(anyhow::Error::msg)?;
        Ok(conary_core::hash::sha256(&canonical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::package;

    fn key() -> ConversionProofKeyV1 {
        ConversionProofKeyV1::current(
            &package(
                "fedora-44",
                "demo",
                "1.0",
                "1",
                Some("x86_64"),
                1,
                "proof-key",
            ),
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("current conversion proof key")
    }

    #[test]
    fn proof_key_binds_exact_current_converter_ccs_signer_and_targets() {
        let current_key = key();
        current_key.validate_current().expect("valid proof key");
        assert_eq!(current_key.target_contracts.len(), 3);
        assert_eq!(current_key.sha256().expect("proof key digest").len(), 64);

        let mut drifted = current_key.clone();
        drifted.converter_schema_version += 1;
        assert!(drifted.validate_current().is_err());

        let mut drifted = current_key.clone();
        drifted.target_contracts.swap(0, 1);
        assert!(drifted.validate_current().is_err());

        let mut drifted = current_key;
        drifted.targets_signer_public_key_sha256 = "c".repeat(64);
        assert_ne!(
            drifted
                .sha256()
                .expect("different signer remains a valid key"),
            key().sha256().expect("original key digest")
        );
    }

    #[test]
    fn candidate_profile_cannot_enter_public_proof_reuse() {
        let mut candidate = key();
        candidate.source_profile = "solus".to_string();
        assert!(candidate.validate_current().is_err());
    }

    #[test]
    fn proof_key_json_rejects_unknown_fields() {
        let mut value = serde_json::to_value(key()).expect("proof key JSON");
        value
            .as_object_mut()
            .expect("proof key object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<ConversionProofKeyV1>(value).is_err());
    }
}
