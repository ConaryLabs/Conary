// conary-core/src/repository/trust/openpgp/arch/verify.rs

//! Pacman-conformant verification of an Arch detached signature.
//!
//! # Why this is not Sequoia's streaming verifier
//!
//! `sequoia_openpgp::parse::stream` binds a signing key with
//! `ka.with_policy(policy, sig_time)` (`src/parse/stream.rs`): the whole
//! certificate is evaluated *as of the moment the document was signed*, so a
//! key needs a self-signature that predates the signature.
//!
//! GnuPG -- the engine GPGME and therefore `libalpm` run -- does the
//! opposite.  `g10/getkey.c:2914` and `:3427` select the *latest* self-
//! signature and subkey binding, and `:3220`/`:3472` judge expiry against the
//! current time.  `g10/sig-check.c:363-435` then ties only two relations to
//! the document signature's own timestamp: the key must not be newer than the
//! signature, and the signature must not have expired.
//!
//! `archlinux-keyring` exports one self-signature per component, so as soon
//! as a packager refreshes their self-signature every package they signed
//! earlier loses its binding under the streaming verifier while pacman still
//! accepts it.  Reproducing pacman means keeping the two reference times
//! apart, which is what this module does.
//!
//! # Result classes
//!
//! [`ArchSignatureStatus`] is `alpm_sigstatus_t` and
//! [`ArchSignatureValidity`] is `alpm_sigvalidity_t`
//! (`lib/libalpm/signing.c:676-717`).  Acceptance reproduces
//! `_alpm_check_pgp_helper` under `TrustedOnly`, i.e. with `marginal` and
//! `unknown` both zero (`lib/libalpm/signing.c:828-863`).

use crate::error::{Error, Result};
use openpgp::PacketPile;
use openpgp::cert::amalgamation::ValidAmalgamation;
use openpgp::packet::Signature;
use openpgp::parse::Parse;
use openpgp::policy::StandardPolicy;
use openpgp::types::{RevocationStatus, SignatureType};
use sequoia_openpgp as openpgp;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::snapshot::{ArchSignatureValidity, ArchTrustSnapshot};

/// GnuPG's tolerance for a signature made slightly in the future.
///
/// `g10/sig-check.c` logs a clock-skew note but does not reject, and
/// `signing.c:637` likewise only logs.  Sequoia demands an explicit value.
const CLOCK_SKEW_TOLERANCE: Duration = Duration::from_secs(0);

/// `alpm_sigstatus_t` (`lib/libalpm/alpm.h`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchSignatureStatus {
    /// `GPG_ERR_NO_ERROR` -> `ALPM_SIGSTATUS_VALID`.
    Valid,
    /// `GPG_ERR_KEY_EXPIRED` -> `ALPM_SIGSTATUS_KEY_EXPIRED`.
    KeyExpired,
    /// `GPG_ERR_SIG_EXPIRED` -> `ALPM_SIGSTATUS_SIG_EXPIRED`.
    SigExpired,
    /// `GPG_ERR_NO_PUBKEY` -> `ALPM_SIGSTATUS_KEY_UNKNOWN`.
    KeyUnknown,
    /// `key->disabled` -> `ALPM_SIGSTATUS_KEY_DISABLED`.
    KeyDisabled,
    /// `GPG_ERR_BAD_SIGNATURE` and every other status -> `ALPM_SIGSTATUS_INVALID`.
    Invalid(ArchInvalidReason),
}

/// Why a signature classified as `ALPM_SIGSTATUS_INVALID`.
///
/// Pacman collapses all of these into one status; Conary keeps the exact
/// cause so a rejection is inspectable without re-running the verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchInvalidReason {
    /// The signing key was created after the signature, or after the trust
    /// snapshot (`g10/sig-check.c:363-408`).
    TimeConflict,
    /// No valid self-signature or subkey binding at the trust-snapshot time.
    KeyNotBound,
    /// The certificate or the signing key is revoked (`REVKEYSIG`).
    KeyRevoked,
    /// The signing key carries no signing key flag.
    KeyNotSigningCapable,
    /// The cryptographic check failed, i.e. the bytes were tampered with.
    BadSignature,
}

