// conary-core/src/repository/rpm_verifier.rs

//! RPM signature framing backed by Conary's pinned OpenPGP authority.
//!
//! rpm-rs enables experimental post-quantum algorithms in its built-in PGP
//! verifier. Conary uses rpm-rs only to parse exact signed bytes and signatures;
//! pinned certificate policy and cryptographic verification stay in the shared
//! Sequoia-backed repository trust implementation.

use super::trust::TrustRole;
use super::trust::openpgp::PreparedOpenPgpTrust;
use crate::error::{Error, Result};

#[derive(Debug)]
pub(super) struct RpmOpenPgpVerifier<'a> {
    trust: &'a PreparedOpenPgpTrust,
}

impl<'a> RpmOpenPgpVerifier<'a> {
    pub(super) const fn new(trust: &'a PreparedOpenPgpTrust) -> Self {
        Self { trust }
    }

    pub(super) fn verify(&self, package: &rpm::Package) -> Result<()> {
        package.verify_digests().map_err(|error| {
            Error::GpgVerificationFailed(format!("RPM digest verification failed: {error}"))
        })?;
        let signed_bytes = package.header_bytes().map_err(|error| {
            Error::GpgVerificationFailed(format!(
                "failed to reconstruct RPM signed header bytes: {error}"
            ))
        })?;
        let signatures = package.raw_signatures().map_err(|error| {
            Error::GpgVerificationFailed(format!("failed to parse RPM signatures: {error}"))
        })?;
        self.verify_signatures(
            &signed_bytes,
            signatures.iter().map(|signature| signature.as_ref()),
        )
    }

    fn verify_signatures<'b>(
        &self,
        signed_bytes: &[u8],
        signatures: impl IntoIterator<Item = &'b [u8]>,
    ) -> Result<()> {
        let mut last_error = None;
        for signature in signatures {
            match self
                .trust
                .verify_detached(TrustRole::RpmPackage, signed_bytes, signature)
            {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            Error::GpgVerificationFailed(
                "RPM package has no embedded OpenPGP signature".to_string(),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::trust::{OpenPgpTrustRoot, RepositoryTrustPolicy, RpmMetadataAuthority};
    use openpgp::cert::prelude::CertBuilder;
    use openpgp::policy::StandardPolicy;
    use openpgp::serialize::Serialize;
    use openpgp::serialize::stream::{Message, Signer};
    use sequoia_openpgp as openpgp;
    use std::fs;
    use std::io::Write;

    fn detached_signature(certificate: &openpgp::Cert, data: &[u8]) -> Vec<u8> {
        let policy = StandardPolicy::new();
        let keypair = certificate
            .keys()
            .unencrypted_secret()
            .with_policy(&policy, None)
            .supported()
            .alive()
            .revoked(false)
            .for_signing()
            .next()
            .unwrap()
            .key()
            .clone()
            .into_keypair()
            .unwrap();
        let mut signature = Vec::new();
        let message = Message::new(&mut signature);
        let mut signer = Signer::new(message, keypair)
            .unwrap()
            .detached()
            .build()
            .unwrap();
        signer.write_all(data).unwrap();
        signer.finalize().unwrap();
        signature
    }

    fn prepared_trust(
        temp: &tempfile::TempDir,
        certificate: &openpgp::Cert,
    ) -> PreparedOpenPgpTrust {
        let fingerprint = certificate.fingerprint().to_hex();
        let key_path = temp
            .path()
            .join("native/test-repository/rpm-package")
            .join(format!("{fingerprint}.pgp"));
        fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        let mut certificate_bytes = Vec::new();
        certificate.serialize(&mut certificate_bytes).unwrap();
        fs::write(key_path, certificate_bytes).unwrap();
        let policy = RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::Metalink {
                url: "https://mirror.example.test/metalink".to_string(),
            },
            package_keys: vec![OpenPgpTrustRoot {
                url: "https://keys.example.test/package.asc".to_string(),
                fingerprint,
            }],
        };
        PreparedOpenPgpTrust::from_prepared("test-repository", temp.path(), &policy).unwrap()
    }

    fn package() -> rpm::Package {
        rpm::PackageBuilder::new(
            "signed-fixture",
            "1.0",
            "MIT",
            "noarch",
            "RPM signature adapter fixture",
        )
        .build()
        .unwrap()
    }

    #[test]
    fn accepts_pinned_signature_over_exact_header_bytes() {
        let (certificate, _) = CertBuilder::new()
            .add_userid("RPM package signer")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let trust = prepared_trust(&temp, &certificate);
        let package = package();
        let header_bytes = package.header_bytes().unwrap();
        let signature = detached_signature(&certificate, &header_bytes);

        RpmOpenPgpVerifier::new(&trust)
            .verify_signatures(&header_bytes, [signature.as_slice()])
            .unwrap();
    }

    #[test]
    fn rejects_unsigned_or_wrongly_signed_package() {
        let (certificate, _) = CertBuilder::new()
            .add_userid("RPM package signer")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let trust = prepared_trust(&temp, &certificate);

        let unsigned = package();
        let error = RpmOpenPgpVerifier::new(&trust)
            .verify(&unsigned)
            .unwrap_err();
        assert!(error.to_string().contains("no embedded OpenPGP signature"));

        let header_bytes = unsigned.header_bytes().unwrap();
        let wrong_signature = detached_signature(&certificate, b"not the RPM header");
        assert!(
            RpmOpenPgpVerifier::new(&trust)
                .verify_signatures(&header_bytes, [wrong_signature.as_slice()])
                .is_err()
        );
    }
}
