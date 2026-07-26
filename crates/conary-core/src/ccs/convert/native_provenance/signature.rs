// conary-core/src/ccs/convert/native_provenance/signature.rs

//! Exact native-package signature extraction.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use sequoia_openpgp::{Packet, parse::Parse};
use std::fs::File;
use std::io::{BufReader, Read};

/// Signature information extracted from a package.
#[derive(Debug, Clone)]
pub(super) struct ExtractedSignature {
    pub key_id: Option<String>,
    pub sig_type: String,
    pub signature_data: String,
}

/// Extract the first exact signature payload represented by an RPM signature
/// header. Absence is `None`; malformed headers and I/O are errors.
pub(super) fn extract_rpm_signature(path: &str) -> Result<Option<ExtractedSignature>> {
    use rpm::IndexSignatureTag;

    let file = File::open(path)
        .with_context(|| format!("failed to open RPM signature source {path:?}"))?;
    let mut reader = BufReader::new(file);
    let package = rpm::Package::parse(&mut reader)
        .with_context(|| format!("failed to parse RPM signature header from {path:?}"))?;
    let header = &package.metadata.signature;

    match header.get_entry_data_as_string_array(IndexSignatureTag::RPMSIGTAG_OPENPGP) {
        Ok(encoded) => {
            if encoded.len() != 1 {
                bail!(
                    "RPM signature header contains {} OpenPGP payloads; exactly one is supported",
                    encoded.len()
                );
            }
            let payload = STANDARD
                .decode(encoded[0])
                .context("RPM OpenPGP signature payload is not valid base64")?;
            let key_id = extract_openpgp_key_id(&payload)?;
            return Ok(Some(ExtractedSignature {
                key_id,
                sig_type: "OPENPGP".to_string(),
                signature_data: STANDARD.encode(payload),
            }));
        }
        Err(rpm::Error::TagNotFound(_)) => {}
        Err(error) => return Err(error).context("malformed RPM OpenPGP signature tag"),
    }

    for (tag, signature_type) in [
        (IndexSignatureTag::RPMSIGTAG_RSA, "RSA"),
        (IndexSignatureTag::RPMSIGTAG_DSA, "DSA"),
        (IndexSignatureTag::RPMSIGTAG_PGP, "PGP"),
    ] {
        match header.get_entry_data_as_binary(tag) {
            Ok(payload) => return Ok(Some(extracted_payload(signature_type, payload))),
            Err(rpm::Error::TagNotFound(_)) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("malformed RPM {signature_type} signature tag"));
            }
        }
    }

    Ok(None)
}

fn extracted_payload(signature_type: &str, payload: &[u8]) -> ExtractedSignature {
    ExtractedSignature {
        key_id: extract_rpm_pgp_key_id(payload),
        sig_type: signature_type.to_string(),
        signature_data: STANDARD.encode(payload),
    }
}

/// RPM legacy signature tags store the OpenPGP signature packet body without
/// its packet framing. Parse the exact v3/v4 issuer fields from that grammar.
fn extract_rpm_pgp_key_id(signature: &[u8]) -> Option<String> {
    match signature.first().copied()? {
        3 if signature.len() >= 15 && signature[1] == 5 => {
            Some(hex::encode(&signature[7..15]).to_uppercase())
        }
        4 if signature.len() >= 8 => {
            let hashed_len = u16::from_be_bytes([signature[4], signature[5]]) as usize;
            let unhashed_len_offset = 6usize.checked_add(hashed_len)?;
            let unhashed_len_bytes = signature.get(unhashed_len_offset..unhashed_len_offset + 2)?;
            let unhashed_len =
                u16::from_be_bytes([unhashed_len_bytes[0], unhashed_len_bytes[1]]) as usize;
            let start = unhashed_len_offset + 2;
            let end = start.checked_add(unhashed_len)?;
            find_v4_issuer_key_id(signature.get(start..end)?)
        }
        _ => None,
    }
}

fn find_v4_issuer_key_id(mut subpackets: &[u8]) -> Option<String> {
    while !subpackets.is_empty() {
        let (packet_len, length_octets) = parse_signature_subpacket_len(subpackets)?;
        if packet_len == 0 {
            return None;
        }
        let packet_end = length_octets.checked_add(packet_len)?;
        let packet = subpackets.get(length_octets..packet_end)?;
        let packet_type = packet.first()? & 0x7f;
        if packet_type == 16 && packet.len() == 9 {
            return Some(hex::encode(&packet[1..]).to_uppercase());
        }
        subpackets = subpackets.get(packet_end..)?;
    }
    None
}