/// One signature's outcome, in pacman's own vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchSignatureResult {
    pub status: ArchSignatureStatus,
    pub validity: ArchSignatureValidity,
    pub signature_created: SystemTime,
    pub trust_snapshot: SystemTime,
    /// Fingerprint of the key that made the signature, when it is known.
    pub signing_key: Option<String>,
    /// Fingerprint of the certificate owning that key, when it is known.
    pub certificate: Option<String>,
}

impl ArchSignatureResult {
    /// `_alpm_check_pgp_helper` with `marginal = 0` and `unknown = 0`.
    pub fn accepted_under_trusted_only(&self) -> bool {
        matches!(
            self.status,
            ArchSignatureStatus::Valid | ArchSignatureStatus::KeyExpired
        ) && self.validity == ArchSignatureValidity::Full
    }

    fn describe(&self) -> String {
        let signer = self.signing_key.as_deref().unwrap_or("an unidentified key");
        match &self.status {
            ArchSignatureStatus::Valid => {
                format!("signature by {signer} is {:?} trust", self.validity)
            }
            ArchSignatureStatus::KeyExpired => format!(
                "signing key {signer} is expired at the trust-snapshot time and the certificate \
                 is {:?} trust",
                self.validity
            ),
            ArchSignatureStatus::SigExpired => {
                format!("signature by {signer} expired before the trust-snapshot time")
            }
            ArchSignatureStatus::KeyUnknown => {
                format!("no authenticated keyring certificate owns signing key {signer}")
            }
            ArchSignatureStatus::KeyDisabled => {
                format!("signing key {signer} is disabled by the keyring revoked list")
            }
            ArchSignatureStatus::Invalid(reason) => {
                format!("signature by {signer} is invalid: {reason:?}")
            }
        }
    }
}

/// Verifies a detached ALPM signature against an authenticated snapshot.
///
/// Every signature in the object must be acceptable, matching
/// `_alpm_check_pgp_helper`'s `for(num = 0; !ret && num < count; num++)` loop.
pub fn verify_detached(
    snapshot: &ArchTrustSnapshot,
    data: &[u8],
    signature: &[u8],
) -> Result<Vec<ArchSignatureResult>> {
    let signatures = parse_signatures(signature)?;
    signatures
        .iter()
        .map(|signature| evaluate(snapshot, data, signature))
        .collect()
}

/// Verifies and enforces pacman's `TrustedOnly` policy.
pub fn check_detached(
    snapshot: &ArchTrustSnapshot,
    data: &[u8],
    signature: &[u8],
) -> Result<Vec<ArchSignatureResult>> {
    let results = verify_detached(snapshot, data, signature)?;
    if let Some(rejected) = results
        .iter()
        .find(|result| !result.accepted_under_trusted_only())
    {
        return Err(Error::GpgVerificationFailed(rejected.describe()));
    }
    Ok(results)
}

/// File-backed form of [`verify_detached`] for distribution-sized ALPM
/// database and package objects.
pub fn verify_detached_file(
    snapshot: &ArchTrustSnapshot,
    path: &Path,
    signature: &[u8],
) -> Result<Vec<ArchSignatureResult>> {
    let signatures = parse_signatures(signature)?;
    signatures
        .iter()
        .map(|signature| evaluate_file(snapshot, path, signature))
        .collect()
}

/// File-backed form of [`check_detached`].
pub fn check_detached_file(
    snapshot: &ArchTrustSnapshot,
    path: &Path,
    signature: &[u8],
) -> Result<Vec<ArchSignatureResult>> {
    let results = verify_detached_file(snapshot, path, signature)?;
    if let Some(rejected) = results
        .iter()
        .find(|result| !result.accepted_under_trusted_only())
    {
        return Err(Error::GpgVerificationFailed(rejected.describe()));
    }
    Ok(results)
}

/// Parses the detached signature object.
///
/// `gpgme_op_verify` fails with `ALPM_ERR_SIG_MISSING` when the object
/// carries no signatures (`lib/libalpm/signing.c:611-614`); anything that is
/// not a signature packet is not part of the detached-signature grammar.
fn parse_signatures(bytes: &[u8]) -> Result<Vec<Signature>> {
    let pile = PacketPile::from_bytes(bytes).map_err(|error| {
        Error::ParseError(format!("failed to parse Arch detached signature: {error}"))
    })?;
    let mut signatures = Vec::new();
    for packet in pile.into_children() {
        match packet {
            openpgp::Packet::Signature(signature) => signatures.push(signature),
            other => {
                return Err(Error::ParseError(format!(
                    "Arch detached signature contains an unexpected {} packet",
                    other.tag()
                )));
            }
        }
    }
    if signatures.is_empty() {
        return Err(Error::GpgVerificationFailed(
            "Arch detached signature contains no signature packet".to_string(),
        ));
    }
    Ok(signatures)
}

