// apps/conary/src/commands/rollback_system_authority/runtime_projections.rs

//! Exact rollback authority for normalized runtime fields owned by installed rows.

use crate::commands::installed_authority_snapshot::StableTroveIdentitySnapshot;
use anyhow::{Result, bail};
use conary_core::ccs::native_transaction::{DebPackageState, NativePackageIdentity};
use conary_core::db::models::{ConfigStatus, InstalledNativeLifecycleBundle, Trove};
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstalledNativeRuntimeSnapshot {
    pub trove: StableTroveIdentitySnapshot,
    pub lifecycle_state: String,
    pub pending_triggers: Vec<String>,
    pub awaited_packages: Vec<NativePackageIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigRuntimeSnapshot {
    pub path: String,
    pub current_hash: Option<String>,
    pub status: ConfigStatus,
    pub modified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DerivedRuntimeSnapshot {
    pub name: String,
    pub status: String,
    pub updated_at: String,
}

pub(super) struct RuntimeProjections {
    pub installed_native: Vec<InstalledNativeRuntimeSnapshot>,
    pub config: Vec<ConfigRuntimeSnapshot>,
    pub derived: Vec<DerivedRuntimeSnapshot>,
}

impl RuntimeProjections {
    pub(super) fn capture(conn: &Connection) -> Result<Self> {
        Ok(Self {
            installed_native: capture_installed_native_runtime(conn)?,
            config: capture_config_runtime(conn)?,
            derived: capture_derived_runtime(conn)?,
        })
    }
}

pub(super) fn validate(
    installed_native: &[InstalledNativeRuntimeSnapshot],
    config: &[ConfigRuntimeSnapshot],
    derived: &[DerivedRuntimeSnapshot],
) -> Result<()> {
    unique_keys(
        "installed native runtime trove",
        installed_native.iter().map(|row| {
            (
                row.trove.name.as_str(),
                row.trove.version.as_str(),
                row.trove.trove_type.as_str(),
                row.trove.architecture.as_deref(),
                row.trove.installed_at.as_str(),
                row.trove.version_scheme.as_str(),
            )
        }),
    )?;
    for runtime in installed_native {
        runtime.trove.validate()?;
        DebPackageState::parse(&runtime.lifecycle_state)?;
        for awaited in &runtime.awaited_packages {
            require_text("awaited native package name", &awaited.package_name)?;
            require_text("awaited native package version", &awaited.package_version)?;
        }
    }

    unique_keys(
        "config runtime path",
        config.iter().map(|row| row.path.as_str()),
    )?;
    for config in config {
        require_text("config runtime path", &config.path)?;
        if let Some(hash) = config.current_hash.as_deref() {
            require_canonical_sha256("config runtime current_hash", hash)?;
        }
    }

    unique_keys(
        "derived runtime package",
        derived.iter().map(|row| row.name.as_str()),
    )?;
    for derived in derived {
        require_text("derived runtime package", &derived.name)?;
        if !matches!(
            derived.status.as_str(),
            "pending" | "built" | "stale" | "error"
        ) {
            bail!(
                "rollback system authority has unknown derived status '{}'",
                derived.status
            );
        }
        require_text("derived runtime updated_at", &derived.updated_at)?;
    }
    Ok(())
}

pub(super) fn restore(
    tx: &Transaction<'_>,
    installed_native: &[InstalledNativeRuntimeSnapshot],
    config: &[ConfigRuntimeSnapshot],
    derived: &[DerivedRuntimeSnapshot],
) -> Result<()> {
    for row in installed_native {
        let trove_id = resolve_stable_trove_id(tx, &row.trove)?;
        let changed = tx.execute(
            "UPDATE installed_native_lifecycle_bundles
             SET lifecycle_state = ?1,
                 pending_triggers_json = ?2,
                 awaited_packages_json = ?3
             WHERE trove_id = ?4",
            params![
                &row.lifecycle_state,
                serde_json::to_string(&row.pending_triggers)?,
                serde_json::to_string(&row.awaited_packages)?,
                trove_id,
            ],
        )?;
        require_exact_update(
            changed,
            &format!(
                "installed native runtime for {}-{}",
                row.trove.name, row.trove.version
            ),
        )?;
    }
    for row in config {
        let changed = tx.execute(
            "UPDATE config_files
             SET current_hash = ?1, status = ?2, modified_at = ?3
             WHERE path = ?4",
            params![
                &row.current_hash,
                row.status.as_str(),
                &row.modified_at,
                &row.path
            ],
        )?;
        require_exact_update(changed, &format!("config runtime for {}", row.path))?;
    }
    for row in derived {
        let changed = tx.execute(
            "UPDATE derived_packages SET status = ?1, updated_at = ?2 WHERE name = ?3",
            params![&row.status, &row.updated_at, &row.name],
        )?;
        require_exact_update(changed, &format!("derived runtime for {}", row.name))?;
    }
    Ok(())
}

fn capture_installed_native_runtime(
    conn: &Connection,
) -> Result<Vec<InstalledNativeRuntimeSnapshot>> {
    InstalledNativeLifecycleBundle::find_all(conn)?
        .into_iter()
        .map(|runtime| {
            Ok(InstalledNativeRuntimeSnapshot {
                trove: stable_trove_identity(conn, runtime.trove_id)?,
                lifecycle_state: runtime.lifecycle_state.as_str().to_string(),
                pending_triggers: runtime.pending_triggers,
                awaited_packages: runtime.awaited_packages,
            })
        })
        .collect()
}

fn capture_config_runtime(conn: &Connection) -> Result<Vec<ConfigRuntimeSnapshot>> {
    let rows = query_rows(
        conn,
        "SELECT path, current_hash, status, modified_at
         FROM config_files ORDER BY path",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    rows.into_iter()
        .map(|(path, current_hash, status, modified_at)| {
            let status = ConfigStatus::parse(&status)
                .ok_or_else(|| anyhow::anyhow!("unknown config runtime status '{status}'"))?;
            Ok(ConfigRuntimeSnapshot {
                path,
                current_hash,
                status,
                modified_at,
            })
        })
        .collect()
}

fn capture_derived_runtime(conn: &Connection) -> Result<Vec<DerivedRuntimeSnapshot>> {
    query_rows(
        conn,
        "SELECT name, status, updated_at FROM derived_packages ORDER BY name",
        |row| {
            Ok(DerivedRuntimeSnapshot {
                name: row.get(0)?,
                status: row.get(1)?,
                updated_at: row.get(2)?,
            })
        },
    )
}

fn stable_trove_identity(conn: &Connection, trove_id: i64) -> Result<StableTroveIdentitySnapshot> {
    let trove = Trove::find_by_id(conn, trove_id)?
        .ok_or_else(|| anyhow::anyhow!("installed runtime references missing trove {trove_id}"))?;
    let installed_at = trove.installed_at.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "installed runtime trove '{}-{}' has no installed_at authority",
            trove.name,
            trove.version
        )
    })?;
    Ok(StableTroveIdentitySnapshot {
        name: trove.name,
        version: trove.version,
        trove_type: trove.trove_type,
        architecture: trove.architecture,
        installed_at,
        version_scheme: trove.version_scheme,
    })
}

