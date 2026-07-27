// conary-core/src/repository/trust/openpgp/arch/keyring.rs

//! Exact grammar of a pacman keyring source.
//!
//! `pacman-key --populate <keyring>` (pacman `a6f7467d`,
//! `scripts/pacman-key.sh.in`) consumes two files from
//! `/usr/share/pacman/keyrings`:
//!
//! * `<keyring>.gpg` -- the OpenPGP certificate material, imported wholesale.
//! * `<keyring>-revoked` -- one fingerprint per line; each is `disable`d in
//!   the resulting keyring, which `lib/libalpm/signing.c` later reads back as
//!   `key->disabled` and maps to `ALPM_SIGSTATUS_KEY_DISABLED`.
//!
//! This module owns the transport and the byte grammar only.  It performs no
//! certification, binding, or signature evaluation; that is
//! [`super::snapshot`] and [`super::verify`].

use crate::error::{Error, Result};
use openpgp::cert::CertParser;
use openpgp::parse::Parse;
use sequoia_openpgp as openpgp;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::Path;

use super::super::super::{ArchKeyringFormat, ArchKeyringTrust};

const CERTIFICATE_MEMBER: &str = "usr/share/pacman/keyrings/archlinux.gpg";
const REVOKED_MEMBER: &str = "usr/share/pacman/keyrings/archlinux-revoked";

const MAX_KEYRING_PACKAGE_OUTPUT: u64 = 64 * 1024 * 1024;
const MAX_KEYRING_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REVOKED_BYTES: u64 = 1024 * 1024;
const MAX_KEYRING_PACKAGE_ENTRIES: usize = 4096;

/// Fingerprints pacman disables in the populated keyring.
///
/// `NotCarried` is not "no revocations by default": it records that the
/// configured transport has no `<keyring>-revoked` channel at all, which is
/// the same input `pacman-key` sees when its `[[ -s ... ]]` guard fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchDisabledKeys {
    Carried(BTreeSet<String>),
    NotCarried,
}

impl ArchDisabledKeys {
    pub fn contains(&self, fingerprint: &str) -> bool {
        match self {
            Self::Carried(fingerprints) => fingerprints.contains(fingerprint),
            Self::NotCarried => false,
        }
    }

    pub(super) fn serialize(&self) -> Vec<u8> {
        match self {
            Self::NotCarried => Vec::new(),
            Self::Carried(fingerprints) => {
                let mut out = String::new();
                for fingerprint in fingerprints {
                    out.push_str(fingerprint);
                    out.push('\n');
                }
                out.into_bytes()
            }
        }
    }
}

/// One authenticated-in-transport keyring snapshot source.
#[derive(Debug, Clone)]
pub(super) struct ArchKeyringSource {
    pub certificates: Vec<u8>,
    pub disabled: ArchDisabledKeys,
}

/// Extracts the certificate material and revoked list from a keyring source.
pub(super) fn extract_keyring_source(
    source: &[u8],
    format: ArchKeyringFormat,
) -> Result<ArchKeyringSource> {
    match format {
        // A bare keyring export has no companion revoked list to carry.
        ArchKeyringFormat::OpenPgp => Ok(ArchKeyringSource {
            certificates: source.to_vec(),
            disabled: ArchDisabledKeys::NotCarried,
        }),
        ArchKeyringFormat::AlpmPackageZstd => {
            let members = extract_alpm_members(source)?;
            let certificates = members.certificates.ok_or_else(|| {
                Error::ParseError(format!(
                    "Arch keyring ALPM package has no exact '{CERTIFICATE_MEMBER}' member"
                ))
            })?;
            // archlinux-keyring always ships the revoked list next to the
            // certificate material.  A package that omits it cannot express
            // pacman's disabled-key state, so it is rejected rather than
            // silently treated as "nothing is disabled".
            let revoked = members.revoked.ok_or_else(|| {
                Error::ParseError(format!(
                    "Arch keyring ALPM package has no exact '{REVOKED_MEMBER}' member"
                ))
            })?;
            Ok(ArchKeyringSource {
                certificates,
                disabled: ArchDisabledKeys::Carried(parse_revoked_list(&revoked)?),
            })
        }
    }
}

/// The two authority members of an `archlinux-keyring` ALPM package, each
/// present at most once.
#[derive(Default)]
struct AlpmKeyringMembers {
    certificates: Option<Vec<u8>>,
    revoked: Option<Vec<u8>>,
}

