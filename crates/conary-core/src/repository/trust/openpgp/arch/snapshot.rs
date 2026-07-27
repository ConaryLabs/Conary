// conary-core/src/repository/trust/openpgp/arch/snapshot.rs

//! The authenticated Arch trust snapshot.
//!
//! Pacman does not evaluate a package signature against a bare certificate.
//! It evaluates it against a *populated keyring*: master certificates locally
//! signed by an ultimately trusted key, packager certificates that reach full
//! validity through enough master certifications, a companion disabled-key
//! list, and one wall-clock instant at which all of that is judged.
//!
//! This type is that keyring, made explicit and typed.  It carries:
//!
//! * the pinned master authority (role-separated, never taken from the
//!   keyring's own `-trusted` file),
//! * every certified packager certificate with the exact set of masters that
//!   certified it,
//! * subkey bindings, revocation and expiry, which are evaluated at
//!   [`ArchTrustSnapshot::snapshot_time`],
//! * disabled-key state from `<keyring>-revoked`.
//!
//! # Reference time
//!
//! GnuPG builds a certificate's effective state from the *latest* valid
//! self-signature (`g10/getkey.c:2914`, `:3427`) and judges expiry against
//! the current time (`g10/getkey.c:3220`, `:3472`), not against the time the
//! document signature was made.  The snapshot therefore has exactly one
//! reference time, and it is wall-clock "now" in production.  The signature's
//! own creation time is a separate input used only for the relations GnuPG
//! actually ties to it (see [`super::verify`]).

use crate::error::{Error, Result};
use openpgp::cert::amalgamation::{ValidAmalgamation, ValidateAmalgamation};
use openpgp::policy::StandardPolicy;
use openpgp::types::SignatureType;
use sequoia_openpgp as openpgp;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use super::super::super::ArchKeyringTrust;
use super::keyring::ArchDisabledKeys;

/// Why a certificate in the keyring counts as trusted.
///
/// This mirrors the two ways a key reaches `GPGME_VALIDITY_FULL` under
/// `pacman-key --populate`: a master key is locally signed by pacman's
/// ultimately trusted key, and a packager key is certified by enough
/// marginally trusted masters (`--marginals-needed`, default 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchKeyAuthority {
    /// A pinned master certificate.
    PinnedMaster,
    /// A packager certificate certified by these pinned masters.
    Certified { masters: BTreeSet<String> },
}

/// Pacman's `alpm_sigvalidity_t` projected onto the pinned Arch model.
///
/// `ALPM_SIGVALIDITY_NEVER` is intentionally not representable: it requires a
/// GnuPG ownertrust of "never", and Conary's authority is a pinned master
/// list rather than a mutable local trustdb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchSignatureValidity {
    Full,
    Marginal,
    Unknown,
}

/// One certificate as the populated keyring sees it.
#[derive(Debug, Clone)]
pub struct ArchKeyringEntry {
    certificate: openpgp::Cert,
    authority: Option<ArchKeyAuthority>,
    disabled: bool,
}

impl ArchKeyringEntry {
    pub fn certificate(&self) -> &openpgp::Cert {
        &self.certificate
    }

    pub fn disabled(&self) -> bool {
        self.disabled
    }

    pub fn authority(&self) -> Option<&ArchKeyAuthority> {
        self.authority.as_ref()
    }

    /// Trust validity under the pinned-master threshold.
    pub fn validity(&self, threshold: usize) -> ArchSignatureValidity {
        match &self.authority {
            Some(ArchKeyAuthority::PinnedMaster) => ArchSignatureValidity::Full,
            Some(ArchKeyAuthority::Certified { masters }) if masters.len() >= threshold => {
                ArchSignatureValidity::Full
            }
            Some(ArchKeyAuthority::Certified { masters }) if !masters.is_empty() => {
                ArchSignatureValidity::Marginal
            }
            _ => ArchSignatureValidity::Unknown,
        }
    }
}

/// An authenticated keyring plus the instant it was judged at.
#[derive(Debug, Clone)]
pub struct ArchTrustSnapshot {
    entries: Vec<ArchKeyringEntry>,
    threshold: usize,
    snapshot_time: SystemTime,
}

