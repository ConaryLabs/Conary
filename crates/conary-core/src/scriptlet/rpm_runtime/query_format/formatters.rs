// conary-core/src/scriptlet/rpm_runtime/query_format/formatters.rs
//! RPM query-format renderers whose behavior depends on typed value classes.
//! Source authority: RPM `lib/formats.cc` at
//! `a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`.

use crate::ccs::native_lifecycle::{
    RpmHeaderValue, RpmQueryFormatEnvironment, RpmQueryFormatTimezone,
};
use anyhow::{Context, Result};
use base64::Engine as _;
use chrono::{Datelike as _, Timelike as _};
use sequoia_openpgp as openpgp;
use std::io::Write as _;
use std::time::UNIX_EPOCH;

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(super) fn date(value: u64, environment: &RpmQueryFormatEnvironment) -> Result<String> {
    let Some(date) = local_datetime(value, environment) else {
        return Ok(String::new());
    };
    Ok(format!(
        "{} {} {:>2} {:02}:{:02}:{:02} {}",
        WEEKDAYS[date.weekday().num_days_from_sunday() as usize],
        MONTHS[date.month0() as usize],
        date.day(),
        date.hour(),
        date.minute(),
        date.second(),
        date.year()
    ))
}

pub(super) fn day(value: u64, environment: &RpmQueryFormatEnvironment) -> Result<String> {
    let Some(date) = local_datetime(value, environment) else {
        return Ok(String::new());
    };
    Ok(format!(
        "{} {} {:02} {}",
        WEEKDAYS[date.weekday().num_days_from_sunday() as usize],
        MONTHS[date.month0() as usize],
        date.day(),
        date.year()
    ))
}

fn local_datetime(
    value: u64,
    environment: &RpmQueryFormatEnvironment,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let timestamp = i64::try_from(value).ok()?;
    let offset = match environment.timezone {
        RpmQueryFormatTimezone::Utc => chrono::FixedOffset::east_opt(0)?,
        RpmQueryFormatTimezone::FixedOffset {
            seconds_east_of_utc,
        } => chrono::FixedOffset::east_opt(seconds_east_of_utc)?,
    };
    chrono::DateTime::from_timestamp(timestamp, 0).map(|date| date.with_timezone(&offset))
}

pub(super) fn armor(value: &RpmHeaderValue, index: usize) -> Result<String> {
    let (bytes, kind) = match value {
        RpmHeaderValue::Binary(value) if index == 0 => (
            match base64::engine::general_purpose::STANDARD.decode(value) {
                Ok(bytes) => bytes,
                Err(_) => return Ok("(not base64)".to_string()),
            },
            openpgp::armor::Kind::Signature,
        ),
        RpmHeaderValue::String(value) if index == 0 => (
            match base64::engine::general_purpose::STANDARD.decode(value) {
                Ok(bytes) => bytes,
                Err(_) => return Ok("(not base64)".to_string()),
            },
            openpgp::armor::Kind::PublicKey,
        ),
        RpmHeaderValue::StringArray(values) => {
            let Some(value) = values.get(index) else {
                return Ok("(none)".to_string());
            };
            (
                match base64::engine::general_purpose::STANDARD.decode(value) {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok("(not base64)".to_string()),
                },
                openpgp::armor::Kind::PublicKey,
            )
        }
        _ => return Ok("(invalid type)".to_string()),
    };

    let mut writer = openpgp::armor::Writer::new(Vec::new(), kind)
        .context("RpmQueryFormatArmorEncodingFailed")?;
    writer
        .write_all(&bytes)
        .context("RpmQueryFormatArmorEncodingFailed")?;
    let encoded = writer
        .finalize()
        .context("RpmQueryFormatArmorEncodingFailed")?;
    String::from_utf8(encoded).context("RpmQueryFormatArmorEncodingFailed")
}

