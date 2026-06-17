// conary-core/src/ccs/attestation.rs

//! CCS build attestation envelopes and release-publish evidence.

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const BUILD_ATTESTATION_SCHEMA_V1: u32 = 1;
pub const FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildOutputIdentity {
    pub file_merkle_root: String,
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub origin_class: String,
    pub hardening_level: String,
    pub hermetic_evidence_hash: String,
    pub canonical_content_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildAttestationPayload {
    pub schema_version: u32,
    pub origin_class: String,
    pub hardening_level: String,
    pub build_input: crate::recipe::hermetic::BuildInputIdentity,
    pub dependency_lock: crate::recipe::hermetic::DependencyLock,
    pub hermetic_evidence_hash: String,
    pub output_identity: BuildOutputIdentity,
    pub build_command_risk_report_hash: String,
    pub scriptlet_risk_report_hash: Option<String>,
    pub conversion_boundary_hash: Option<String>,
    pub publish_policy_digest: String,
    pub command_risk_classifier_version: String,
    pub sandbox_profile: String,
    pub seccomp_profile: Option<String>,
    pub builder_identity: String,
    pub conary_version: String,
    pub issued_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildAttestationEnvelope {
    pub schema_version: u32,
    pub payload: BuildAttestationPayload,
    pub signature_algorithm: String,
    pub signature: String,
    pub signer_key_id: String,
    pub signer_public_key: String,
    pub signed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForeignConversionBoundary {
    pub schema_version: u32,
    pub source_format: String,
    pub source_checksum: String,
    pub output_identity: BuildOutputIdentity,
    pub build_risk_report_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_risk_report: Option<crate::security::command_risk::CommandRiskReport>,
    pub scriptlet_risk_report_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scriptlet_risk_report: Option<crate::security::command_risk::CommandRiskReport>,
    pub diagnostics: Vec<String>,
}

impl ForeignConversionBoundary {
    #[cfg(test)]
    pub(crate) fn for_tests(source_format: &str, source_checksum: &str) -> Self {
        Self {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: source_format.to_string(),
            source_checksum: source_checksum.to_string(),
            output_identity: test_support::sample_output_identity_for_tests(),
            build_risk_report_hash: None,
            build_risk_report: None,
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        }
    }
}

pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    crate::json::canonical_json(value).map_err(anyhow::Error::msg)
}

pub fn canonical_json_hash<T: Serialize>(value: &T) -> Result<String> {
    let bytes = canonical_json_bytes(value)?;
    Ok(crate::hash::sha256_prefixed(&bytes))
}

pub fn sign_build_attestation(
    payload: BuildAttestationPayload,
    key: &crate::ccs::signing::SigningKeyPair,
) -> Result<BuildAttestationEnvelope> {
    let canonical = canonical_json_bytes(&payload)?;
    let signature = key.sign(&canonical);
    Ok(BuildAttestationEnvelope {
        schema_version: BUILD_ATTESTATION_SCHEMA_V1,
        payload,
        signature_algorithm: signature.algorithm,
        signature: signature.signature,
        signer_key_id: key
            .key_id()
            .map(str::to_string)
            .unwrap_or_else(|| key.public_key_base64()),
        signer_public_key: signature.public_key,
        signed_at: signature
            .timestamp
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
    })
}

pub fn verify_build_attestation_envelope(
    envelope: &BuildAttestationEnvelope,
    public_key_base64: &str,
) -> Result<()> {
    anyhow::ensure!(
        envelope.schema_version == BUILD_ATTESTATION_SCHEMA_V1,
        "unsupported build attestation schema version {}",
        envelope.schema_version
    );
    anyhow::ensure!(
        envelope.signature_algorithm == "ed25519",
        "unsupported build attestation signature algorithm {}",
        envelope.signature_algorithm
    );
    anyhow::ensure!(
        envelope.signer_public_key == public_key_base64,
        "build attestation signer public key does not match trusted key"
    );

    let signature_bytes = BASE64
        .decode(&envelope.signature)
        .context("decode build attestation signature")?;
    let signature =
        Signature::from_slice(&signature_bytes).context("parse build attestation signature")?;
    let key_bytes = BASE64
        .decode(public_key_base64)
        .context("decode build attestation public key")?;
    let verifying_key = VerifyingKey::from_bytes(
        &key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("build attestation public key must be 32 bytes"))?,
    )
    .context("parse build attestation public key")?;
    let canonical = canonical_json_bytes(&envelope.payload)?;
    verifying_key
        .verify_strict(&canonical, &signature)
        .context("verify build attestation signature")
}

pub fn compute_v2_content_identity(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Result<String> {
    crate::ccs::v2::compute_v2_content_identity(authority)
}

pub fn compute_v2_file_merkle_root(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Result<String> {
    crate::ccs::v2::compute_v2_file_merkle_root(authority)
}

pub fn compute_build_output_identity(
    package: &crate::ccs::package::CcsPackage,
) -> Result<BuildOutputIdentity> {
    if let Some(authority) = package.v2_authority() {
        let provenance = &authority.provenance;
        return Ok(BuildOutputIdentity {
            file_merkle_root: compute_v2_file_merkle_root(authority)?,
            package_name: authority.identity.name.clone(),
            package_version: authority.identity.version.clone(),
            package_release: authority.identity.release.clone(),
            architecture: authority.identity.architecture.clone(),
            origin_class: provenance
                .origin_class
                .clone()
                .context("v2 build output identity requires origin_class")?,
            hardening_level: provenance
                .hardening_level
                .clone()
                .context("v2 build output identity requires hardening_level")?,
            hermetic_evidence_hash: provenance
                .hermetic_evidence_hash
                .clone()
                .context("v2 build output identity requires hermetic_evidence_hash")?,
            canonical_content_identity: compute_v2_content_identity(authority)?,
        });
    }

    let manifest = package.manifest();
    let provenance = manifest
        .provenance
        .as_ref()
        .context("build output identity requires manifest provenance")?;
    let hardening_level = provenance
        .hardening_level
        .clone()
        .context("build output identity requires hardening_level")?;
    let origin_class = provenance
        .origin_class
        .clone()
        .context("build output identity requires origin_class")?;
    let hermetic_evidence_hash = provenance
        .hermetic_evidence
        .as_ref()
        .map(canonical_json_hash)
        .transpose()?
        .context("build output identity requires hermetic evidence")?;
    let file_merkle_root = provenance
        .merkle_root
        .clone()
        .or_else(|| {
            package
                .binary_manifest()
                .map(|manifest| manifest.content_root.value.clone())
        })
        .context("build output identity requires file Merkle root")?;
    let canonical_content_identity = compute_content_identity_excluding_signatures(package)?;

    Ok(BuildOutputIdentity {
        file_merkle_root,
        package_name: manifest.package.name.clone(),
        package_version: manifest.package.version.clone(),
        package_release: "1".to_string(),
        architecture: manifest
            .package
            .platform
            .as_ref()
            .and_then(|platform| platform.arch.clone()),
        origin_class,
        hardening_level,
        hermetic_evidence_hash,
        canonical_content_identity,
    })
}

pub fn compute_content_identity_excluding_signatures(
    package: &crate::ccs::package::CcsPackage,
) -> Result<String> {
    let mut manifest = package.manifest().clone();
    if let Some(provenance) = manifest.provenance.as_mut() {
        provenance.build_attestation = None;
        provenance.foreign_conversion_boundary = None;
        provenance.signatures.clear();
        provenance.dna_hash = None;
    }

    let manifest_bytes = canonical_json_bytes(&manifest)
        .context("serialize content identity manifest projection")?;
    let components_bytes = canonical_json_bytes(package.components())
        .context("serialize content identity components")?;
    let files_bytes = canonical_json_bytes(&package.file_entries())
        .context("serialize content identity files")?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&manifest_bytes);
    bytes.extend_from_slice(&components_bytes);
    bytes.extend_from_slice(&files_bytes);

    Ok(crate::hash::sha256_prefixed(&bytes))
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn sample_output_identity_for_tests() -> BuildOutputIdentity {
        BuildOutputIdentity {
            file_merkle_root: "sha256:files".to_string(),
            package_name: "hello".to_string(),
            package_version: "1.0".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            origin_class: "native-built".to_string(),
            hardening_level: "hermetic".to_string(),
            hermetic_evidence_hash: "sha256:evidence".to_string(),
            canonical_content_identity: "sha256:content".to_string(),
        }
    }

    pub(crate) fn sample_payload_for_tests() -> BuildAttestationPayload {
        BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: "native-built".to_string(),
            hardening_level: "hermetic".to_string(),
            build_input: sample_build_input_for_tests(),
            dependency_lock: crate::recipe::hermetic::DependencyLock::default(),
            hermetic_evidence_hash: "sha256:evidence".to_string(),
            output_identity: sample_output_identity_for_tests(),
            build_command_risk_report_hash: "sha256:build-risk".to_string(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: None,
            publish_policy_digest: "m2-policy-v1".to_string(),
            command_risk_classifier_version: "m2-command-risk-v1".to_string(),
            sandbox_profile: "kitchen-pristine-network-none".to_string(),
            seccomp_profile: Some("scriptlet-v1".to_string()),
            builder_identity: "conary-test-builder".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        }
    }

    fn sample_build_input_for_tests() -> crate::recipe::hermetic::BuildInputIdentity {
        crate::recipe::hermetic::BuildInputIdentity {
            recipe: crate::recipe::hermetic::RecipeIdentity::ExplicitRecipe {
                path: "recipe.toml".to_string(),
                hash: "sha256:recipe".to_string(),
            },
            source: crate::recipe::hermetic::SourceIdentity::Archive {
                url: "https://example.invalid/hello.tar.gz".to_string(),
                checksum: "sha256:source".to_string(),
            },
            additional_sources: Vec::new(),
            patches: Vec::new(),
            local_tree: None,
            ecosystem_dependencies: Vec::new(),
            builder_environment: crate::recipe::hermetic::BuilderEnvironmentIdentity {
                kind: crate::recipe::hermetic::BuilderEnvironmentKind::Pristine,
                sysroot_hash: Some("sha256:sysroot".to_string()),
                toolchain_hash: None,
                diagnostics: Vec::new(),
            },
        }
    }

    pub(crate) fn sample_envelope_for_tests(
        key: &crate::ccs::signing::SigningKeyPair,
    ) -> BuildAttestationEnvelope {
        sign_build_attestation(sample_payload_for_tests(), key).unwrap()
    }

    pub(crate) fn sample_v2_envelope_for_tests(
        authority: &crate::ccs::v2::AuthorityDocumentV2,
        key: &crate::ccs::signing::SigningKeyPair,
        policy_digest: &str,
    ) -> BuildAttestationEnvelope {
        let provenance = &authority.provenance;
        let output_identity = BuildOutputIdentity {
            file_merkle_root: compute_v2_file_merkle_root(authority).unwrap(),
            package_name: authority.identity.name.clone(),
            package_version: authority.identity.version.clone(),
            package_release: authority.identity.release.clone(),
            architecture: authority.identity.architecture.clone(),
            origin_class: provenance.origin_class.clone().unwrap(),
            hardening_level: provenance.hardening_level.clone().unwrap(),
            hermetic_evidence_hash: provenance.hermetic_evidence_hash.clone().unwrap(),
            canonical_content_identity: compute_v2_content_identity(authority).unwrap(),
        };
        let mut payload = sample_payload_for_tests();
        payload.origin_class = output_identity.origin_class.clone();
        payload.hardening_level = output_identity.hardening_level.clone();
        payload.hermetic_evidence_hash = output_identity.hermetic_evidence_hash.clone();
        payload.output_identity = output_identity;
        payload.publish_policy_digest = policy_digest.to_string();
        sign_build_attestation(payload, key).unwrap()
    }

    pub(crate) fn sample_hermetic_evidence_for_tests()
    -> crate::recipe::hermetic::HermeticBuildEvidence {
        crate::recipe::hermetic::HermeticBuildEvidence {
            schema_version: crate::recipe::hermetic::HERMETIC_EVIDENCE_SCHEMA_V1,
            build_input: sample_build_input_for_tests(),
            dependency_lock: crate::recipe::hermetic::DependencyLock::default(),
            ecosystem_policy: crate::recipe::hermetic::EcosystemPolicyReport::clean("test"),
            command_risk: crate::recipe::hermetic::BuildCommandRiskReport::clean(),
            reproducibility: crate::recipe::hermetic::ReproducibilityRecord {
                source_date_epoch: Some(1),
                path_remap_count: 1,
                env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
            },
            divergence: crate::recipe::hermetic::DivergenceReport::default(),
            diagnostics: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::packages::traits::PackageFormat;

    #[test]
    fn canonical_payload_hash_is_stable_across_key_ordering() {
        let payload = sample_payload_for_tests();
        let first = canonical_json_hash(&payload).unwrap();
        let second = canonical_json_hash(
            &serde_json::from_str::<serde_json::Value>(&serde_json::to_string(&payload).unwrap())
                .unwrap(),
        )
        .unwrap();

        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn attestation_signature_round_trips() {
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publisher");
        let envelope = sign_build_attestation(sample_payload_for_tests(), &key).unwrap();

        verify_build_attestation_envelope(&envelope, &key.public_key_base64()).unwrap();
        assert_eq!(envelope.signer_key_id, "publisher");
        assert_eq!(envelope.schema_version, BUILD_ATTESTATION_SCHEMA_V1);
    }

    #[test]
    fn attestation_signature_rejects_tampered_payload() {
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publisher");
        let mut envelope = sign_build_attestation(sample_payload_for_tests(), &key).unwrap();
        envelope.payload.publish_policy_digest = "tampered".to_string();

        assert!(verify_build_attestation_envelope(&envelope, &key.public_key_base64()).is_err());
    }

    #[test]
    fn foreign_boundary_hash_changes_when_source_checksum_changes() {
        let mut boundary = ForeignConversionBoundary::for_tests("rpm", "sha256:one");
        let first = canonical_json_hash(&boundary).unwrap();
        boundary.source_checksum = "sha256:two".to_string();
        let second = canonical_json_hash(&boundary).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn output_identity_does_not_use_dna_hash() {
        let (_temp, package) = package_with_attestation_and_signature_for_tests("sha256:dna-a");
        let identity = compute_build_output_identity(&package).unwrap();

        assert_ne!(identity.canonical_content_identity, "sha256:dna-a");
    }

    #[test]
    fn output_identity_survives_resigning_and_attestation_replacement() {
        let first_key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("first");
        let second_key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("second");
        let (_first_temp, first) = signed_package_for_identity_tests(&first_key, "m2-policy-v1");
        let (_second_temp, second) = signed_package_for_identity_tests(&second_key, "m2-policy-v2");

        let first_identity = compute_build_output_identity(&first).unwrap();
        let second_identity = compute_build_output_identity(&second).unwrap();

        assert_eq!(
            first_identity.canonical_content_identity,
            second_identity.canonical_content_identity
        );
    }

    fn package_with_attestation_and_signature_for_tests(
        dna_hash: &str,
    ) -> (tempfile::TempDir, crate::ccs::package::CcsPackage) {
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("identity-test");
        let (temp, mut package) = signed_package_for_identity_tests(&key, "m2-policy-v1");
        package
            .manifest_mut_for_tests()
            .provenance
            .get_or_insert_with(Default::default)
            .dna_hash = Some(dna_hash.to_string());
        (temp, package)
    }

    fn signed_package_for_identity_tests(
        key: &crate::ccs::signing::SigningKeyPair,
        policy_digest: &str,
    ) -> (tempfile::TempDir, crate::ccs::package::CcsPackage) {
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("identity.ccs");
        let mut result = crate::ccs::builder::test_support::minimal_build_result("identity", "1.0");
        let mut payload = crate::ccs::attestation::test_support::sample_payload_for_tests();
        payload.publish_policy_digest = policy_digest.to_string();
        result
            .manifest
            .provenance
            .get_or_insert_with(Default::default)
            .build_attestation =
            Some(crate::ccs::attestation::sign_build_attestation(payload, key).unwrap());
        crate::ccs::builder::write_signed_ccs_package(&result, &package_path, key).unwrap();
        let package =
            crate::ccs::package::CcsPackage::parse(package_path.to_str().unwrap()).unwrap();
        (temp, package)
    }
}
