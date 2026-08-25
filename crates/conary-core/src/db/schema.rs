// conary-core/src/db/schema.rs

//! Current-only SQLite schema initialization.
//!
//! Conary is pre-alpha and does not carry forward databases from former
//! schema revisions. A database either has the current schema or
//! must be rebuilt from authoritative package and repository inputs.

use super::current_schema;
use crate::error::{Error, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use tracing::info;

/// Revision 51 of the current-only schema epoch.
///
/// Revision 45 makes registered Remi profile membership immutable and journals
/// exact catalog filesystem deletions before resource metadata disappears.
/// Revision 46 adds the durable singleton Remi runtime session and binds reader
/// pins to that exact session for crash recovery.
/// Revision 47 binds repository conversions to the exact immutable profile
/// revision that supplied their source metadata.
/// Revision 48 removes the mutable latest-authenticated-snapshot observation;
/// native refresh validates transient roots and immutable catalogs own source
/// revision identity.
/// Revision 50 hard-cuts exact ordered public profile membership.
/// Revision 51 adds artifact-level conversion proof and per-revision binding
/// authority for exact-key reuse.
/// Earlier pre-alpha databases must be rebuilt; no compatibility migration is
/// provided.
pub const SCHEMA_VERSION: i32 = 51;
/// Stable identity that distinguishes this epoch from retired schema revisions.
pub const SCHEMA_EPOCH: &str = "conary-current-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCompatibility {
    Fresh,
    Current,
    RebuildRequired { observed: String },
}

/// Inspect a database without creating, validating, or otherwise mutating it.
pub fn inspect(path: impl AsRef<Path>) -> Result<SchemaCompatibility> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(SchemaCompatibility::Fresh);
    }

    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let current_version = get_schema_version(&conn)?;
    match get_schema_identity(&conn)? {
        Some((epoch, revision)) if epoch == SCHEMA_EPOCH && revision == SCHEMA_VERSION => {
            if current_version == SCHEMA_VERSION {
                Ok(SchemaCompatibility::Current)
            } else {
                Ok(SchemaCompatibility::RebuildRequired {
                    observed: format!(
                        "schema epoch {epoch} with inconsistent version {current_version}"
                    ),
                })
            }
        }
        Some((epoch, revision)) => Ok(SchemaCompatibility::RebuildRequired {
            observed: format!("schema epoch {epoch} revision {revision}"),
        }),
        None if database_is_fresh(&conn)? => Ok(SchemaCompatibility::Fresh),
        None if current_version == 0 => Ok(SchemaCompatibility::RebuildRequired {
            observed: "unversioned non-empty database".to_string(),
        }),
        None => Ok(SchemaCompatibility::RebuildRequired {
            observed: format!("retired migration-chain schema version {current_version}"),
        }),
    }
}

/// Return zero for a fresh database or the exact schema epoch stored on disk.
pub fn get_schema_version(conn: &Connection) -> Result<i32> {
    if !table_exists(conn, "schema_version")? {
        return Ok(0);
    }

    conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map(|version| version.unwrap_or(0))
    .map_err(Into::into)
}

/// Require the current schema epoch without creating or migrating any state.
pub fn require_current(conn: &Connection) -> Result<()> {
    let current_version = get_schema_version(conn)?;
    match get_schema_identity(conn)? {
        Some((epoch, revision)) if epoch == SCHEMA_EPOCH && revision == SCHEMA_VERSION => {
            if current_version == SCHEMA_VERSION {
                Ok(())
            } else {
                Err(rebuild_required(&format!(
                    "schema epoch {epoch} with inconsistent version {current_version}"
                )))
            }
        }
        Some((epoch, revision)) => Err(rebuild_required(&format!(
            "schema epoch {epoch} revision {revision}"
        ))),
        None if database_is_fresh(conn)? => Err(rebuild_required("fresh database")),
        None if current_version == 0 => Err(rebuild_required("unversioned non-empty")),
        None => Err(rebuild_required(&format!(
            "retired migration-chain schema version {current_version}"
        ))),
    }
}

/// Initialize a fresh database or validate that it already uses this epoch.
///
/// Any prior schema is rejected with an explicit rebuild requirement. This is
/// deliberate: carrying an untested compatibility chain would make old
/// structure, queue normalization, and retired workflow state authoritative.
pub fn ensure_current(conn: &Connection) -> Result<()> {
    super::generation_delta::configure_mutation_epoch(conn)?;
    let current_version = get_schema_version(conn)?;
    match get_schema_identity(conn)? {
        Some((epoch, revision)) if epoch == SCHEMA_EPOCH && revision == SCHEMA_VERSION => {
            if current_version != SCHEMA_VERSION {
                return Err(rebuild_required(&format!(
                    "schema epoch {epoch} with inconsistent version {current_version}"
                )));
            }
            info!("Schema is current at epoch {}", SCHEMA_VERSION);
            return Ok(());
        }
        Some((epoch, revision)) => {
            return Err(rebuild_required(&format!(
                "schema epoch {epoch} revision {revision}"
            )));
        }
        None if database_is_fresh(conn)? => {}
        None if current_version == 0 => return Err(rebuild_required("unversioned non-empty")),
        None => {
            return Err(rebuild_required(&format!(
                "retired migration-chain schema version {current_version}"
            )));
        }
    }

    let tx = conn.unchecked_transaction()?;
    current_schema::create_current_schema(&tx)?;
    tx.execute(
        "INSERT INTO schema_identity (epoch, revision) VALUES (?1, ?2)",
        params![SCHEMA_EPOCH, SCHEMA_VERSION],
    )?;
    tx.execute(
        "INSERT INTO schema_version (version) VALUES (?1)",
        params![SCHEMA_VERSION],
    )?;
    tx.commit()?;
    info!("Initialized current schema epoch {}", SCHEMA_VERSION);
    Ok(())
}

fn get_schema_identity(conn: &Connection) -> Result<Option<(String, i32)>> {
    if !table_exists(conn, "schema_identity")? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT epoch, revision FROM schema_identity LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_schema
            WHERE type = 'table' AND name = ?1
        )",
        [table],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn database_is_fresh(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT NOT EXISTS(
            SELECT 1
            FROM sqlite_schema
            WHERE name NOT LIKE 'sqlite_%'
        )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn rebuild_required(observed: &str) -> Error {
    Error::SchemaRebuildRequired {
        observed: observed.to_string(),
        supported_epoch: SCHEMA_EPOCH.to_string(),
        supported_revision: SCHEMA_VERSION,
    }
}

#[cfg(test)]
mod tests;
