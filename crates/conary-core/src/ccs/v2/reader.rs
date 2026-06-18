// conary-core/src/ccs/v2/reader.rs

use super::schema::AuthorityDocumentV2;
use super::validation::validate_authority;
use crate::ccs::verify::{
    PackageSignature, SignatureStatus, TrustPolicy, verify_manifest_signature,
};
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
    validate_authority(&authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    let signature_raw = signature_raw.context("CCS v2 MANIFEST.sig is required")?;
    let signature: PackageSignature =
        serde_json::from_str(signature_raw).context("parse MANIFEST.sig")?;
    verify_v2_signature(raw_manifest, &signature, policy)?;
    verify_debug_toml_hash(&authority, toml_raw)?;
    reject_install_authority_toml(toml_raw)?;
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
    match verify_manifest_signature(raw_manifest, Some(package_signature), policy)? {
        SignatureStatus::Valid { .. } => Ok(()),
        SignatureStatus::Unsigned => bail!("CCS v2 MANIFEST.sig is required"),
        SignatureStatus::Invalid(reason) => bail!("invalid CCS v2 signature: {reason}"),
        SignatureStatus::Untrusted { key_id } => {
            bail!("CCS v2 package signature key is not trusted: {key_id:?}")
        }
    }
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

fn reject_install_authority_toml(toml_raw: Option<&[u8]>) -> Result<()> {
    let Some(toml_raw) = toml_raw else {
        return Ok(());
    };
    let toml_manifest = crate::ccs::manifest::CcsManifest::parse(
        std::str::from_utf8(toml_raw).context("decode v2 MANIFEST.toml as UTF-8")?,
    )
    .context("parse v2 MANIFEST.toml debug projection")?;
    if !toml_manifest.requires.packages.is_empty()
        || !toml_manifest.requires.capabilities.is_empty()
        || !toml_manifest.config.files.is_empty()
        || toml_manifest.hooks.has_script_hooks()
        || toml_manifest.hooks.has_service_hooks()
        || toml_manifest.hooks.has_declarative_hooks()
        || toml_manifest.scriptlets.has_capability_declarations()
        || toml_manifest.legacy_scriptlets.is_some()
        || !toml_manifest.components.overrides.is_empty()
        || !toml_manifest.components.files.is_empty()
    {
        bail!(
            "v2 MANIFEST.toml contains install-affecting fields; signed CBOR authority is required"
        );
    }
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
    fn v2_debug_toml_with_service_hooks_is_rejected() {
        let toml = r#"
[package]
name = "hello"
version = "0.1.0"
description = "hello"

[[hooks.services]]
name = "hello.service"
action = "restart"
"#;

        let error = reject_install_authority_toml(Some(toml.as_bytes())).unwrap_err();
        assert!(error.to_string().contains("install-affecting"));
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
        assert!(error.to_string().contains("Unsupported algorithm"));
    }
}
