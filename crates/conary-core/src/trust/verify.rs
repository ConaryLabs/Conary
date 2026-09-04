// conary-core/src/trust/verify.rs

//! TUF signature and metadata verification
//!
//! Provides the core verification primitives for TUF trust:
//! - Signature threshold verification (enough valid signatures?)
//! - Version monotonicity (rollback protection)
//! - Expiration checking (freeze protection)
//! - Snapshot consistency (mix-and-match protection)

use crate::hash;
use crate::trust::keys::{canonical_json, compute_key_id};
use crate::trust::metadata::{MetaFile, Role, RootMetadata, Signed, SnapshotMetadata, TufKey};
use crate::trust::{TrustError, TrustResult};
use chrono::Utc;
use ed25519_dalek::{Signature, VerifyingKey};
use std::collections::BTreeMap;
use std::path::Path;

/// Verify that a signed metadata document has enough valid signatures
///
/// Checks each signature against the provided keys and verifies that
/// the threshold is met.
///
/// IMPORTANT: Callers MUST pass only role-specific keys, not the full
/// `root.keys` map. Use `extract_role_keys()` to filter keys for the
/// target role. Passing the full key map would allow any key for any
/// role to satisfy the threshold.
pub fn verify_signatures<T: serde::Serialize>(
    signed: &Signed<T>,
    role: Role,
    keys: &BTreeMap<String, TufKey>,
    threshold: u64,
) -> TrustResult<()> {
    if threshold == 0 {
        return Err(TrustError::ConsistencyError(format!(
            "{role} signature threshold must be at least 1"
        )));
    }

    let canonical = canonical_json(&signed.signed)?;
    let mut valid_count: u64 = 0;
    let mut seen_keyids = std::collections::HashSet::new();

    for sig in &signed.signatures {
        // Skip duplicate key IDs
        if !seen_keyids.insert(&sig.keyid) {
            continue;
        }

        // Look up the key
        let Some(tuf_key) = keys.get(&sig.keyid) else {
            continue;
        };

        // Only support ed25519
        if tuf_key.keytype != "ed25519" {
            continue;
        }

        // Verify the signature
        if verify_ed25519_signature(&canonical, &sig.sig, &tuf_key.keyval.public).is_ok() {
            valid_count += 1;
        }
    }

    if valid_count >= threshold {
        Ok(())
    } else {
        Err(TrustError::ThresholdNotMet {
            role: role.to_string(),
            threshold,
            got: valid_count,
        })
    }
}

/// Verify a single Ed25519 signature
fn verify_ed25519_signature(
    message: &[u8],
    sig_hex: &str,
    public_key_hex: &str,
) -> TrustResult<()> {
    let public_bytes = hex::decode(public_key_hex)
        .map_err(|e| TrustError::KeyError(format!("Invalid public key hex: {e}")))?;

    let public_key_bytes: [u8; 32] = public_bytes
        .try_into()
        .map_err(|_| TrustError::KeyError("Public key must be 32 bytes".to_string()))?;

    let verifying_key = VerifyingKey::from_bytes(&public_key_bytes)
        .map_err(|e| TrustError::KeyError(format!("Invalid Ed25519 public key: {e}")))?;

    let sig_bytes = hex::decode(sig_hex)
        .map_err(|e| TrustError::VerificationFailed(format!("Invalid signature hex: {e}")))?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| TrustError::VerificationFailed(format!("Invalid signature: {e}")))?;

    verifying_key
        .verify_strict(message, &signature)
        .map_err(|_| TrustError::VerificationFailed("Signature verification failed".to_string()))
}

/// Verify that a metadata version is strictly increasing (rollback protection)
pub fn verify_version_increase(
    role: Role,
    new_version: u64,
    stored_version: u64,
) -> TrustResult<()> {
    if new_version > stored_version {
        Ok(())
    } else {
        Err(TrustError::RollbackAttack {
            role: role.to_string(),
            new: new_version,
            stored: stored_version,
        })
    }
}

/// Verify that metadata has not expired (freeze protection)
pub fn verify_not_expired(role: Role, expires: &chrono::DateTime<Utc>) -> TrustResult<()> {
    if Utc::now() < *expires {
        Ok(())
    } else {
        Err(TrustError::MetadataExpired {
            role: role.to_string(),
            expires: expires.to_rfc3339(),
        })
    }
}

/// Verify snapshot consistency with root and targets versions
///
/// Ensures the snapshot pins the expected versions of other metadata,
/// preventing mix-and-match attacks.
pub fn verify_snapshot_consistency(
    snapshot: &SnapshotMetadata,
    expected_root_version: u64,
    expected_targets_version: Option<u64>,
) -> TrustResult<()> {
    // Check root version in snapshot
    if let Some(root_meta) = snapshot.meta.get("root.json")
        && root_meta.version != expected_root_version
    {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins root.json v{} but expected v{}",
            root_meta.version, expected_root_version
        )));
    }

    // Check targets version if provided
    if let Some(expected_tv) = expected_targets_version
        && let Some(targets_meta) = snapshot.meta.get("targets.json")
        && targets_meta.version != expected_tv
    {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins targets.json v{} but expected v{}",
            targets_meta.version, expected_tv
        )));
    }

    Ok(())
}