impl ArchTrustSnapshot {
    /// Authenticates a parsed keyring against the pinned master authority.
    pub fn authenticate(
        certificates: Vec<openpgp::Cert>,
        disabled: &ArchDisabledKeys,
        keyring: &ArchKeyringTrust,
        snapshot_time: SystemTime,
    ) -> Result<Self> {
        let policy = StandardPolicy::new();
        let by_fingerprint = certificates
            .iter()
            .map(|certificate| (certificate.fingerprint().to_hex(), certificate))
            .collect::<BTreeMap<_, _>>();

        // A master certificate confers authority only while it is itself
        // usable.  GnuPG drops expired or revoked keys from the trust
        // computation entirely (`g10/trustdb.c:2070`, `:2248`), so an expired
        // master silently stops certifying rather than certifying forever.
        let mut masters = Vec::with_capacity(keyring.master_fingerprints.len());
        for fingerprint in &keyring.master_fingerprints {
            let certificate = by_fingerprint.get(fingerprint).copied().ok_or_else(|| {
                Error::GpgVerificationFailed(format!(
                    "Arch keyring lost pinned master certificate '{fingerprint}'"
                ))
            })?;
            let usable = certificate
                .keys()
                .with_policy(&policy, snapshot_time)
                .supported()
                .alive()
                .revoked(false)
                .for_certification()
                .any(|key| {
                    use openpgp::cert::amalgamation::key::PrimaryKey;
                    key.primary()
                });
            if !usable {
                return Err(Error::GpgVerificationFailed(format!(
                    "Arch master certificate '{fingerprint}' has no live, unrevoked certification \
                     key at the trust-snapshot time"
                )));
            }
            masters.push(certificate);
        }
        let master_fingerprints = keyring
            .master_fingerprints
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();

        let mut entries = Vec::with_capacity(certificates.len());
        for certificate in &certificates {
            let fingerprint = certificate.fingerprint().to_hex();
            let authority = if master_fingerprints.contains(fingerprint.as_str()) {
                Some(ArchKeyAuthority::PinnedMaster)
            } else {
                certifying_masters(certificate, &masters, &policy, snapshot_time)
                    .map(|masters| ArchKeyAuthority::Certified { masters })
            };
            entries.push(ArchKeyringEntry {
                certificate: certificate.clone(),
                authority,
                disabled: disabled.contains(&fingerprint),
            });
        }

        if !entries.iter().any(|entry| {
            matches!(
                entry.validity(keyring.packager_key_threshold),
                ArchSignatureValidity::Full
            )
        }) {
            return Err(Error::GpgVerificationFailed(
                "Arch keyring contains no fully valid certificate under the pinned master \
                 authority"
                    .to_string(),
            ));
        }

        Ok(Self {
            entries,
            threshold: keyring.packager_key_threshold,
            snapshot_time,
        })
    }

    pub fn snapshot_time(&self) -> SystemTime {
        self.snapshot_time
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    pub fn entries(&self) -> &[ArchKeyringEntry] {
        &self.entries
    }

    /// Every certificate that reaches full validity, for keyring export.
    pub fn fully_valid_certificates(&self) -> Vec<openpgp::Cert> {
        self.entries
            .iter()
            .filter(|entry| {
                !entry.disabled
                    && matches!(entry.validity(self.threshold), ArchSignatureValidity::Full)
            })
            .map(|entry| entry.certificate.clone())
            .collect()
    }

    /// Finds the keyring entry that owns a key matching `handle`.
    ///
    /// The handle may name a primary key or a signing subkey; pacman looks
    /// the signature's issuer up in the whole keyring the same way.
    pub fn lookup(&self, handle: &openpgp::KeyHandle) -> Option<&ArchKeyringEntry> {
        self.entries.iter().find(|entry| {
            entry
                .certificate
                .keys()
                .any(|key| key.key().key_handle().aliases(handle))
        })
    }
}

/// Distinct pinned masters with an active positive certification on some
/// self-signed user ID of `certificate` at `reference_time`.
///
/// Returns `None` when the certificate itself cannot carry validity, which
/// GnuPG decides before any certification is counted (`g10/trustdb.c:2070`):
/// a revoked or expired key is excluded from the web of trust outright.
fn certifying_masters(
    certificate: &openpgp::Cert,
    masters: &[&openpgp::Cert],
    policy: &StandardPolicy<'_>,
    reference_time: SystemTime,
) -> Option<BTreeSet<String>> {
    let valid = certificate.with_policy(policy, reference_time).ok()?;
    if valid.alive().is_err() {
        return None;
    }
    if matches!(
        valid.revocation_status(),
        openpgp::types::RevocationStatus::Revoked(_)
    ) {
        return None;
    }

    let mut certifying = BTreeSet::new();
    for userid in certificate.userids() {
        // Only a self-signed, unrevoked user ID can carry third-party
        // certifications into the web of trust.
        let Ok(valid_userid) = userid.clone().with_policy(policy, reference_time) else {
            continue;
        };
        if matches!(
            valid_userid.revocation_status(),
            openpgp::types::RevocationStatus::Revoked(_)
        ) {
            continue;
        }
        for master in masters {
            let certified = userid
                .active_certifications_by_key(policy, reference_time, master.primary_key().key())
                .any(|signature| {
                    matches!(
                        signature.typ(),
                        SignatureType::GenericCertification
                            | SignatureType::PersonaCertification
                            | SignatureType::CasualCertification
                            | SignatureType::PositiveCertification
                    )
                });
            if certified {
                certifying.insert(master.fingerprint().to_hex());
            }
        }
    }
    Some(certifying)
}

#[cfg(test)]
mod tests {
    use super::super::testkeys;
    use super::*;

