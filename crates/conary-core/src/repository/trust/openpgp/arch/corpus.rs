// conary-core/src/repository/trust/openpgp/arch/corpus.rs

//! Real-package conformance corpus.
//!
//! Native pacman/GPGME is the conformance oracle for ALPM signature trust,
//! but it must never be runtime authority, so nothing here runs `pacman` or
//! `gpg`.  Each entry instead records the result classes GnuPG produced under
//! a keyring populated exactly like `pacman-key --populate archlinux`, and the
//! tests assert the typed Rust verifier reaches the same classes from the
//! same bytes.
//!
//! # Regenerating
//!
//! `tests/fixtures/arch/archlinux-keyring-corpus.pkg.tar.zst` is a bounded
//! `archlinux-keyring` ALPM package holding the five pinned master
//! certificates (exported with `export-minimal`, since masters are pinned by
//! fingerprint and carry no third-party authority) plus the corpus packager
//! certificates exported in full, together with the untouched
//! `archlinux-revoked` and `archlinux-trusted` members.  Oracle classes are
//! captured by replaying `pacman-key --populate`:
//!
//! ```text
//! gpg --quick-gen-key 'Pacman Keyring Master Key <pacman@localhost>'
//! gpg --import usr/share/pacman/keyrings/archlinux.gpg
//! # lsign every archlinux-trusted fingerprint with that local key
//! gpg --import-ownertrust usr/share/pacman/keyrings/archlinux-trusted
//! # `disable` every fingerprint in usr/share/pacman/keyrings/archlinux-revoked
//! gpg --status-fd 1 --verify <pkg>.sig <pkg>
//! ```
//!
//! `GOODSIG`/`VALIDSIG` is `GPG_ERR_NO_ERROR`, which
//! `lib/libalpm/signing.c:678` maps to `ALPM_SIGSTATUS_VALID`; `TRUST_FULLY`
//! is `GPGME_VALIDITY_FULL`, which `:703` maps to `ALPM_SIGVALIDITY_FULL`.
//! Both together are what `_alpm_check_pgp_helper` accepts under
//! `TrustedOnly`.  Every entry below was captured as `GOODSIG` +
//! `TRUST_FULLY`.
//!
//! # Recorded differences from the oracle
//!
//! These are deliberate and all in the fail-closed direction:
//!
//! * Conary rejects an `archlinux-keyring` ALPM package that omits the
//!   `archlinux-revoked` member.  `pacman-key` tolerates a missing revoked
//!   file (`[[ -s ... ]]`); Conary refuses a keyring that cannot express the
//!   disabled-key state it is about to be checked against.
//! * Conary evaluates certificates under Sequoia's `StandardPolicy`, which
//!   rejects SHA-1 protected self-signatures and certifications.  GnuPG 2.4
//!   treats only MD5 as weak by default (`g10/gpg.c:2604`).
//! * Conary accepts only v4 binary document signatures.  GnuPG additionally
//!   accepts v3, and `SignatureType::Text`; ALPM produces neither.
//! * Conary derives trust validity from a pinned master list and an explicit
//!   certification threshold rather than a mutable local trustdb, so
//!   `ALPM_SIGVALIDITY_NEVER` (GnuPG ownertrust "never") is unrepresentable.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::super::super::{ArchKeyringFormat, ArchKeyringTrust};
use super::keyring::{extract_keyring_source, parse_certificates};
use super::snapshot::{ArchSignatureValidity, ArchTrustSnapshot};
use super::verify::{ArchInvalidReason, ArchSignatureStatus, check_detached, verify_detached};

const KEYRING: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/arch/archlinux-keyring-corpus.pkg.tar.zst"
));

macro_rules! corpus_entry {
    ($name:literal, $signing_key:literal, $certificate:literal, $class:expr) => {
        CorpusEntry {
            package: $name,
            bytes: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/arch/packages/",
                $name
            )),
            signature: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/arch/packages/",
                $name,
                ".sig"
            )),
            oracle_signing_key: $signing_key,
            oracle_certificate: $certificate,
            class: $class,
        }
    };
}