pub(super) fn pgpsig(
    value: &RpmHeaderValue,
    index: usize,
    environment: &RpmQueryFormatEnvironment,
) -> Result<String> {
    let bytes = match value {
        RpmHeaderValue::Binary(value) if index == 0 => {
            match base64::engine::general_purpose::STANDARD.decode(value) {
                Ok(bytes) => bytes,
                Err(_) => return Ok("(not an OpenPGP signature)".to_string()),
            }
        }
        RpmHeaderValue::StringArray(values) => {
            let Some(value) = values.get(index) else {
                return Ok("(none)".to_string());
            };
            match base64::engine::general_purpose::STANDARD.decode(value) {
                Ok(bytes) => bytes,
                Err(_) => return Ok("(not an OpenPGP signature)".to_string()),
            }
        }
        _ => return Ok("(invalid type)".to_string()),
    };

    use openpgp::parse::Parse as _;
    let packet = match openpgp::Packet::from_bytes(&bytes) {
        Ok(openpgp::Packet::Signature(signature)) => signature,
        _ => return Ok("(not an OpenPGP signature)".to_string()),
    };
    let creation_time = packet
        .signature_creation_time()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs());
    let rendered_time = date(creation_time, environment)?;
    if rendered_time.is_empty() {
        return Ok(format!("Invalid date {creation_time}"));
    }

    let key_id = packet
        .get_issuers()
        .into_iter()
        .next()
        .map(openpgp::KeyID::from)
        .map_or_else(
            || "0000000000000000".to_string(),
            |key_id| hex::encode(key_id.as_bytes()),
        );
    Ok(format!(
        "{}/{}, {}, Key ID {}",
        public_key_algorithm(u8::from(packet.pk_algo())),
        hash_algorithm(u8::from(packet.hash_algo())),
        rendered_time,
        key_id
    ))
}

fn public_key_algorithm(value: u8) -> &'static str {
    match value {
        1 => "RSA",
        2 => "RSA(Encrypt-Only)",
        3 => "RSA(Sign-Only)",
        16 => "Elgamal(Encrypt-Only)",
        17 => "DSA",
        18 => "Elliptic Curve",
        19 => "ECDSA",
        20 => "Elgamal",
        21 => "Diffie-Hellman (X9.42)",
        22 => "EdDSA",
        25 => "X25519",
        26 => "X448",
        27 => "Ed25519",
        28 => "Ed448",
        30 => "ML-DSA-65+Ed25519",
        31 => "ML-DSA-87+Ed448",
        32 => "SLH-DSA-SHAKE-128s",
        33 => "SLH-DSA-SHAKE-128f",
        34 => "SLH-DSA-SHAKE-256s",
        35 => "ML-KEM-768+X25519",
        36 => "ML-KEM-1024+X448",
        _ => "Unknown public key algorithm",
    }
}

fn hash_algorithm(value: u8) -> &'static str {
    match value {
        1 => "MD5",
        2 => "SHA1",
        3 => "RIPEMD160",
        5 => "MD2",
        6 => "TIGER192",
        7 => "HAVAL-5-160",
        8 => "SHA256",
        9 => "SHA384",
        10 => "SHA512",
        11 => "SHA224",
        12 => "SHA3-256",
        14 => "SHA3-512",
        _ => "Unknown hash algorithm",
    }
}

#[cfg(test)]
mod tests {
    use super::{hash_algorithm, public_key_algorithm};

    #[test]
    fn algorithm_names_match_rpm_pgp_value_tables() {
        assert_eq!(
            [
                1, 2, 3, 16, 17, 18, 19, 20, 21, 22, 25, 26, 27, 28, 30, 31, 32, 33, 34, 35, 36,
            ]
            .map(public_key_algorithm),
            [
                "RSA",
                "RSA(Encrypt-Only)",
                "RSA(Sign-Only)",
                "Elgamal(Encrypt-Only)",
                "DSA",
                "Elliptic Curve",
                "ECDSA",
                "Elgamal",
                "Diffie-Hellman (X9.42)",
                "EdDSA",
                "X25519",
                "X448",
                "Ed25519",
                "Ed448",
                "ML-DSA-65+Ed25519",
                "ML-DSA-87+Ed448",
                "SLH-DSA-SHAKE-128s",
                "SLH-DSA-SHAKE-128f",
                "SLH-DSA-SHAKE-256s",
                "ML-KEM-768+X25519",
                "ML-KEM-1024+X448",
            ]
        );
        assert_eq!(
            [1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 14].map(hash_algorithm),
            [
                "MD5",
                "SHA1",
                "RIPEMD160",
                "MD2",
                "TIGER192",
                "HAVAL-5-160",
                "SHA256",
                "SHA384",
                "SHA512",
                "SHA224",
                "SHA3-256",
                "SHA3-512",
            ]
        );
        assert_eq!(public_key_algorithm(29), "Unknown public key algorithm");
        assert_eq!(hash_algorithm(13), "Unknown hash algorithm");
    }
}
