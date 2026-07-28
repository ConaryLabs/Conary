// conary-core/src/packages/arch/payload/xattrs.rs

//! PAX extended-attribute projection from libarchive's exact writer grammar.
//!
//! libarchive's PAX writer defaults to emitting both `LIBARCHIVE.xattr.<name>`
//! (base64-encoded, never padded) and `SCHILY.xattr.<name>` (raw) for each
//! xattr, with both derived from the same source value.  `RHT.security.selinux`
//! is a third family spelling of `security.selinux`.
//!
//! This module collects per-family records during PAX extension iteration and
//! projects one xattr per name when values agree; disagreement or same-family
//! duplicates fail closed.  Decoding uses the writer's exact grammar: standard
//! alphabet, no padding, no trailing `=`, non-zero trailing bits rejected.

use crate::error::{Error, Result};
use base64::Engine;
use base64::alphabet;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Base64 decoder matching libarchive's exact writer grammar.
//
// libarchive commit f5509ae993ac30417f81acc5118f232ae3f2d27d,
// archive_write_set_format_pax.c base64_encode (L2122):
//   - standard alphabet (A-Z a-z 0-9 + /)
//   - final group of 1 byte → 2 chars
//   - final group of 2 bytes → 3 chars
//   - never emits '=' padding
//
// We use the base64 crate's GeneralPurpose with the standard alphabet,
// RequireNone decode padding (rejects any '=' in input), and
// decode_allow_trailing_bits(false) (the default for new()).
// ---------------------------------------------------------------------------

/// Base64 engine that matches libarchive's writer: standard alphabet, no
/// padding, no trailing '=', non-zero trailing bits rejected.
const UNPADDED_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

/// Decode a `LIBARCHIVE.xattr.<name>` value using libarchive's exact writer
/// grammar.  Padded input (trailing `=`) is malformed because the writer
/// cannot produce it — treat padded input as evidence of tampering and fail
/// closed.
pub(super) fn decode_libarchive_base64(value: &[u8], path: &str) -> Result<Vec<u8>> {
    UNPADDED_BASE64.decode(value).map_err(|error| {
        Error::ParseError(format!(
            "invalid LIBARCHIVE xattr value for {path}: {error}"
        ))
    })
}

// ---------------------------------------------------------------------------
// Per-family xattr tracking and projection
// ---------------------------------------------------------------------------

/// The three PAX families that can carry xattr values for the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PaxXattrFamily {
    /// `SCHILY.xattr.<name>` — raw bytes.
    Schily,
    /// `LIBARCHIVE.xattr.<name>` — base64-decoded bytes.
    Libarchive,
    /// `RHT.security.selinux` — raw bytes; spells `security.selinux`.
    RhtSelinux,
}

/// Accumulates xattr records keyed by (name, family) during PAX iteration.
/// After all records are collected, [`project`](Self::project) validates
/// cross-family agreement and produces the final `BTreeMap<String, Vec<u8>>`.
#[derive(Default)]
pub(super) struct PaxXattrRecords {
    records: BTreeMap<String, BTreeMap<PaxXattrFamily, Vec<u8>>>,
}

impl PaxXattrRecords {
    /// Insert a value for the given name and family.  Same-family duplicates
    /// are rejected fail-closed.
    pub(super) fn insert(
        &mut self,
        name: String,
        family: PaxXattrFamily,
        value: Vec<u8>,
        path: &str,
    ) -> Result<()> {
        let families = self.records.entry(name.clone()).or_default();
        if families.contains_key(&family) {
            return Err(Error::ParseError(format!(
                "duplicate {:?} xattr {name} for ALPM payload node {path}",
                family,
            )));
        }
        families.insert(family, value);
        Ok(())
    }

    /// Project all accumulated records into a `BTreeMap<String, Vec<u8>>`.
    ///
    /// For each name:
    /// - One family: use its value directly.
    /// - Multiple families: all values must agree byte-for-byte (a well-formed
    ///   writer produces identical values from the same source).  Disagreement
    ///   fails closed.
    pub(super) fn project(self, path: &str) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut xattrs = BTreeMap::new();
        for (name, families) in self.records {
            let mut iter = families.into_values();
            // SAFETY: at least one family exists (inserted above).
            let first = iter.next().expect("at least one family");
            for other in iter {
                if first != other {
                    return Err(Error::ParseError(format!(
                        "xattr {name} for {path} has conflicting values across PAX families"
                    )));
                }
            }
            xattrs.insert(name, first);
        }
        Ok(xattrs)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Base64 decode -------------------------------------------------------

    #[test]
    fn decode_unpadded_value_non_multiple_of_3() {
        // 2 bytes → 3 chars (no padding). "AA==" in padded form.
        let decoded = decode_libarchive_base64(b"AAA", "test").unwrap();
        assert_eq!(decoded, vec![0, 0]);
    }

    #[test]
    fn decode_unpadded_value_multiple_of_3() {
        // 3 bytes → 4 chars (no padding needed).
        let decoded = decode_libarchive_base64(b"AAAA", "test").unwrap();
        assert_eq!(decoded, vec![0, 0, 0]);
    }

    #[test]
    fn decode_20_byte_value_27_chars_unpadded() {
        // 20 bytes → 27 unpadded chars (20 * 8 = 160 bits; ceil(160 / 6) = 27).
        // 27 A's = 27 × 0 = zero bits → 20 zero bytes, no trailing bits set.
        let encoded = "AAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert_eq!(encoded.len(), 27);
        let decoded = decode_libarchive_base64(encoded.as_bytes(), "test").unwrap();
        assert_eq!(decoded.len(), 20);
        assert!(decoded.iter().all(|&b| b == 0));
    }

