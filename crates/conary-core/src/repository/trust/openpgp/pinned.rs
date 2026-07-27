// conary-core/src/repository/trust/openpgp/pinned.rs

//! Verification against an explicitly pinned certificate set.
//!
//! Debian `Release`/`InRelease` and rpm-md/RPM authority is a *pinned
//! certificate* model: the configuration names exact fingerprints, and only
//! those certificates may sign.  There is no keyring, no certification graph,
//! and no revoked/disabled companion list, so Sequoia's streaming verifier
//! carries the whole decision.
//!
//! Arch is a different authority model (a certifying keyring evaluated
//! against a trust snapshot) and is owned by [`super::arch`]; the two must
//! not be collapsed into one verifier.

use openpgp::parse::Parse;
use openpgp::parse::stream::{
    DetachedVerifierBuilder, MessageLayer, MessageStructure, VerificationHelper, VerifierBuilder,
};
use openpgp::policy::StandardPolicy;
use sequoia_openpgp as openpgp;
use std::io::Read;

#[derive(Clone)]
struct PinnedCertificateHelper {
    certificates: Vec<openpgp::Cert>,
}

impl VerificationHelper for PinnedCertificateHelper {
    fn get_certs(
        &mut self,
        _identities: &[openpgp::KeyHandle],
    ) -> openpgp::Result<Vec<openpgp::Cert>> {
        Ok(self.certificates.clone())
    }

    fn check(&mut self, structure: MessageStructure<'_>) -> openpgp::Result<()> {
        let mut valid = 0usize;
        for layer in structure {
            let MessageLayer::SignatureGroup { results } = layer else {
                return Err(anyhow::anyhow!(
                    "unexpected encrypted or compressed layer in repository signature"
                ));
            };
            for result in results {
                result.map_err(|error| anyhow::anyhow!("{error}"))?;
                valid += 1;
            }
        }
        if valid == 0 {
            return Err(anyhow::anyhow!(
                "repository object has no valid signature from a pinned certificate"
            ));
        }
        Ok(())
    }
}

pub(super) fn verify_detached_with_certificates(
    data: &[u8],
    signature: &[u8],
    certificates: Vec<openpgp::Cert>,
) -> anyhow::Result<()> {
    let policy = StandardPolicy::new();
    let helper = PinnedCertificateHelper { certificates };
    let mut verifier =
        DetachedVerifierBuilder::from_bytes(signature)?.with_policy(&policy, None, helper)?;
    verifier.verify_bytes(data)?;
    Ok(())
}

pub(super) fn verify_inline_with_certificates(
    signed_data: &[u8],
    certificates: Vec<openpgp::Cert>,
) -> anyhow::Result<Vec<u8>> {
    let policy = StandardPolicy::new();
    let helper = PinnedCertificateHelper { certificates };
    let mut verifier =
        VerifierBuilder::from_bytes(signed_data)?.with_policy(&policy, None, helper)?;
    let mut payload = Vec::new();
    verifier.read_to_end(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpgp::cert::prelude::CertBuilder;
    use openpgp::serialize::stream::{Message, Signer};
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

    #[test]
    fn detached_signature_tampering_is_fatal() {
        let (certificate, _) = CertBuilder::new()
            .add_userid("Repository Signer")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let signature = detached_signature(&certificate, b"trusted metadata");

        verify_detached_with_certificates(
            b"trusted metadata",
            &signature,
            vec![certificate.clone()],
        )
        .unwrap();
        assert!(
            verify_detached_with_certificates(b"tampered metadata", &signature, vec![certificate],)
                .is_err()
        );
    }

    #[test]
    fn unpinned_certificate_cannot_sign() {
        let (pinned, _) = CertBuilder::new()
            .add_userid("Pinned Signer")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let (other, _) = CertBuilder::new()
            .add_userid("Other Signer")
            .add_signing_subkey()
            .generate()
            .unwrap();
        let signature = detached_signature(&other, b"trusted metadata");
        assert!(
            verify_detached_with_certificates(b"trusted metadata", &signature, vec![pinned])
                .is_err()
        );
    }
}