/// Whether the entry reproduces issue #102's binding-at-signature-time class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CorpusClass {
    /// The signing key has no binding at the signature's own creation time.
    /// Sequoia's streaming verifier rejects these; pacman accepts them.
    UnboundAtSignatureTime,
    /// The signing key was already bound when it signed.  These verified
    /// before this change and must keep verifying.
    BoundAtSignatureTime,
}

/// One real package, its detached signature, and the signer pacman reported.
struct CorpusEntry {
    package: &'static str,
    bytes: &'static [u8],
    signature: &'static [u8],
    /// `VALIDSIG` field 1: the key that actually made the signature.
    oracle_signing_key: &'static str,
    /// `VALIDSIG` last field: the primary key of the signer's certificate.
    oracle_certificate: &'static str,
    class: CorpusClass,
}

fn corpus() -> Vec<CorpusEntry> {
    use CorpusClass::{BoundAtSignatureTime, UnboundAtSignatureTime};
    vec![
        // Issue #102's pinned fixture.  Robin Candau signed it with his
        // primary key on 2025-09-18; the certificate's only self-signature in
        // the current keyring is dated 2025-11-04.
        corpus_entry!(
            "alpine-keyring-2.6-1-any.pkg.tar.zst",
            "262A58EC6C51F7EA395B2E2DFDC3040B92ACA748",
            "262A58EC6C51F7EA395B2E2DFDC3040B92ACA748",
            UnboundAtSignatureTime
        ),
        // Second packager key.  Frederik Schwan signed on 2025-05-24; every
        // self-signature on the exported certificate is from 2026-05-27.
        corpus_entry!(
            "dnssec-anchors-20250524-1-any.pkg.tar.zst",
            "05C7775A9E8B977407FE08E69D4C5AA15426DA0A",
            "05C7775A9E8B977407FE08E69D4C5AA15426DA0A",
            UnboundAtSignatureTime
        ),
        // Third packager key.  Fabio Castelli signed on 2024-08-27; the
        // exported self-signatures start on 2025-10-11.
        corpus_entry!(
            "perl-json-maybexs-1.004008-1-any.pkg.tar.zst",
            "42DFAFB7C03B2E4E7BBDBA69930B82BFC2BDA011",
            "42DFAFB7C03B2E4E7BBDBA69930B82BFC2BDA011",
            UnboundAtSignatureTime
        ),
        // Control: a signing *subkey* rather than a primary key, so the
        // subkey-binding path is exercised on real material too.
        corpus_entry!(
            "systemd-resolvconf-261.2-1-x86_64.pkg.tar.zst",
            "0429897DE5F3BDAC537A30696D42BDD116E0068F",
            "02FD1C7A934E614545849F19A6234074498E9CEE",
            BoundAtSignatureTime
        ),
        // Control: a primary-key signer whose self-signatures already predate
        // the package signature.  This class passed before #102.
        corpus_entry!(
            "base-3-3-any.pkg.tar.zst",
            "83BC8889351B5DEBBB68416EB8AC08600F108CDF",
            "83BC8889351B5DEBBB68416EB8AC08600F108CDF",
            BoundAtSignatureTime
        ),
    ]
}

/// The five `archlinux-trusted` master fingerprints, pinned in configuration
/// rather than read from the keyring's own `-trusted` member.
fn corpus_trust() -> ArchKeyringTrust {
    ArchKeyringTrust {
        url: "file:///fixtures/arch/archlinux-keyring-corpus.pkg.tar.zst".to_string(),
        format: ArchKeyringFormat::AlpmPackageZstd,
        master_fingerprints: vec![
            "2AC0A42EFB0B5CBC7A0402ED4DC95B6D7BE9892E".to_string(),
            "3572FA2A1B067F22C58AF155F8B821B42A6FDCD7".to_string(),
            "69E6471E3AE065297529832E6BA0F5A2037F4F41".to_string(),
            "99B6618472A3B3B814185BAED7D3D823B88BDB9B".to_string(),
            "D8AFDDA07A5B6EDFA7D8CCDAD6D055F927843F1C".to_string(),
        ],
        // GnuPG's default `--marginals-needed`, which is the threshold the
        // marginally trusted Arch master keys are evaluated under.
        packager_key_threshold: 3,
    }
}