fn extract_alpm_members(source: &[u8]) -> Result<AlpmKeyringMembers> {
    let decoder = crate::compression::create_decoder_limited(
        Cursor::new(source),
        crate::compression::CompressionFormat::Zstd,
        MAX_KEYRING_PACKAGE_OUTPUT,
    )
    .map_err(|error| {
        Error::ParseError(format!(
            "failed to decode Arch keyring ALPM package: {error}"
        ))
    })?;
    let mut archive = tar::Archive::new(decoder);
    let mut members = AlpmKeyringMembers::default();
    for (index, entry) in archive
        .entries()
        .map_err(|error| {
            Error::ParseError(format!("failed to read Arch keyring ALPM package: {error}"))
        })?
        .enumerate()
    {
        if index >= MAX_KEYRING_PACKAGE_ENTRIES {
            return Err(Error::ParseError(format!(
                "Arch keyring ALPM package exceeds {MAX_KEYRING_PACKAGE_ENTRIES} entries"
            )));
        }
        let entry = entry.map_err(|error| {
            Error::ParseError(format!("failed to read Arch keyring ALPM member: {error}"))
        })?;
        let path = entry.path().map_err(|error| {
            Error::ParseError(format!("invalid Arch keyring ALPM member path: {error}"))
        })?;
        let (slot, member, limit) = if path.as_ref() == Path::new(CERTIFICATE_MEMBER) {
            (
                &mut members.certificates,
                CERTIFICATE_MEMBER,
                MAX_KEYRING_BYTES,
            )
        } else if path.as_ref() == Path::new(REVOKED_MEMBER) {
            (&mut members.revoked, REVOKED_MEMBER, MAX_REVOKED_BYTES)
        } else {
            continue;
        };
        if slot.is_some() {
            return Err(Error::ParseError(format!(
                "Arch keyring ALPM package repeats exact member '{member}'"
            )));
        }
        *slot = Some(read_member(entry, member, limit)?);
    }
    Ok(members)
}

fn read_member<R: Read>(entry: tar::Entry<'_, R>, member: &str, limit: u64) -> Result<Vec<u8>> {
    if !entry.header().entry_type().is_file() {
        return Err(Error::ParseError(format!(
            "Arch keyring ALPM member '{member}' is not a regular file"
        )));
    }
    let mut bytes = Vec::new();
    entry
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            Error::ParseError(format!(
                "failed to read Arch keyring ALPM member '{member}': {error}"
            ))
        })?;
    if bytes.len() as u64 > limit {
        return Err(Error::ParseError(format!(
            "Arch keyring ALPM member '{member}' exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

/// Parses `<keyring>-revoked`: one uppercase 40-hex fingerprint per line.
pub(super) fn parse_revoked_list(bytes: &[u8]) -> Result<BTreeSet<String>> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        Error::ParseError(format!(
            "Arch keyring revoked list is not valid UTF-8: {error}"
        ))
    })?;
    let mut fingerprints = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() != 40 || !line.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::ParseError(format!(
                "Arch keyring revoked list line {} is not a 40-digit OpenPGP fingerprint",
                index + 1
            )));
        }
        let fingerprint = line.to_ascii_uppercase();
        if !fingerprints.insert(fingerprint) {
            return Err(Error::ParseError(format!(
                "Arch keyring revoked list repeats fingerprint on line {}",
                index + 1
            )));
        }
    }
    Ok(fingerprints)
}

/// Parses the keyring's certificate material, merging split certificates.
///
/// `gpg --import` merges packets belonging to one primary key; a keyring that
/// lists a certificate's components in separate blocks must therefore produce
/// one certificate here too, not several partial ones.
pub(super) fn parse_certificates(
    bytes: &[u8],
    keyring: &ArchKeyringTrust,
) -> Result<Vec<openpgp::Cert>> {
    let mut certificates = BTreeMap::<String, openpgp::Cert>::new();
    for parsed in CertParser::from_bytes(bytes)
        .map_err(|error| Error::ParseError(format!("failed to parse Arch keyring: {error}")))?
    {
        let certificate = parsed
            .map_err(|error| Error::ParseError(format!("malformed Arch keyring: {error}")))?;
        let fingerprint = certificate.fingerprint().to_hex();
        match certificates.remove(&fingerprint) {
            Some(existing) => {
                let merged = existing.merge_public(certificate).map_err(|error| {
                    Error::ParseError(format!(
                        "failed to merge Arch keyring certificate '{fingerprint}': {error}"
                    ))
                })?;
                certificates.insert(fingerprint, merged);
            }
            None => {
                certificates.insert(fingerprint, certificate);
            }
        }
    }
    if certificates.is_empty() {
        return Err(Error::GpgVerificationFailed(
            "Arch keyring contains no OpenPGP certificates".to_string(),
        ));
    }
    for master in &keyring.master_fingerprints {
        if !certificates.contains_key(master) {
            return Err(Error::GpgVerificationFailed(format!(
                "Arch keyring '{}' does not contain pinned master certificate '{}'",
                keyring.url, master
            )));
        }
    }
    Ok(certificates.into_values().collect())
}

