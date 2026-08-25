// apps/remi/src/server/conversion_crawl/ccs_reopen.rs
//! Independent post-persistence CCS artifact reopen authority.

use super::{
    CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CcsArtifactReopenProofV1, ReopenedCcsArtifactEvidence,
    exact_prefixed_sha256, target_preflight,
};
use crate::server::conversion::ServerConversionResult;
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, ensure};
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use conary_core::repository::catalog::CatalogPackageRecordV1;
use std::fs::File;
use std::path::Path;

#[derive(Clone)]
pub(super) struct CcsArtifactReopener {
    policy: TrustPolicy,
    signer_public_key_sha256: String,
}

impl CcsArtifactReopener {
    pub(super) fn for_profile(keys_root: &Path, profile: &str) -> Result<Self> {
        let signing_key = load_role_key(keys_root, profile, RepositorySigningRole::Targets)?;
        let signer_public_key_sha256 =
            conary_core::hash::sha256(signing_key.verifying_key().as_bytes());
        Ok(Self {
            policy: TrustPolicy::strict(vec![signing_key.public_key_base64()]),
            signer_public_key_sha256,
        })
    }

    pub(super) fn reopen(
        &self,
        package: &CatalogPackageRecordV1,
        result: &ServerConversionResult,
    ) -> Result<ReopenedCcsArtifactEvidence> {
        ensure!(
            result.name == package.name
                && result.version == package.version
                && result.source_profile.as_deref() == Some(package.source_profile.as_str()),
            "conversion result identity differs from the exact catalog package"
        );

        let mut persisted = File::open(&result.ccs_path).with_context(|| {
            format!(
                "independently reopen persisted CCS artifact {}",
                result.ccs_path.display()
            )
        })?;
        let actual_ccs = conary_core::hash::hash_reader(
            conary_core::hash::HashAlgorithm::Sha256,
            &mut persisted,
        )?;
        let expected_ccs = exact_prefixed_sha256(&result.content_hash, "converted CCS")?;
        ensure!(
            actual_ccs.as_str() == expected_ccs,
            "independently reopened CCS bytes differ from the persisted conversion digest"
        );

        let verified = verify_package(&result.ccs_path, &self.policy)
            .context("independently verify persisted CCS artifact")?;
        let authority = verified.authority();
        ensure!(
            authority.identity.name == package.name
                && authority.identity.version == package.version
                && authority.identity.release == package.package_release
                && authority.identity.architecture == package.architecture,
            "independently reopened CCS authority identity differs from the exact catalog package"
        );
        ensure!(
            verified.signature().public_key
                == self
                    .policy
                    .trusted_keys()
                    .first()
                    .context("missing trusted CCS key")?
                    .as_str(),
            "independently reopened CCS signer differs from the exact profile targets authority"
        );

        let reopened_transport =
            conary_core::ccs::CcsTransportEnvelopeV1::from_verified_archive(&verified)
                .context("project independently reopened CCS transport")?;
        ensure!(
            reopened_transport == result.transport,
            "independently reopened CCS transport differs from the persisted conversion result"
        );
        let boundary_json = reopened_transport
            .foreign_conversion_boundary_json
            .as_deref()
            .context("independently reopened CCS has no foreign conversion boundary")?;
        let boundary: conary_core::ccs::attestation::ForeignConversionBoundary =
            serde_json::from_str(boundary_json)
                .context("parse independently reopened CCS foreign conversion boundary")?;
        ensure!(
            boundary.output_identity.package_name == package.name
                && boundary.output_identity.package_version == package.version
                && boundary.output_identity.package_release == package.package_release
                && boundary.output_identity.architecture == package.architecture,
            "independently reopened CCS boundary identity differs from the exact catalog package"
        );
        let source = exact_prefixed_sha256(&boundary.source_checksum, "source artifact")?;
        let transport_bytes = conary_core::json::canonical_json(&reopened_transport)
            .map_err(anyhow::Error::msg)
            .context("canonicalize independently reopened CCS transport")?;
        let proof = CcsArtifactReopenProofV1 {
            schema_version: CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
            ccs_format_version: authority.format_version,
            foreign_conversion_boundary_schema_version: boundary.schema_version,
            signer_public_key_sha256: self.signer_public_key_sha256.clone(),
            transport_sha256: conary_core::hash::sha256(&transport_bytes),
            verified_files: u64::try_from(verified.files_checked())
                .context("reopened CCS file count exceeds u64")?,
            verified_objects: u64::try_from(reopened_transport.objects.len())
                .context("reopened CCS object count exceeds u64")?,
        };
        proof.validate()?;
        let target_compatibility_proofs =
            target_preflight::preflight_all_targets(authority, actual_ccs.as_str())?;
        Ok(ReopenedCcsArtifactEvidence {
            source_artifact_sha256: source.to_string(),
            ccs_sha256: actual_ccs.as_str().to_string(),
            reopen_proof: proof,
            target_compatibility_proofs,
        })
    }

    pub(super) fn signer_public_key_sha256(&self) -> &str {
        &self.signer_public_key_sha256
    }