/// The instant the oracle classes above were captured, 2026-07-27T00:00:00Z.
///
/// The corpus pins its trust-snapshot time so the recorded oracle result
/// stays reproducible as the fixture ages.  Production always uses wall-clock
/// now; see `super::load_snapshot`.
const CAPTURED_AT: Duration = Duration::from_secs(1_785_110_400);

fn snapshot_at(time: SystemTime) -> Result<ArchTrustSnapshot, crate::error::Error> {
    let trust = corpus_trust();
    let source = extract_keyring_source(KEYRING, trust.format).unwrap();
    ArchTrustSnapshot::authenticate(
        parse_certificates(&source.certificates, &trust).unwrap(),
        &source.disabled,
        &trust,
        time,
    )
}

fn captured_snapshot() -> ArchTrustSnapshot {
    snapshot_at(UNIX_EPOCH + CAPTURED_AT).unwrap()
}

#[test]
fn the_corpus_matches_pacmans_result_classes() {
    let snapshot = captured_snapshot();
    for entry in corpus() {
        let results = verify_detached(&snapshot, entry.bytes, entry.signature)
            .unwrap_or_else(|error| panic!("{}: {error}", entry.package));
        assert_eq!(results.len(), 1, "{}", entry.package);
        let result = &results[0];
        assert_eq!(
            result.status,
            ArchSignatureStatus::Valid,
            "{} status (oracle: GOODSIG)",
            entry.package
        );
        assert_eq!(
            result.validity,
            ArchSignatureValidity::Full,
            "{} validity (oracle: TRUST_FULLY)",
            entry.package
        );
        assert_eq!(
            result.signing_key.as_deref(),
            Some(entry.oracle_signing_key),
            "{} signing key (oracle: VALIDSIG field 1)",
            entry.package
        );
        assert_eq!(
            result.certificate.as_deref(),
            Some(entry.oracle_certificate),
            "{} certificate (oracle: VALIDSIG primary fingerprint)",
            entry.package
        );
        assert!(
            result.accepted_under_trusted_only(),
            "{} must be accepted under TrustedOnly",
            entry.package
        );
        check_detached(&snapshot, entry.bytes, entry.signature)
            .unwrap_or_else(|error| panic!("{}: {error}", entry.package));
    }
}

#[test]
fn the_recorded_corpus_classes_still_describe_the_fixtures() {
    // The `class` column is a claim about the fixture bytes, so it is checked
    // rather than trusted: `UnboundAtSignatureTime` entries must really have
    // no binding at their own signature time, which is exactly what Sequoia's
    // streaming verifier requires and pacman does not.
    let policy = sequoia_openpgp::policy::StandardPolicy::new();
    let snapshot = captured_snapshot();
    for entry in corpus() {
        let results = verify_detached(&snapshot, entry.bytes, entry.signature).unwrap();
        let signature_created = results[0].signature_created;
        let certificate = snapshot
            .entries()
            .iter()
            .find(|candidate| {
                candidate.certificate().fingerprint().to_hex() == entry.oracle_certificate
            })
            .unwrap()
            .certificate();
        let bound_at_signature_time = certificate
            .with_policy(&policy, signature_created)
            .map(|valid| {
                valid
                    .keys()
                    .any(|key| key.key().fingerprint().to_hex() == entry.oracle_signing_key)
            })
            .unwrap_or(false);
        let observed = if bound_at_signature_time {
            CorpusClass::BoundAtSignatureTime
        } else {
            CorpusClass::UnboundAtSignatureTime
        };
        assert_eq!(observed, entry.class, "{} recorded class", entry.package);
        assert!(
            signature_created < results[0].trust_snapshot,
            "{} signature must predate the trust snapshot",
            entry.package
        );
    }
}