#[cfg(test)]
pub(super) fn alpm_keyring_package(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    for (path, content) in members {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(content.len() as u64);
        header.set_cksum();
        archive.append_data(&mut header, path, *content).unwrap();
    }
    let tar = archive.into_inner().unwrap();
    zstd::encode_all(Cursor::new(tar), 0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVOKED: &[u8] = b"0A9DDABB64B993D82AD45E4F32EAB0A976938292\n";

    #[test]
    fn alpm_keyring_source_requires_both_exact_members() {
        let content = b"openpgp keyring";
        let package = alpm_keyring_package(&[
            (".PKGINFO", b"pkgname = archlinux-keyring"),
            (CERTIFICATE_MEMBER, content),
            (REVOKED_MEMBER, REVOKED),
        ]);
        let source = extract_keyring_source(&package, ArchKeyringFormat::AlpmPackageZstd).unwrap();
        assert_eq!(source.certificates, content);
        assert!(
            source
                .disabled
                .contains("0A9DDABB64B993D82AD45E4F32EAB0A976938292")
        );

        let lookalike = alpm_keyring_package(&[
            (
                "usr/share/pacman/keyrings/lookalike-archlinux.gpg",
                content as &[u8],
            ),
            (REVOKED_MEMBER, REVOKED),
        ]);
        let error =
            extract_keyring_source(&lookalike, ArchKeyringFormat::AlpmPackageZstd).unwrap_err();
        assert!(error.to_string().contains("archlinux.gpg' member"));
    }

    #[test]
    fn alpm_keyring_source_requires_the_revoked_list() {
        let package = alpm_keyring_package(&[(CERTIFICATE_MEMBER, b"openpgp keyring")]);
        let error =
            extract_keyring_source(&package, ArchKeyringFormat::AlpmPackageZstd).unwrap_err();
        assert!(error.to_string().contains("archlinux-revoked' member"));
    }

    #[test]
    fn alpm_keyring_source_rejects_duplicate_authority_members() {
        let package = alpm_keyring_package(&[
            (CERTIFICATE_MEMBER, b"first"),
            (CERTIFICATE_MEMBER, b"second"),
            (REVOKED_MEMBER, REVOKED),
        ]);
        let error =
            extract_keyring_source(&package, ArchKeyringFormat::AlpmPackageZstd).unwrap_err();
        assert!(error.to_string().contains("repeats exact member"));

        let package = alpm_keyring_package(&[
            (CERTIFICATE_MEMBER, b"first"),
            (REVOKED_MEMBER, REVOKED),
            (REVOKED_MEMBER, REVOKED),
        ]);
        let error =
            extract_keyring_source(&package, ArchKeyringFormat::AlpmPackageZstd).unwrap_err();
        assert!(error.to_string().contains("repeats exact member"));
    }

    #[test]
    fn bare_keyring_transport_carries_no_revoked_list() {
        let source =
            extract_keyring_source(b"openpgp keyring", ArchKeyringFormat::OpenPgp).unwrap();
        assert_eq!(source.disabled, ArchDisabledKeys::NotCarried);
        assert!(!source.disabled.contains(&"A".repeat(40)));
    }

    #[test]
    fn revoked_list_grammar_is_exact() {
        let parsed = parse_revoked_list(b"\n0a9ddabb64b993d82ad45e4f32eab0a976938292\n\n").unwrap();
        assert_eq!(
            parsed,
            BTreeSet::from(["0A9DDABB64B993D82AD45E4F32EAB0A976938292".to_string()])
        );

        assert!(parse_revoked_list(b"not-a-fingerprint\n").is_err());
        assert!(parse_revoked_list(b"0A9DDABB64B993D82AD45E4F32EAB0A9769382\n").is_err());
        let repeated = format!("{0}\n{0}\n", "0A9DDABB64B993D82AD45E4F32EAB0A976938292");
        assert!(parse_revoked_list(repeated.as_bytes()).is_err());
    }

    #[test]
    fn disabled_key_state_round_trips_through_the_trust_store() {
        let disabled = ArchDisabledKeys::Carried(parse_revoked_list(REVOKED).unwrap());
        let serialized = disabled.serialize();
        assert_eq!(
            ArchDisabledKeys::Carried(parse_revoked_list(&serialized).unwrap()),
            disabled
        );
    }
}