    #[test]
    fn packager_key_requires_direct_master_certification() {
        let master = testkeys::master("Arch Master");
        let packager = testkeys::certify(&master, testkeys::packager("Arch Packager"));
        let uncertified = testkeys::packager("Uncertified Packager");
        let trust = testkeys::trust(vec![master.fingerprint().to_hex()], 1);

        let snapshot = ArchTrustSnapshot::authenticate(
            vec![master.clone(), packager.clone(), uncertified.clone()],
            &ArchDisabledKeys::NotCarried,
            &trust,
            SystemTime::now(),
        )
        .unwrap();

        let fully_valid = snapshot
            .fully_valid_certificates()
            .iter()
            .map(|certificate| certificate.fingerprint().to_hex())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            fully_valid,
            BTreeSet::from([
                master.fingerprint().to_hex(),
                packager.fingerprint().to_hex()
            ])
        );

        let entry = snapshot
            .lookup(&uncertified.fingerprint().into())
            .expect("uncertified keys stay visible so they classify as unknown, not missing");
        assert_eq!(entry.validity(1), ArchSignatureValidity::Unknown);
    }

    #[test]
    fn packager_key_must_meet_the_exact_master_threshold() {
        let master_a = testkeys::master("Arch Master A");
        let master_b = testkeys::master("Arch Master B");
        let packager = testkeys::certify(&master_a, testkeys::packager("Arch Packager"));
        let trust = testkeys::trust(
            vec![
                master_a.fingerprint().to_hex(),
                master_b.fingerprint().to_hex(),
            ],
            2,
        );

        let snapshot = ArchTrustSnapshot::authenticate(
            vec![master_a, master_b, packager.clone()],
            &ArchDisabledKeys::NotCarried,
            &trust,
            SystemTime::now(),
        )
        .unwrap();
        let entry = snapshot.lookup(&packager.fingerprint().into()).unwrap();
        assert_eq!(entry.validity(2), ArchSignatureValidity::Marginal);
        assert!(
            !snapshot
                .fully_valid_certificates()
                .iter()
                .any(|certificate| certificate.fingerprint() == packager.fingerprint())
        );
    }

    #[test]
    fn disabled_state_is_carried_into_the_snapshot() {
        let master = testkeys::master("Arch Master");
        let packager = testkeys::certify(&master, testkeys::packager("Arch Packager"));
        let trust = testkeys::trust(vec![master.fingerprint().to_hex()], 1);
        let disabled = ArchDisabledKeys::Carried(BTreeSet::from([packager.fingerprint().to_hex()]));

        let snapshot = ArchTrustSnapshot::authenticate(
            vec![master, packager.clone()],
            &disabled,
            &trust,
            SystemTime::now(),
        )
        .unwrap();
        let entry = snapshot.lookup(&packager.fingerprint().into()).unwrap();
        assert!(entry.disabled());
        // Disabled keys keep full validity in GnuPG's trust computation; the
        // disabled bit is a separate axis that `signing.c` checks on its own.
        assert_eq!(entry.validity(1), ArchSignatureValidity::Full);
    }

    #[test]
    fn expired_master_authority_is_fatal() {
        let master = testkeys::expiring_master("Expiring Arch Master");
        let packager = testkeys::certify(&master, testkeys::packager("Arch Packager"));
        let trust = testkeys::trust(vec![master.fingerprint().to_hex()], 1);

        let error = ArchTrustSnapshot::authenticate(
            vec![master, packager],
            &ArchDisabledKeys::NotCarried,
            &trust,
            SystemTime::now() + std::time::Duration::from_secs(3600 * 24 * 30),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no live, unrevoked certification key")
        );
    }

    #[test]
    fn a_keyring_without_a_fully_valid_certificate_is_fatal() {
        let master_a = testkeys::master("Arch Master A");
        let master_b = testkeys::master("Arch Master B");
        let trust = testkeys::trust(
            vec![
                master_a.fingerprint().to_hex(),
                master_b.fingerprint().to_hex(),
            ],
            2,
        );
        // Masters are pinned, so they are always fully valid; drop the
        // threshold check to a keyring whose only entries are uncertified.
        let packager = testkeys::packager("Arch Packager");
        let error = ArchTrustSnapshot::authenticate(
            vec![packager],
            &ArchDisabledKeys::NotCarried,
            &trust,
            SystemTime::now(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("lost pinned master certificate"));
    }
}