fn resolve_stable_trove_id(
    conn: &Connection,
    identity: &StableTroveIdentitySnapshot,
) -> Result<i64> {
    let mut candidates = Trove::find_by_name(conn, &identity.name)?
        .into_iter()
        .filter(|trove| {
            trove.version == identity.version
                && trove.trove_type == identity.trove_type
                && trove.architecture == identity.architecture
                && trove.installed_at.as_deref() == Some(identity.installed_at.as_str())
                && trove.version_scheme == identity.version_scheme
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        bail!(
            "installed runtime trove identity '{}-{}' resolved to {} rows",
            identity.name,
            identity.version,
            candidates.len()
        );
    }
    candidates
        .pop()
        .and_then(|trove| trove.id)
        .ok_or_else(|| anyhow::anyhow!("resolved installed runtime trove has no database identity"))
}

fn require_exact_update(changed: usize, context: &str) -> Result<()> {
    if changed != 1 {
        bail!("rollback system authority could not restore {context}: updated {changed} rows");
    }
    Ok(())
}

fn query_rows<T>(
    conn: &Connection,
    sql: &str,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    Ok(conn
        .prepare(sql)?
        .query_map([], map)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn require_text(context: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("rollback system authority has empty {context}");
    }
    Ok(())
}

fn require_canonical_sha256(context: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{context} must be a lowercase raw 64-hex SHA-256 digest");
    }
    Ok(())
}

fn unique_keys<T: Ord>(context: &str, values: impl IntoIterator<Item = T>) -> Result<BTreeSet<T>> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            bail!("rollback system authority repeats {context}");
        }
    }
    Ok(seen)
}
