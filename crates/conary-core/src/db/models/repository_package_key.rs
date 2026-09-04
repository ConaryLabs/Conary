// conary-core/src/db/models/repository_package_key.rs

//! Repository package signing key persistence.

use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rusqlite::{Connection, Transaction, params};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryPackageKeyStatus {
    Active,
    Retired,
}

impl RepositoryPackageKeyStatus {
    fn as_db_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPackageKey {
    pub repository_id: i64,
    pub public_key: String,
    pub key_id: Option<String>,
    pub status: RepositoryPackageKeyStatus,
    pub synced_at: Option<String>,
}

impl RepositoryPackageKey {
    pub fn replace_for_repository(
        conn: &Connection,
        repository_id: i64,
        keys: &[Self],
    ) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        Self::replace_for_repository_in_transaction(&tx, repository_id, keys)?;
        tx.commit()?;
        Ok(())
    }

    /// Replace the exact authority set inside a caller-owned transaction.
    pub fn replace_for_repository_in_transaction(
        tx: &Transaction<'_>,
        repository_id: i64,
        keys: &[Self],
    ) -> Result<()> {
        validate_replacement(repository_id, keys)?;
        replace_rows(tx, repository_id, keys)
    }

    /// Reconcile the exact authority set inside a caller-owned transaction.
    ///
    /// Returns `true` only when persisted authority changed. Synchronization
    /// timestamps are deliberately excluded from equality so an idempotent
    /// repository initialization does not churn trust state.
    pub fn reconcile_for_repository_in_transaction(
        tx: &Transaction<'_>,
        repository_id: i64,
        keys: &[Self],
    ) -> Result<bool> {
        validate_replacement(repository_id, keys)?;
        if authority_rows_match(tx, repository_id, keys)? {
            return Ok(false);
        }
        Self::replace_for_repository_in_transaction(tx, repository_id, keys)?;
        Ok(true)
    }

    pub fn trusted_keys_for_repository(
        conn: &Connection,
        repository_id: i64,
    ) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT public_key
             FROM repository_package_keys
             WHERE repository_id = ?1
               AND status = 'active'
             ORDER BY public_key",
        )?;

        let keys = stmt
            .query_map([repository_id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(keys)
    }

    /// Load exact Ed25519 package authority keys assigned to the TUF targets
    /// role by the repository's verified root metadata.
    pub fn trusted_tuf_targets_keys_for_repository(
        conn: &Connection,
        repository_id: i64,
    ) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT key_type, public_key, roles_json
             FROM tuf_keys
             WHERE repository_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([repository_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut keys = Vec::new();
        for (key_type, public_key, roles_json) in rows {
            let roles: Vec<String> = serde_json::from_str(&roles_json).map_err(|error| {
                Error::ParseError(format!(
                    "Invalid TUF key roles for repository {repository_id}: {error}"
                ))
            })?;
            if !roles.iter().any(|role| role == "targets") {
                continue;
            }
            if key_type != "ed25519" {
                return Err(Error::ParseError(format!(
                    "TUF targets key for repository {repository_id} uses unsupported type {key_type:?}"
                )));
            }
            let bytes = hex::decode(&public_key).map_err(|error| {
                Error::ParseError(format!(
                    "Invalid TUF targets public key for repository {repository_id}: {error}"
                ))
            })?;
            if bytes.len() != 32 {
                return Err(Error::ParseError(format!(
                    "TUF targets key for repository {repository_id} decoded to {} bytes; expected 32",
                    bytes.len()
                )));
            }
            keys.push(BASE64.encode(bytes));
        }
        Ok(keys)
    }
}

fn validate_replacement(repository_id: i64, keys: &[RepositoryPackageKey]) -> Result<()> {
    let mut public_keys = BTreeSet::new();
    for key in keys {
        if key.repository_id != repository_id {
            return Err(Error::InternalError(format!(
                "repository_id mismatch for repository package key: expected {repository_id}, got {}",
                key.repository_id
            )));
        }
        if !public_keys.insert(key.public_key.as_str()) {
            return Err(Error::ConflictError(format!(
                "repository {repository_id} package authority repeats public key {}",
                key.public_key
            )));
        }
    }
    Ok(())
}

fn authority_rows_match(
    conn: &Connection,
    repository_id: i64,
    keys: &[RepositoryPackageKey],
) -> Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT public_key, key_id, status
         FROM repository_package_keys
         WHERE repository_id = ?1
         ORDER BY public_key",
    )?;
    let observed = stmt
        .query_map([repository_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut expected = keys
        .iter()
        .map(|key| {
            (
                key.public_key.clone(),
                key.key_id.clone(),
                key.status.as_db_str().to_string(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    Ok(observed == expected)
}

fn replace_rows(
    conn: &Connection,
    repository_id: i64,
    keys: &[RepositoryPackageKey],
) -> Result<()> {
    conn.execute(
        "DELETE FROM repository_package_keys WHERE repository_id = ?1",
        [repository_id],
    )?;

    let mut insert_with_default_synced_at = conn.prepare(
        "INSERT INTO repository_package_keys (repository_id, public_key, key_id, status)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut insert_with_synced_at = conn.prepare(
        "INSERT INTO repository_package_keys
            (repository_id, public_key, key_id, status, synced_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    for key in keys {
        if let Some(synced_at) = &key.synced_at {
            insert_with_synced_at.execute(params![
                key.repository_id,
                &key.public_key,
                &key.key_id,
                key.status.as_db_str(),
                synced_at,
            ])?;
        } else {
            insert_with_default_synced_at.execute(params![
                key.repository_id,
                &key.public_key,
                &key.key_id,
                key.status.as_db_str(),
            ])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