    #[cfg(test)]
    fn for_public_key(public_key: String) -> Self {
        let decoded =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &public_key)
                .expect("test public key is base64");
        Self {
            policy: TrustPolicy::strict(vec![public_key]),
            signer_public_key_sha256: conary_core::hash::sha256(&decoded),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::package;
    use crate::server::conversion::ScriptletPackageMetadata;
    use conary_core::ccs::convert::{
        ConversionOptions, ForeignConversionInput, NativePackageConverter,
    };
    use conary_core::packages::source_authority::{CcsPackageAuthority, SourcePackageAuthority};
    use conary_core::repository::versioning::VersionScheme;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct Fixture {
        _directory: tempfile::TempDir,
        package: CatalogPackageRecordV1,
        result: ServerConversionResult,
        reopener: CcsArtifactReopener,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().expect("CCS reopen fixture directory");
        let signing_key =
            conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("fedora-44-targets");
        let public_key = signing_key.public_key_base64();
        let mut metadata = ForeignConversionInput::new(
            PathBuf::from("demo-1.0-1.x86_64.rpm"),
            "demo".to_string(),
            "1.0".to_string(),
            VersionScheme::Rpm,
        );
        metadata.source_authority = SourcePackageAuthority::Ccs(CcsPackageAuthority {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            version_scheme: VersionScheme::Rpm,
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            capabilities: Vec::new(),
            config: Vec::new(),
        });
        let source_sha256 = "d".repeat(64);
        let converted = NativePackageConverter::new(ConversionOptions {
            output_dir: directory.path().to_path_buf(),
        })
        .with_source_profile("fedora-44")
        .with_source_release("1")
        .with_conversion_tool("remi-test")
        .with_signing_key(Arc::new(signing_key))
        .convert_payload(
            &metadata,
            &[],
            "rpm",
            &conary_core::hash::Hash::new(conary_core::hash::HashAlgorithm::Sha256, source_sha256)
                .expect("source SHA-256"),
        )
        .expect("convert CCS reopen fixture");
        let ccs_path = converted.package_path.expect("converted CCS path");
        let policy = TrustPolicy::strict(vec![public_key.clone()]);
        let verified = verify_package(&ccs_path, &policy).expect("verify CCS reopen fixture");
        let transport = conary_core::ccs::CcsTransportEnvelopeV1::from_verified_archive(&verified)
            .expect("fixture transport");
        let mut ccs_file = File::open(&ccs_path).expect("open fixture CCS");
        let ccs_sha256 =
            conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut ccs_file)
                .expect("hash fixture CCS");
        let package = package(
            "fedora-44",
            "demo",
            "1.0",
            "1",
            Some("x86_64"),
            1,
            "ccs-reopen",
        );
        let result = ServerConversionResult {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            source_profile: Some("fedora-44".to_string()),
            transport,
            total_size: std::fs::metadata(&ccs_path)
                .expect("fixture CCS metadata")
                .len(),
            content_hash: ccs_sha256.to_prefixed_string(),
            ccs_path,
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata {
                scriptlet_fidelity: converted.scriptlet_metadata.scriptlet_fidelity,
                evidence_digest: converted.scriptlet_metadata.evidence_digest,
            },
            timing: None,
        };
        Fixture {
            _directory: directory,
            package,
            result,
            reopener: CcsArtifactReopener::for_public_key(public_key),
        }
    }

    #[test]
    fn persisted_ccs_is_independently_reopened_and_bound_to_exact_evidence() {
        let fixture = fixture();
        let evidence = fixture
            .reopener
            .reopen(&fixture.package, &fixture.result)
            .expect("independently reopen persisted CCS");
        assert_eq!(evidence.source_artifact_sha256, "d".repeat(64));
        assert_eq!(
            evidence.ccs_sha256,
            fixture.result.content_hash.trim_start_matches("sha256:")
        );
        assert_eq!(
            evidence.reopen_proof.ccs_format_version,
            conary_core::ccs::v3::FORMAT_VERSION_V3
        );
        assert_eq!(
            evidence.reopen_proof.verified_objects,
            fixture.result.transport.objects.len() as u64
        );
        assert_eq!(evidence.target_compatibility_proofs.len(), 3);
    }

    #[test]
    fn persisted_ccs_reopen_rejects_tamper_signer_transport_and_identity_drift() {
        let tampered = fixture();
        let mut bytes = std::fs::read(&tampered.result.ccs_path).expect("read fixture CCS");
        let index = bytes.len() / 2;
        bytes[index] ^= 1;
        std::fs::write(&tampered.result.ccs_path, bytes).expect("tamper fixture CCS");
        assert!(
            tampered
                .reopener
                .reopen(&tampered.package, &tampered.result)
                .is_err()
        );

        let signer_drift = fixture();
        let unrelated = conary_core::ccs::signing::SigningKeyPair::generate();
        assert!(
            CcsArtifactReopener::for_public_key(unrelated.public_key_base64())
                .reopen(&signer_drift.package, &signer_drift.result)
                .is_err()
        );

        let mut transport_drift = fixture();
        transport_drift.result.transport.schema_version += 1;
        assert!(
            transport_drift
                .reopener
                .reopen(&transport_drift.package, &transport_drift.result)
                .is_err()
        );

        let mut identity_drift = fixture();
        identity_drift.package.name = "other".to_string();
        assert!(
            identity_drift
                .reopener
                .reopen(&identity_drift.package, &identity_drift.result)
                .is_err()
        );
    }
}