fn evaluate(
    snapshot: &ArchTrustSnapshot,
    data: &[u8],
    signature: &Signature,
) -> Result<ArchSignatureResult> {
    evaluate_document(snapshot, signature, |signature, key| {
        verify_bytes(signature, key, data)
    })
}

fn evaluate_file(
    snapshot: &ArchTrustSnapshot,
    path: &Path,
    signature: &Signature,
) -> Result<ArchSignatureResult> {
    evaluate_document(snapshot, signature, |signature, key| {
        verify_file(signature, key, path)
    })
}

fn evaluate_document(
    snapshot: &ArchTrustSnapshot,
    signature: &Signature,
    verify_document: impl FnOnce(
        &Signature,
        &openpgp::packet::Key<
            openpgp::packet::key::PublicParts,
            openpgp::packet::key::UnspecifiedRole,
        >,
    ) -> anyhow::Result<()>,
) -> Result<ArchSignatureResult> {
    let policy = StandardPolicy::new();
    let snapshot_time = snapshot.snapshot_time();
    let signature_created = signature.signature_creation_time().ok_or_else(|| {
        Error::GpgVerificationFailed(
            "Arch signature has no creation time subpacket; GnuPG requires one to relate the \
             signature to the signing key"
                .to_string(),
        )
    })?;

    // ALPM signs package and database bytes as binary documents.  Text-mode
    // signatures hash a canonicalized form and are a different grammar.
    if signature.typ() != SignatureType::Binary {
        return Err(Error::GpgVerificationFailed(format!(
            "Arch signature has type {:?}; only binary document signatures are ALPM signatures",
            signature.typ()
        )));
    }
    if signature.version() != 4 {
        return Err(Error::GpgVerificationFailed(format!(
            "Arch signature version {} is not the v4 grammar ALPM produces",
            signature.version()
        )));
    }

    let issuers = signature.get_issuers();
    if issuers.is_empty() {
        return Err(Error::GpgVerificationFailed(
            "Arch signature names no issuer; a wildcard signer cannot be looked up in the \
             keyring"
                .to_string(),
        ));
    }

    let mut result = ArchSignatureResult {
        status: ArchSignatureStatus::KeyUnknown,
        validity: ArchSignatureValidity::Unknown,
        signature_created,
        trust_snapshot: snapshot_time,
        signing_key: None,
        certificate: None,
    };

    let Some(entry) = issuers.iter().find_map(|handle| snapshot.lookup(handle)) else {
        result.signing_key = issuers.first().map(|handle| handle.to_hex());
        return Ok(result);
    };
    let certificate = entry.certificate();
    result.certificate = Some(certificate.fingerprint().to_hex());
    result.validity = entry.validity(snapshot.threshold());

    let Some(signing_key) = certificate.keys().find(|key| {
        issuers
            .iter()
            .any(|handle| key.key().key_handle().aliases(handle))
    }) else {
        result.signing_key = issuers.first().map(|handle| handle.to_hex());
        return Ok(result);
    };
    let signing_key = signing_key.key().clone();
    result.signing_key = Some(signing_key.fingerprint().to_hex());

    // `pacman-key --populate` disables every fingerprint in the keyring's
    // revoked list; `signing.c:695-698` reads that back as a status of its
    // own, ahead of any GPGME status.
    if entry.disabled() {
        result.status = ArchSignatureStatus::KeyDisabled;
        return Ok(result);
    }

    // `g10/sig-check.c:363-408`: the only two relations GnuPG ties to the
    // document signature's own timestamp.
    let key_created = signing_key.creation_time();
    if key_created > signature_created || key_created > snapshot_time {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::TimeConflict);
        return Ok(result);
    }

    // Everything below is judged at the trust-snapshot time, exactly as
    // `merge_selfsigs` builds the effective key from the latest self-
    // signature and compares expiry against `curtime`.
    let Ok(valid_certificate) = certificate.with_policy(&policy, snapshot_time) else {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::KeyNotBound);
        return Ok(result);
    };
    if matches!(
        valid_certificate.revocation_status(),
        RevocationStatus::Revoked(_)
    ) {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::KeyRevoked);
        return Ok(result);
    }
    let Some(bound_key) = valid_certificate
        .keys()
        .find(|key| key.key().fingerprint() == signing_key.fingerprint())
    else {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::KeyNotBound);
        return Ok(result);
    };
    if matches!(bound_key.revocation_status(), RevocationStatus::Revoked(_)) {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::KeyRevoked);
        return Ok(result);
    }
    if !bound_key.for_signing() {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::KeyNotSigningCapable);
        return Ok(result);
    }

    if verify_document(signature, &signing_key).is_err() {
        result.status = ArchSignatureStatus::Invalid(ArchInvalidReason::BadSignature);
        return Ok(result);
    }

    // `g10/sig-check.c`: an expired signature is fatal, an expired key is a
    // status GnuPG reports and `_alpm_check_pgp_helper` still calls valid.
    // An expired *certificate* separately loses its trust validity, which is
    // what actually stops it (`g10/trustdb.c:2070`).
    if signature
        .signature_alive(snapshot_time, CLOCK_SKEW_TOLERANCE)
        .is_err()
    {
        result.status = ArchSignatureStatus::SigExpired;
        return Ok(result);
    }
    if valid_certificate.alive().is_err() || bound_key.alive().is_err() {
        result.status = ArchSignatureStatus::KeyExpired;
        return Ok(result);
    }

    result.status = ArchSignatureStatus::Valid;
    Ok(result)
}

