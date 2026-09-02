// apps/remi/src/server/universe_revision_inspection.rs

//! Typed inspection of stored universe revisions across profile-schema hard cuts.

use anyhow::{Context, Result, bail};
use conary_core::repository::catalog::PROFILE_REVISION_SCHEMA_V3;
use conary_core::repository::universe::{REMI_UNIVERSE_SCHEMA_V2, RemiUniverseManifestV2};

#[derive(Debug)]
pub(crate) enum StoredUniverseManifestV2 {
    Current(RemiUniverseManifestV2),
    ObsoleteProfileSchema,
}

pub(crate) fn inspect_stored_universe_manifest_v2(
    manifest_sha256: &str,
    sequence: i64,
    manifest_json: &str,
) -> Result<StoredUniverseManifestV2> {
    let raw: serde_json::Value =
        serde_json::from_str(manifest_json).context("parse stored Remi universe manifest")?;
    let canonical_raw = conary_core::json::canonical_json(&raw)
        .map_err(anyhow::Error::msg)
        .context("canonicalize stored Remi universe manifest")?;
    let sequence = u64::try_from(sequence).context("stored universe sequence is negative")?;
    let raw_sequence = raw
        .get("sequence")
        .and_then(serde_json::Value::as_u64)
        .context("stored Remi universe sequence must be an unsigned integer")?;
    let universe_schema = raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok())
        .context("stored Remi universe schema_version must be an unsigned 32-bit integer")?;
    if canonical_raw != manifest_json.as_bytes()
        || conary_core::hash::sha256(manifest_json.as_bytes()) != manifest_sha256
        || raw_sequence != sequence
    {
        bail!("stored Remi universe pointer disagrees with its manifest authority");
    }
    if universe_schema != REMI_UNIVERSE_SCHEMA_V2 {
        bail!(
            "stored Remi universe schema must be {REMI_UNIVERSE_SCHEMA_V2}, got {universe_schema}"
        );
    }

    let mut obsolete = false;
    for (ordinal, profile) in raw
        .get("profiles")
        .and_then(serde_json::Value::as_array)
        .context("stored Remi universe profiles must be an array")?
        .iter()
        .enumerate()
    {
        let schema = profile
            .get("revision")
            .and_then(|revision| revision.get("schema_version"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|schema| u32::try_from(schema).ok())
            .with_context(|| {
                format!(
                    "stored Remi universe profile {ordinal} revision schema_version must be an unsigned 32-bit integer"
                )
            })?;
        if schema > PROFILE_REVISION_SCHEMA_V3 {
            bail!(
                "stored Remi universe profile {ordinal} revision schema {schema} is newer than required schema {PROFILE_REVISION_SCHEMA_V3}"
            );
        }
        obsolete |= schema < PROFILE_REVISION_SCHEMA_V3;
    }
    if obsolete {
        return Ok(StoredUniverseManifestV2::ObsoleteProfileSchema);
    }

    let manifest: RemiUniverseManifestV2 =
        serde_json::from_value(raw).context("parse current Remi universe manifest")?;
    manifest.validate().map_err(anyhow::Error::from)?;
    Ok(StoredUniverseManifestV2::Current(manifest))
}