    #[test]
    fn rejects_padded_value() {
        let err = decode_libarchive_base64(b"AQIDBA==", "test").expect_err("padded must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("invalid LIBARCHIVE xattr value"),
            "expected padding rejection: {msg}"
        );
    }

    #[test]
    fn rejects_foreign_byte_in_alphabet() {
        // '!' is not in the standard base64 alphabet.
        let err = decode_libarchive_base64(b"AAA!", "test").expect_err("foreign byte must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("invalid LIBARCHIVE xattr value"),
            "expected foreign-byte rejection: {msg}"
        );
    }

    #[test]
    fn rejects_one_char_final_group() {
        // 1-char final group: the writer emits 2 chars for 1 byte, 3 chars
        // for 2 bytes.  A single char cannot represent a complete final group.
        // "A" alone is 1 char → 6 bits, not a complete byte.
        let err = decode_libarchive_base64(b"A", "test").expect_err("1-char group must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("invalid LIBARCHIVE xattr value"),
            "expected 1-char rejection: {msg}"
        );
    }

    #[test]
    fn rejects_trailing_equals_sign() {
        // Trailing '=' anywhere in the input is malformed per RequireNone.
        let err =
            decode_libarchive_base64(b"AAAA=", "test").expect_err("trailing equals must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("invalid LIBARCHIVE xattr value"),
            "expected trailing-equals rejection: {msg}"
        );
    }

    // -- PaxXattrRecords projection ------------------------------------------

    #[test]
    fn dual_family_agreement_projects_one_xattr() {
        let mut records = PaxXattrRecords::default();
        records
            .insert(
                "security.capability".into(),
                PaxXattrFamily::Libarchive,
                vec![1, 2, 3, 4],
                "test",
            )
            .unwrap();
        records
            .insert(
                "security.capability".into(),
                PaxXattrFamily::Schily,
                vec![1, 2, 3, 4],
                "test",
            )
            .unwrap();
        let xattrs = records.project("test").unwrap();
        assert_eq!(xattrs.len(), 1);
        assert_eq!(xattrs.get("security.capability"), Some(&vec![1, 2, 3, 4]));
    }

    #[test]
    fn dual_family_disagreement_rejects() {
        let mut records = PaxXattrRecords::default();
        records
            .insert(
                "security.capability".into(),
                PaxXattrFamily::Libarchive,
                vec![1, 2, 3, 4],
                "test",
            )
            .unwrap();
        records
            .insert(
                "security.capability".into(),
                PaxXattrFamily::Schily,
                vec![5, 6, 7, 8],
                "test",
            )
            .unwrap();
        let err = records
            .project("test")
            .expect_err("disagreement must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("conflicting values across PAX families"),
            "expected disagreement message: {msg}"
        );
    }

    #[test]
    fn same_family_duplicate_rejects() {
        let mut records = PaxXattrRecords::default();
        records
            .insert("user.test".into(), PaxXattrFamily::Schily, vec![1], "test")
            .unwrap();
        let err = records
            .insert("user.test".into(), PaxXattrFamily::Schily, vec![2], "test")
            .expect_err("same-family duplicate must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate Schily xattr user.test"),
            "expected duplicate message: {msg}"
        );
    }

    #[test]
    fn schily_only_projects_directly() {
        let mut records = PaxXattrRecords::default();
        records
            .insert(
                "user.example".into(),
                PaxXattrFamily::Schily,
                b"hello".to_vec(),
                "test",
            )
            .unwrap();
        let xattrs = records.project("test").unwrap();
        assert_eq!(xattrs.len(), 1);
        assert_eq!(xattrs.get("user.example"), Some(&b"hello".to_vec()));
    }

    #[test]
    fn libarchive_only_projects_directly() {
        let mut records = PaxXattrRecords::default();
        records
            .insert(
                "user.lib".into(),
                PaxXattrFamily::Libarchive,
                vec![9, 8, 7],
                "test",
            )
            .unwrap();
        let xattrs = records.project("test").unwrap();
        assert_eq!(xattrs.len(), 1);
        assert_eq!(xattrs.get("user.lib"), Some(&vec![9, 8, 7]));
    }

    #[test]
    fn three_family_agreement_projects_one() {
        let mut records = PaxXattrRecords::default();
        records
            .insert(
                "security.selinux".into(),
                PaxXattrFamily::Schily,
                b"selinux_data".to_vec(),
                "test",
            )
            .unwrap();
        records
            .insert(
                "security.selinux".into(),
                PaxXattrFamily::Libarchive,
                b"selinux_data".to_vec(),
                "test",
            )
            .unwrap();
        records
            .insert(
                "security.selinux".into(),
                PaxXattrFamily::RhtSelinux,
                b"selinux_data".to_vec(),
                "test",
            )
            .unwrap();
        let xattrs = records.project("test").unwrap();
        assert_eq!(xattrs.len(), 1);
        assert_eq!(
            xattrs.get("security.selinux"),
            Some(&b"selinux_data".to_vec())
        );
    }

    #[test]
    fn three_family_partial_disagreement_rejects() {
        let mut records = PaxXattrRecords::default();
        records
            .insert(
                "security.selinux".into(),
                PaxXattrFamily::Schily,
                b"foo".to_vec(),
                "test",
            )
            .unwrap();
        records
            .insert(
                "security.selinux".into(),
                PaxXattrFamily::Libarchive,
                b"foo".to_vec(),
                "test",
            )
            .unwrap();
        records
            .insert(
                "security.selinux".into(),
                PaxXattrFamily::RhtSelinux,
                b"bar".to_vec(),
                "test",
            )
            .unwrap();
        let err = records
            .project("test")
            .expect_err("disagreement must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("conflicting values across PAX families"),
            "expected disagreement message: {msg}"
        );
    }
}