fn verify_bytes(
    signature: &Signature,
    key: &openpgp::packet::Key<
        openpgp::packet::key::PublicParts,
        openpgp::packet::key::UnspecifiedRole,
    >,
    data: &[u8],
) -> anyhow::Result<()> {
    let mut context = signature
        .hash_algo()
        .context()?
        .for_signature(signature.version());
    context.update(data);
    signature.verify_hash(key, context)
}

fn verify_file(
    signature: &Signature,
    key: &openpgp::packet::Key<
        openpgp::packet::key::PublicParts,
        openpgp::packet::key::UnspecifiedRole,
    >,
    path: &Path,
) -> anyhow::Result<()> {
    let mut context = signature
        .hash_algo()
        .context()?
        .for_signature(signature.version());
    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0_u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    signature.verify_hash(key, context)
}

#[cfg(test)]
mod tests {
    use super::super::keyring::ArchDisabledKeys;
    use super::super::testkeys;
    use super::*;
    use std::collections::BTreeSet;

    const DATA: &[u8] = b"alpm package bytes";

    fn snapshot_of(
        certificates: Vec<openpgp::Cert>,
        masters: Vec<String>,
        threshold: usize,
    ) -> ArchTrustSnapshot {
        ArchTrustSnapshot::authenticate(
            certificates,
            &ArchDisabledKeys::NotCarried,
            &testkeys::trust(masters, threshold),
            SystemTime::now(),
        )
        .unwrap()
    }