fn parse_signature_subpacket_len(bytes: &[u8]) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    match first {
        0..=191 => Some((first as usize, 1)),
        192..=223 => {
            let second = *bytes.get(1)? as usize;
            let length = ((first as usize - 192) << 8) + second + 192;
            Some((length, 2))
        }
        255 => {
            let encoded = bytes.get(1..5)?;
            let length = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
            Some((usize::try_from(length).ok()?, 5))
        }
        224..=254 => None,
    }
}

/// Extract a Debian `_gpgorigin` detached signature. Archive read failures and
/// malformed OpenPGP payloads are errors; structural absence is `None`.
pub(super) fn extract_deb_signature(path: &str) -> Result<Option<ExtractedSignature>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open DEB signature source {path:?}"))?;
    let mut archive = ar::Archive::new(file);

    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.context("failed to read DEB ar archive entry")?;
        if entry.header().identifier() != b"_gpgorigin" {
            continue;
        }

        let mut payload = Vec::new();
        entry
            .read_to_end(&mut payload)
            .context("failed to read DEB _gpgorigin signature payload")?;
        let key_id = extract_openpgp_key_id(&payload)?;
        return Ok(Some(ExtractedSignature {
            key_id,
            sig_type: "OPENPGP".to_string(),
            signature_data: STANDARD.encode(payload),
        }));
    }

    Ok(None)
}

fn extract_openpgp_key_id(payload: &[u8]) -> Result<Option<String>> {
    let packet = Packet::from_bytes(payload).context("malformed OpenPGP signature payload")?;
    let Packet::Signature(signature) = packet else {
        bail!("OpenPGP payload is not a signature packet");
    };
    Ok(signature
        .get_issuers()
        .first()
        .map(|issuer| format!("{issuer:X}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_key_id_parser_rejects_truncated_and_unknown_versions() {
        assert!(extract_rpm_pgp_key_id(&[0x04, 0x00, 0x01]).is_none());
        assert!(
            extract_rpm_pgp_key_id(&[0x05, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08])
                .is_none()
        );
    }

    #[test]
    fn rpm_v3_key_id_uses_the_fixed_issuer_field() {
        let signature = [
            0x03, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67,
            0x89, 0x01, 0x08,
        ];
        assert_eq!(
            extract_rpm_pgp_key_id(&signature).as_deref(),
            Some("ABCDEF0123456789")
        );
    }

    #[test]
    fn rpm_v4_key_id_uses_the_issuer_subpacket() {
        let signature = [
            0x04, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00, 0x0A, 0x09, 0x10, 0xAB, 0xCD, 0xEF, 0x01,
            0x23, 0x45, 0x67, 0x89,
        ];
        assert_eq!(
            extract_rpm_pgp_key_id(&signature).as_deref(),
            Some("ABCDEF0123456789")
        );
    }

    #[test]
    fn armor_headers_cannot_invent_an_issuer_without_a_signature_packet() {
        let fake = b"-----BEGIN PGP SIGNATURE-----\nKey ID: ABCD1234\n\nnot-a-signature\n-----END PGP SIGNATURE-----";
        assert!(extract_openpgp_key_id(fake).is_err());
    }

    #[test]
    fn openpgp_parser_extracts_the_packet_issuer() {
        let signature = b"-----BEGIN PGP SIGNATURE-----\n\nwsBzBAABCgAdFiEEwD+mQRsDrhJXZGEYciO1ZnjgJSgFAlnclx8ACgkQciO1Znjg\nJShldAf+NBvUTVPnVPhYM4KihWOUlup8lbD6g1IduSM5rpsGvOVb+uKF6ik+GOBB\nRlMT4s183r3teFxiTkDx2pRhUz0MnOMPfbXovjF6Y93fKCOxCQWLBa0ukjNmE+ax\ngu9nZ3XXDGXZW22iGE52uVjPGSfuLfqvdMy5bKHn8xow/kepuGHZwy8yn7uFv7sl\nLnOBUz1FKA7iRl457XKPUhw5K7BnfRW/I2BRlnrwTDkjfXaJZC+bUTIJvm682Bvt\nZNn8zc0JucyEkuL9WXYNuZg0znDE3T7D/6+tzfEdSf706unsXFXWHf83vL2eHCcw\nqhImm1lmcC+agFtWQ6/qD923LR9xmg==\n=htNu\n-----END PGP SIGNATURE-----";

        assert_eq!(
            extract_openpgp_key_id(signature).unwrap().as_deref(),
            Some("C03FA6411B03AE12576461187223B56678E02528")
        );
    }
}
