// crates/conary-core/src/db/models/remi_catalog/validation.rs

//! Closed validation rules for operational immutable-catalog metadata.

use crate::error::{Error, Result};

pub(super) fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

pub(super) fn validate_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must contain 1 to 255 printable ASCII characters without surrounding whitespace"
        )));
    }
    Ok(())
}

pub(super) fn validate_storage_component(value: &str, label: &str) -> Result<()> {
    validate_identity(value, label)?;
    if value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be a safe ASCII storage-path component"
        )));
    }
    Ok(())
}

pub(super) fn validate_uuid(value: &str, label: &str) -> Result<()> {
    if value.len() != 36 || uuid::Uuid::parse_str(value).is_err() {
        return Err(Error::ConfigError(format!(
            "{label} must be a canonical 36-character UUID"
        )));
    }
    Ok(())
}

pub(super) fn validate_canonical_manifest(value: &str) -> Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|error| {
        Error::ConfigError(format!("catalog manifest JSON is malformed: {error}"))
    })?;
    if !parsed.is_object() {
        return Err(Error::ConfigError(
            "catalog manifest JSON must be an object".to_string(),
        ));
    }
    let canonical = crate::json::canonical_json(&parsed).map_err(|error| {
        Error::ConfigError(format!("catalog manifest is not serializable: {error}"))
    })?;
    if canonical.as_slice() != value.as_bytes() {
        return Err(Error::ConfigError(
            "catalog manifest JSON must use canonical object ordering".to_string(),
        ));
    }
    Ok(())
}