#[test]
fn the_corpus_covers_the_reported_production_class_broadly() {
    let entries = corpus();
    let regressed = entries
        .iter()
        .filter(|entry| entry.class == CorpusClass::UnboundAtSignatureTime)
        .map(|entry| entry.oracle_certificate)
        .collect::<BTreeSet<_>>();
    assert!(
        regressed.len() >= 3,
        "issue #102 needs the pinned fixture plus at least two more packager keys, got {regressed:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.package.starts_with("alpine-keyring-2.6-1")),
        "the corpus must keep issue #102's pinned fixture"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.oracle_signing_key != entry.oracle_certificate),
        "the corpus must exercise a signing subkey, not only primary-key signers"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.class == CorpusClass::BoundAtSignatureTime),
        "the corpus must keep a control that already verified"
    );
}

#[test]
fn tampering_with_any_corpus_package_fails_closed() {
    let snapshot = captured_snapshot();
    for entry in corpus() {
        let mut tampered = entry.bytes.to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        let results = verify_detached(&snapshot, &tampered, entry.signature).unwrap();
        assert_eq!(
            results[0].status,
            ArchSignatureStatus::Invalid(ArchInvalidReason::BadSignature),
            "{}",
            entry.package
        );
        assert!(check_detached(&snapshot, &tampered, entry.signature).is_err());

        let mut tampered_signature = entry.signature.to_vec();
        let last = tampered_signature.len() - 1;
        tampered_signature[last] ^= 0xff;
        assert!(
            check_detached(&snapshot, entry.bytes, &tampered_signature).is_err(),
            "{}",
            entry.package
        );
    }
}

#[test]
fn a_corpus_signature_is_never_accepted_before_its_signing_key_existed() {
    // Rolling the trust snapshot back before the packager keys were created
    // must not accept anything.  Authenticating may fail outright, because
    // the pinned masters are not live that far back; if it succeeds, no
    // corpus signature may be accepted.
    let Ok(snapshot) = snapshot_at(UNIX_EPOCH + Duration::from_secs(1_300_000_000)) else {
        return;
    };
    for entry in corpus() {
        if let Ok(results) = verify_detached(&snapshot, entry.bytes, entry.signature) {
            assert!(
                !results[0].accepted_under_trusted_only(),
                "{} accepted against a pre-dated trust snapshot",
                entry.package
            );
        }
        assert!(check_detached(&snapshot, entry.bytes, entry.signature).is_err());
    }
}

#[test]
fn the_corpus_keyring_carries_the_disabled_key_state() {
    let trust = corpus_trust();
    let source = extract_keyring_source(KEYRING, trust.format).unwrap();
    // A fingerprint from the same release's `archlinux-revoked`.
    assert!(
        source
            .disabled
            .contains("0A9DDABB64B993D82AD45E4F32EAB0A976938292")
    );
    for entry in corpus() {
        assert!(
            !source.disabled.contains(entry.oracle_certificate),
            "{} signer must not be disabled",
            entry.package
        );
    }
}

#[test]
fn every_corpus_packager_reaches_the_pinned_master_threshold() {
    let snapshot = captured_snapshot();
    let trust = corpus_trust();
    for entry in corpus() {
        let keyring_entry = snapshot
            .entries()
            .iter()
            .find(|candidate| {
                candidate.certificate().fingerprint().to_hex() == entry.oracle_certificate
            })
            .unwrap();
        match keyring_entry.authority() {
            Some(super::snapshot::ArchKeyAuthority::Certified { masters }) => assert!(
                masters.len() >= trust.packager_key_threshold
                    && masters.is_subset(
                        &trust
                            .master_fingerprints
                            .iter()
                            .cloned()
                            .collect::<BTreeSet<_>>()
                    ),
                "{} must be certified only by pinned masters, got {masters:?}",
                entry.package
            ),
            other => panic!("{} unexpected authority {other:?}", entry.package),
        }
    }
}