    #[test]
    fn a_certified_packager_signature_is_valid_and_fully_trusted() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::detached_signature(&secret, DATA);
        let snapshot = snapshot_of(
            vec![master.clone(), packager],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = check_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ArchSignatureStatus::Valid);
        assert_eq!(results[0].validity, ArchSignatureValidity::Full);
    }

    #[test]
    fn tampered_package_bytes_fail_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::detached_signature(&secret, DATA);
        let snapshot = snapshot_of(
            vec![master.clone(), packager],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = verify_detached(&snapshot, b"tampered package bytes", &signature).unwrap();
        assert_eq!(
            results[0].status,
            ArchSignatureStatus::Invalid(ArchInvalidReason::BadSignature)
        );
        assert!(check_detached(&snapshot, b"tampered package bytes", &signature).is_err());
    }

    #[test]
    fn tampered_signature_bytes_fail_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let mut signature = testkeys::detached_signature(&secret, DATA);
        let last = signature.len() - 1;
        signature[last] ^= 0xff;
        let snapshot = snapshot_of(
            vec![master.clone(), packager],
            vec![master.fingerprint().to_hex()],
            1,
        );

        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn an_unknown_signing_key_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (_, outsider) = testkeys::packager_with_secret("Outsider");
        let signature = testkeys::detached_signature(&outsider, DATA);
        let snapshot = snapshot_of(vec![master.clone()], vec![master.fingerprint().to_hex()], 1);

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(results[0].status, ArchSignatureStatus::KeyUnknown);
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn an_uncertified_keyring_key_is_unknown_trust_and_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Uncertified Packager");
        let signature = testkeys::detached_signature(&secret, DATA);
        let snapshot = snapshot_of(
            vec![master.clone(), packager],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        // Cryptographically good, but the keyring gives it no validity.
        assert_eq!(results[0].status, ArchSignatureStatus::Valid);
        assert_eq!(results[0].validity, ArchSignatureValidity::Unknown);
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn a_disabled_key_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::detached_signature(&secret, DATA);
        let disabled = ArchDisabledKeys::Carried(BTreeSet::from([packager.fingerprint().to_hex()]));
        let snapshot = ArchTrustSnapshot::authenticate(
            vec![master.clone(), packager],
            &disabled,
            &testkeys::trust(vec![master.fingerprint().to_hex()], 1),
            SystemTime::now(),
        )
        .unwrap();

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(results[0].status, ArchSignatureStatus::KeyDisabled);
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn a_revoked_packager_certificate_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::detached_signature(&secret, DATA);
        let revoked = testkeys::revoke(packager, &secret);
        let snapshot = snapshot_of(
            vec![master.clone(), revoked],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        // A revoked certificate is dropped from the web of trust outright,
        // so it is unknown trust as well as an invalid status.
        assert_eq!(results[0].validity, ArchSignatureValidity::Unknown);
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn a_forged_subkey_binding_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, _) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let (foreign, foreign_secret) = testkeys::packager_with_secret("Foreign Signer");
        let signature = testkeys::detached_signature(&foreign_secret, DATA);
        // Splice the foreign signing subkey into the certified packager
        // certificate without a binding signature.
        let spliced = testkeys::splice_unbound_subkey(packager, &foreign);
        let snapshot = snapshot_of(
            vec![master.clone(), spliced],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(
            results[0].status,
            ArchSignatureStatus::Invalid(ArchInvalidReason::KeyNotBound)
        );
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn a_signature_made_before_the_key_existed_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::detached_signature_at(
            &secret,
            DATA,
            SystemTime::now() - Duration::from_secs(3600 * 24 * 365 * 10),
        );
        let snapshot = snapshot_of(
            vec![master.clone(), packager],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(
            results[0].status,
            ArchSignatureStatus::Invalid(ArchInvalidReason::TimeConflict)
        );
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn an_expired_signature_fails_closed() {
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::expiring_detached_signature(&secret, DATA);
        let snapshot = snapshot_of(
            vec![master.clone(), packager],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = verify_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(results[0].status, ArchSignatureStatus::SigExpired);
        assert!(check_detached(&snapshot, DATA, &signature).is_err());
    }

    #[test]
    fn a_signature_object_without_signature_packets_fails_closed() {
        let master = testkeys::master("Arch Master");
        let snapshot = snapshot_of(vec![master.clone()], vec![master.fingerprint().to_hex()], 1);
        let error = verify_detached(&snapshot, DATA, b"").unwrap_err();
        assert!(error.to_string().contains("no signature packet"));
    }

    #[test]
    fn a_signature_made_after_the_latest_self_signature_still_verifies() {
        // The production class from issue #102: `archlinux-keyring` exports
        // only the newest self-signature, so the certificate has no binding
        // at the moment the package was signed.
        let master = testkeys::master("Arch Master");
        let (packager, secret) = testkeys::packager_with_secret("Arch Packager");
        let packager = testkeys::certify(&master, packager);
        let signature = testkeys::detached_signature_at(
            &secret,
            DATA,
            SystemTime::now() - Duration::from_secs(3600),
        );
        let refreshed = testkeys::refresh_self_signatures(packager, &secret);
        let snapshot = snapshot_of(
            vec![master.clone(), refreshed],
            vec![master.fingerprint().to_hex()],
            1,
        );

        let results = check_detached(&snapshot, DATA, &signature).unwrap();
        assert_eq!(results[0].status, ArchSignatureStatus::Valid);
        assert_eq!(results[0].validity, ArchSignatureValidity::Full);
        assert!(results[0].signature_created < results[0].trust_snapshot);
    }
}
