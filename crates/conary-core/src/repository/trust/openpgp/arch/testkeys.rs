// conary-core/src/repository/trust/openpgp/arch/testkeys.rs

//! Synthetic OpenPGP material for Arch trust tests.
//!
//! These builders exist so the negative cases (forged bindings, revocations,
//! disabled keys, invalid time relations) can be expressed exactly instead of
//! being approximated against real keyring bytes.

use openpgp::cert::prelude::CertBuilder;
use openpgp::packet::Packet;
use openpgp::packet::signature::SignatureBuilder;
use openpgp::policy::StandardPolicy;
use openpgp::serialize::Serialize;
use openpgp::types::{ReasonForRevocation, SignatureType};
use sequoia_openpgp as openpgp;
use std::time::{Duration, SystemTime};

use super::super::super::{ArchKeyringFormat, ArchKeyringTrust};

pub(crate) fn trust(master_fingerprints: Vec<String>, threshold: usize) -> ArchKeyringTrust {
    ArchKeyringTrust {
        url: "https://keys.example.test/archlinux-keyring.pkg.tar.zst".to_string(),
        format: ArchKeyringFormat::AlpmPackageZstd,
        master_fingerprints,
        packager_key_threshold: threshold,
    }
}

pub(crate) fn master(name: &str) -> openpgp::Cert {
    CertBuilder::new().add_userid(name).generate().unwrap().0
}

pub(crate) fn expiring_master(name: &str) -> openpgp::Cert {
    CertBuilder::new()
        .add_userid(name)
        .set_validity_period(Duration::from_secs(3600 * 24))
        .generate()
        .unwrap()
        .0
}

/// How far in the past synthetic packager keys are created.
///
/// Signatures in these tests are dated in the past to exercise the
/// signature-time relations, and GnuPG rejects a key that is newer than the
/// signature it made (`g10/sig-check.c:363-383`), so the key must exist first.
pub(crate) const PACKAGER_KEY_AGE: Duration = Duration::from_secs(3600 * 24 * 365);

pub(crate) fn packager(name: &str) -> openpgp::Cert {
    packager_with_secret(name).0
}

/// Returns `(public certificate, certificate with secret key material)`.
pub(crate) fn packager_with_secret(name: &str) -> (openpgp::Cert, openpgp::Cert) {
    let (cert, _) = CertBuilder::new()
        .add_userid(name)
        .add_signing_subkey()
        .set_creation_time(SystemTime::now() - PACKAGER_KEY_AGE)
        .generate()
        .unwrap();
    (cert.clone().strip_secret_key_material(), cert)
}

fn certification_signer(cert: &openpgp::Cert) -> openpgp::crypto::KeyPair {
    let policy = StandardPolicy::new();
    cert.keys()
        .unencrypted_secret()
        .with_policy(&policy, None)
        .supported()
        .alive()
        .revoked(false)
        .for_certification()
        .next()
        .unwrap()
        .key()
        .clone()
        .into_keypair()
        .unwrap()
}

fn signing_signer(cert: &openpgp::Cert) -> openpgp::crypto::KeyPair {
    let policy = StandardPolicy::new();
    cert.keys()
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
        .unwrap()
}

/// Certifies the packager's first user ID with the master key.
pub(crate) fn certify(master: &openpgp::Cert, packager: openpgp::Cert) -> openpgp::Cert {
    let mut signer = certification_signer(master);
    let userid = packager.userids().next().unwrap().userid().clone();
    let certification = SignatureBuilder::new(SignatureType::GenericCertification)
        .sign_userid_binding(&mut signer, packager.primary_key().key(), &userid)
        .unwrap();
    packager.insert_packets(certification).unwrap().0
}

/// Hard-revokes a certificate with its own primary key.
pub(crate) fn revoke(cert: openpgp::Cert, secret: &openpgp::Cert) -> openpgp::Cert {
    let mut signer = certification_signer(secret);
    let revocation = openpgp::cert::CertRevocationBuilder::new()
        .set_reason_for_revocation(ReasonForRevocation::KeyCompromised, b"test")
        .unwrap()
        .build(&mut signer, &cert, None)
        .unwrap();
    cert.insert_packets(revocation).unwrap().0
}

/// Splices a foreign signing subkey into `cert` with no binding signature.
pub(crate) fn splice_unbound_subkey(cert: openpgp::Cert, foreign: &openpgp::Cert) -> openpgp::Cert {
    let policy = StandardPolicy::new();
    let subkey = foreign
        .keys()
        .subkeys()
        .with_policy(&policy, None)
        .for_signing()
        .next()
        .unwrap()
        .key()
        .clone();
    cert.insert_packets(Packet::PublicSubkey(subkey.role_as_subordinate().clone()))
        .unwrap()
        .0
}

/// Replaces every user-ID self-signature with one created now.
///
/// This reproduces the `archlinux-keyring` export shape that issue #102 hit:
/// the certificate carries exactly one self-signature, and it is newer than
/// the package signatures the packager already made.
pub(crate) fn refresh_self_signatures(
    cert: openpgp::Cert,
    secret: &openpgp::Cert,
) -> openpgp::Cert {
    let policy = StandardPolicy::new();
    let mut signer = certification_signer(secret);
    let binding = cert.userids().next().unwrap();
    let userid = binding.userid().clone();
    let old = binding.binding_signature(&policy, None).unwrap().clone();
    let refreshed = SignatureBuilder::from(old.clone())
        .set_signature_creation_time(SystemTime::now())
        .unwrap()
        .sign_userid_binding(&mut signer, cert.primary_key().key(), &userid)
        .unwrap();

    let retained = cert
        .into_packets()
        .filter(|packet| !matches!(packet, Packet::Signature(signature) if *signature == old))
        .collect::<Vec<_>>();
    openpgp::Cert::from_packets(retained.into_iter())
        .unwrap()
        .insert_packets(refreshed)
        .unwrap()
        .0
}

fn serialize(signature: openpgp::packet::Signature) -> Vec<u8> {
    let mut bytes = Vec::new();
    Packet::from(signature).serialize(&mut bytes).unwrap();
    bytes
}

pub(crate) fn detached_signature(secret: &openpgp::Cert, data: &[u8]) -> Vec<u8> {
    let mut signer = signing_signer(secret);
    serialize(
        SignatureBuilder::new(SignatureType::Binary)
            .sign_message(&mut signer, data)
            .unwrap(),
    )
}

pub(crate) fn detached_signature_at(
    secret: &openpgp::Cert,
    data: &[u8],
    created: SystemTime,
) -> Vec<u8> {
    let mut signer = signing_signer(secret);
    serialize(
        SignatureBuilder::new(SignatureType::Binary)
            .set_signature_creation_time(created)
            .unwrap()
            .sign_message(&mut signer, data)
            .unwrap(),
    )
}

pub(crate) fn expiring_detached_signature(secret: &openpgp::Cert, data: &[u8]) -> Vec<u8> {
    let mut signer = signing_signer(secret);
    serialize(
        SignatureBuilder::new(SignatureType::Binary)
            .set_signature_creation_time(SystemTime::now() - Duration::from_secs(7200))
            .unwrap()
            .set_signature_validity_period(Duration::from_secs(3600))
            .unwrap()
            .sign_message(&mut signer, data)
            .unwrap(),
    )
}