/// Verify strict static-repository snapshot consistency.
///
/// Static repositories must publish a complete snapshot that explicitly pins
/// both root and targets metadata. This is stricter than the generic helper
/// because static publish/sync relies on those references as a closed metadata
/// set rather than accepting partially populated TUF snapshots.
pub fn verify_static_snapshot_consistency(
    snapshot: &SnapshotMetadata,
    expected_root_version: u64,
    expected_targets_version: u64,
) -> TrustResult<()> {
    let Some(root_meta) = snapshot.meta.get("root.json") else {
        return Err(TrustError::ConsistencyError(
            "Snapshot missing mandatory root.json reference".to_string(),
        ));
    };
    let Some(targets_meta) = snapshot.meta.get("targets.json") else {
        return Err(TrustError::ConsistencyError(
            "Snapshot missing mandatory targets.json reference".to_string(),
        ));
    };
    if root_meta.version != expected_root_version {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins root.json v{} but expected v{}",
            root_meta.version, expected_root_version
        )));
    }
    if targets_meta.version != expected_targets_version {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins targets.json v{} but expected v{}",
            targets_meta.version, expected_targets_version
        )));
    }
    Ok(())
}

/// Verify that a hash matches the expected value from a MetaFile reference
///
/// When `require_hash` is true, returns an error if the metadata reference
/// does not contain a sha256 hash. Use `require_hash: true` for critical
/// cross-references (snapshot->targets, timestamp->snapshot) to prevent
/// downgrade attacks where an attacker strips hashes from metadata.
pub fn verify_metadata_hash(
    meta_ref: &MetaFile,
    actual_bytes: &[u8],
    require_hash: bool,
) -> TrustResult<()> {
    if let Some(ref hashes) = meta_ref.hashes
        && let Some(expected_sha256) = hashes.get("sha256")
    {
        let actual_hash = hash::sha256(actual_bytes);
        if actual_hash != *expected_sha256 {
            return Err(TrustError::ConsistencyError(format!(
                "Hash mismatch: expected {expected_sha256}, got {actual_hash}"
            )));
        }
    } else if require_hash {
        return Err(TrustError::ConsistencyError(
            "Metadata reference is missing required sha256 hash".to_string(),
        ));
    }
    Ok(())
}

/// Extract keys and threshold for a specific role from root metadata
pub fn extract_role_keys(
    root: &RootMetadata,
    role: Role,
) -> TrustResult<(BTreeMap<String, TufKey>, u64)> {
    validate_root_key_ids(root)?;

    let role_name = role.to_string();
    let role_def = root.roles.get(&role_name).ok_or_else(|| {
        TrustError::ConsistencyError(format!(
            "Root metadata missing role definition: {role_name}"
        ))
    })?;
    if role_def.threshold == 0 {
        return Err(TrustError::ConsistencyError(format!(
            "Role {role_name} threshold must be at least 1"
        )));
    }

    let mut role_keys = BTreeMap::new();
    for keyid in &role_def.keyids {
        let Some(key) = root.keys.get(keyid) else {
            return Err(TrustError::ConsistencyError(format!(
                "Role {role_name} references missing key ID {keyid}"
            )));
        };
        role_keys.insert(keyid.clone(), key.clone());
    }

    if role_keys.is_empty() {
        return Err(TrustError::KeyError(format!(
            "No keys found for role {role_name}"
        )));
    }
    if role_def.threshold > role_keys.len() as u64 {
        return Err(TrustError::ConsistencyError(format!(
            "Role {role_name} threshold {} exceeds available key count {}",
            role_def.threshold,
            role_keys.len()
        )));
    }

    Ok((role_keys, role_def.threshold))
}

fn validate_root_key_ids(root: &RootMetadata) -> TrustResult<()> {
    for (key_id, key) in &root.keys {
        let computed = compute_key_id(key)?;
        if computed != *key_id {
            return Err(TrustError::KeyError(format!(
                "Root key ID {key_id} does not match canonical key ID {computed}"
            )));
        }
    }
    Ok(())
}

/// Verify a file's SHA-256 hash matches the expected value from a `MetaFile` reference
///
/// Reads the file at `path` and verifies its content hash. IO errors include
/// the file path for easier debugging.
pub fn verify_file(meta_ref: &MetaFile, path: &Path) -> TrustResult<()> {
    let content = std::fs::read(path).map_err(|e| {
        TrustError::VerificationFailed(format!(
            "failed to read file for verification ({}): {}",
            path.display(),
            e
        ))
    })?;
    verify_metadata_hash(meta_ref, &content, false)
}

/// Verify root metadata self-signatures using its own keys
///
/// Root is special: it's verified against both the old root keys
/// (from the previously trusted root) and its own new keys.
pub fn verify_root(
    signed_root: &Signed<RootMetadata>,
    trusted_keys: &BTreeMap<String, TufKey>,
    trusted_threshold: u64,
) -> TrustResult<()> {
    // Verify signatures against the trusted keys
    verify_signatures(signed_root, Role::Root, trusted_keys, trusted_threshold)?;

    // Also verify the root is self-signed (new keys must also meet threshold)
    let (new_keys, new_threshold) = extract_role_keys(&signed_root.signed, Role::Root)?;
    verify_signatures(signed_root, Role::Root, &new_keys, new_threshold)?;

    Ok(())
}

#[cfg(test)]
mod tests;
