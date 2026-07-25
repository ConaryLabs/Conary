// conary-core/src/ccs/v2/reader.rs

use super::schema::AuthorityDocumentV2;
use super::validation::validate_authority_structure;
use crate::ccs::verify::{PackageSignature, TrustPolicy, VerifyError, verify_manifest_signature};
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct ReadAuthorityV2 {
    pub authority: AuthorityDocumentV2,
    pub raw_manifest: Vec<u8>,
    pub signature: PackageSignature,
    pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
}

pub fn read_authority_document(
    raw_manifest: &[u8],
    signature_raw: Option<&str>,
    toml_raw: Option<&[u8]>,
    build_attestation_raw: Option<&str>,
    foreign_conversion_boundary_raw: Option<&str>,
    policy: &TrustPolicy,
) -> Result<ReadAuthorityV2> {
    let authority =
        AuthorityDocumentV2::from_cbor(raw_manifest).context("decode CCS v2 MANIFEST")?;
    validate_authority_structure(&authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    let signature_raw = signature_raw.ok_or(VerifyError::NotSigned)?;
    let signature: PackageSignature =
        serde_json::from_str(signature_raw).context("parse MANIFEST.sig")?;
    verify_v2_signature(raw_manifest, &signature, policy)?;
    verify_debug_toml_hash(&authority, toml_raw)?;
    validate_debug_toml(&authority, toml_raw)?;
    let build_attestation = build_attestation_raw
        .map(serde_json::from_str)
        .transpose()
        .context("parse MANIFEST.attestation.json")?;
    let foreign_conversion_boundary = foreign_conversion_boundary_raw
        .map(serde_json::from_str)
        .transpose()
        .context("parse MANIFEST.conversion-boundary.json")?;
    verify_conversion_boundary_hash(&authority, foreign_conversion_boundary.as_ref())?;
    Ok(ReadAuthorityV2 {
        authority,
        raw_manifest: raw_manifest.to_vec(),
        signature,
        build_attestation,
        foreign_conversion_boundary,
    })
}

fn verify_v2_signature(
    raw_manifest: &[u8],
    package_signature: &PackageSignature,
    policy: &TrustPolicy,
) -> Result<()> {
    verify_manifest_signature(raw_manifest, package_signature, policy)
        .context("verify CCS v2 MANIFEST signature")
}

fn verify_debug_toml_hash(authority: &AuthorityDocumentV2, toml_raw: Option<&[u8]>) -> Result<()> {
    if let Some(expected) = &authority.debug_toml_sha256 {
        let toml_raw =
            toml_raw.context("v2 debug TOML hash present but MANIFEST.toml is missing")?;
        let actual = crate::hash::sha256(toml_raw);
        if &actual != expected {
            bail!("v2 TOML manifest integrity check failed: expected {expected}, got {actual}");
        }
    }
    Ok(())
}

fn validate_debug_toml(authority: &AuthorityDocumentV2, toml_raw: Option<&[u8]>) -> Result<()> {
    let Some(toml_raw) = toml_raw else {
        return Ok(());
    };
    let toml_manifest = crate::ccs::manifest::CcsManifest::parse(
        std::str::from_utf8(toml_raw).context("decode v2 MANIFEST.toml as UTF-8")?,
    )
    .context("parse v2 MANIFEST.toml debug projection")?;
    super::debug_projection::reject_unsupported_debug_toml_install_authority(&toml_manifest)?;
    super::debug_projection::validate_debug_toml_projection(authority, &toml_manifest)?;
    Ok(())
}

fn verify_conversion_boundary_hash(
    authority: &AuthorityDocumentV2,
    boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    if let Some(expected) = &authority.provenance.foreign_conversion_boundary_hash {
        let boundary = boundary.context(
            "v2 foreign conversion boundary hash present but MANIFEST.conversion-boundary.json is missing",
        )?;
        let actual = crate::ccs::attestation::canonical_json_hash(boundary)?;
        if &actual != expected {
            bail!(
                "v2 foreign conversion boundary hash mismatch: expected {expected}, got {actual}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::signing::SigningKeyPair;
    use crate::ccs::verify::TrustPolicy;

    #[test]
    fn verifies_signature_against_exact_archived_manifest_bytes() {
        let authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("signed");
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            None,
            None,
            None,
            &policy,
        )
        .unwrap();

        let mut drifted = raw.clone();
        drifted.push(0);
        assert!(
            read_authority_document(
                &drifted,
                Some(&serde_json::to_string(&signature).unwrap()),
                None,
                None,
                None,
                &policy,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_toml_debug_drift() {
        let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("debug");
        authority.debug_toml_sha256 = Some(crate::hash::sha256(b"original"));
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        let error = read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            Some(b"modified"),
            None,
            None,
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("TOML"));
    }

    #[test]
    fn reader_accepts_debug_toml_config_when_signed_projection_matches() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"

[config]
files = ["/etc/conary-example/config.toml"]
noreplace = true
"#;
        let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("demo");
        authority.debug_toml_sha256 = Some(crate::hash::sha256(toml.as_bytes()));
        if let crate::ccs::v2::schema::PackageKindV2::Package(package) = &mut authority.kind {
            let file = package.files.first_mut().unwrap();
            file.path = "/etc/conary-example/config.toml".to_string();
            file.node.mode = libc::S_IFREG | 0o644;
            file.config = Some(crate::ccs::v2::schema::ConfigPolicyV2::NoReplace);
            package
                .config
                .push(crate::ccs::v2::schema::ConfigAuthorityV2 {
                    path: "/etc/conary-example/config.toml".to_string(),
                    policy: crate::ccs::v2::schema::ConfigPolicyV2::NoReplace,
                });
        }
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            Some(toml.as_bytes()),
            None,
            None,
            &policy,
        )
        .unwrap();
    }

    #[test]
    fn reader_accepts_lifecycle_authority_without_no_profile_rejection() {
        let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("svc");
        authority.lifecycle.services = vec![crate::ccs::v2::schema::LifecycleServiceV2 {
            name: "conary-example.service".to_string(),
            action: crate::ccs::v2::schema::LifecycleServiceActionV2::Restart,
            reversible: None,
        }];
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            None,
            None,
            None,
            &policy,
        )
        .unwrap();
    }

    #[test]
    fn reader_accepts_debug_toml_lifecycle_when_signed_projection_matches() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"

[[hooks.services]]
name = "conary-example.service"
action = "restart"
"#;
        let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("demo");
        authority.debug_toml_sha256 = Some(crate::hash::sha256(toml.as_bytes()));
        authority.lifecycle.services = vec![crate::ccs::v2::schema::LifecycleServiceV2 {
            name: "conary-example.service".to_string(),
            action: crate::ccs::v2::schema::LifecycleServiceActionV2::Restart,
            reversible: None,
        }];
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            Some(toml.as_bytes()),
            None,
            None,
            &policy,
        )
        .unwrap();
    }

    #[test]
    fn v2_debug_toml_requirements_must_match_signed_authority() {
        let toml = r#"
[package]
name = "hello"
version = "0.1.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "hello"
"#;

        let mut manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();
        manifest.requirements.push(
            crate::repository::requirement::parse_native_requirement(
                crate::repository::dependency_model::RepositoryRequirementKind::Depends,
                crate::repository::versioning::VersionScheme::Conary,
                "openssl >= 3.0.0",
            )
            .unwrap(),
        );
        let authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("hello");
        let error =
            crate::ccs::v2::debug_projection::validate_debug_toml_projection(&authority, &manifest)
                .unwrap_err();
        assert!(error.to_string().contains("requirement projection"));
    }

    #[test]
    fn rejects_modified_manifest_signature() {
        let authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("tamper");
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let mut signature = key.sign(&raw);
        signature.signature.push_str("AA");
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        assert!(
            read_authority_document(
                &raw,
                Some(&serde_json::to_string(&signature).unwrap()),
                None,
                None,
                None,
                &policy,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unsupported_signature_algorithms() {
        let authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("algo");
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let mut signature = key.sign(&raw);
        signature.algorithm = "rsa".to_string();
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        let error = read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            None,
            None,
            None,
            &policy,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("unsupported algorithm"));
    }
}
