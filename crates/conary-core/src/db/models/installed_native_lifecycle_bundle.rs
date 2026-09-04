// conary-core/src/db/models/installed_native_lifecycle_bundle.rs

//! Installed native lifecycle bundle persistence.

use crate::ccs::native_lifecycle::NativeLifecycleBundle;
use crate::ccs::native_transaction::{DebPackageState, NativePackageIdentity};
use anyhow::{Context, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledNativeLifecycleBundle {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub source_format: String,
    pub source_family: String,
    pub source_profile: Option<String>,
    pub source_release: Option<String>,
    pub source_arch: Option<String>,
    pub source_package: String,
    pub source_version: String,
    pub scriptlet_fidelity: String,
    pub evidence_digest: Option<String>,
    pub lifecycle_state: DebPackageState,
    pub pending_triggers: Vec<String>,
    pub awaited_packages: Vec<NativePackageIdentity>,
    pub bundle_toml: String,
    pub installed_changeset_id: Option<i64>,
    pub installed_at: Option<String>,
}

impl InstalledNativeLifecycleBundle {
    const COLUMNS: &'static str = "id, trove_id, source_format, source_family, source_profile, \
         source_release, source_arch, source_package, source_version, scriptlet_fidelity, \
         evidence_digest, lifecycle_state, pending_triggers_json, \
         awaited_packages_json, bundle_toml, installed_changeset_id, installed_at";

    pub fn new(
        trove_id: i64,
        installed_changeset_id: Option<i64>,
        bundle: &NativeLifecycleBundle,
    ) -> anyhow::Result<Self> {
        bundle
            .validate()
            .context("native lifecycle bundle validation failed")?;
        let bundle_toml = toml::to_string_pretty(bundle)
            .context("native lifecycle bundle TOML serialization failed")?;

        Ok(Self {
            id: None,
            trove_id,
            source_format: bundle.source_format.as_str().to_string(),
            source_family: bundle.source_family.clone(),
            source_profile: bundle.source_profile.clone(),
            source_release: bundle.source_release.clone(),
            source_arch: bundle.source_arch.clone(),
            source_package: bundle.source_package.clone(),
            source_version: bundle.source_version.clone(),
            scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
            evidence_digest: bundle.evidence_digest.clone(),
            lifecycle_state: DebPackageState::Installed,
            pending_triggers: Vec::new(),
            awaited_packages: Vec::new(),
            bundle_toml,
            installed_changeset_id,
            installed_at: None,
        })
    }

    pub fn insert_or_replace(&mut self, conn: &Connection) -> anyhow::Result<()> {
        self.bundle()
            .context("installed native lifecycle bundle cannot be persisted")?;

        conn.execute(
            "INSERT INTO installed_native_lifecycle_bundles (
                trove_id, source_format, source_family, source_profile, source_release,
                source_arch, source_package, source_version, scriptlet_fidelity,
                evidence_digest, lifecycle_state, pending_triggers_json,
                awaited_packages_json, bundle_toml, installed_changeset_id
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
             )
             ON CONFLICT(trove_id) DO UPDATE SET
                source_format = excluded.source_format,
                source_family = excluded.source_family,
                source_profile = excluded.source_profile,
                source_release = excluded.source_release,
                source_arch = excluded.source_arch,
                source_package = excluded.source_package,
                source_version = excluded.source_version,
                scriptlet_fidelity = excluded.scriptlet_fidelity,
                evidence_digest = excluded.evidence_digest,
                lifecycle_state = excluded.lifecycle_state,
                pending_triggers_json = excluded.pending_triggers_json,
                awaited_packages_json = excluded.awaited_packages_json,
                bundle_toml = excluded.bundle_toml,
                installed_changeset_id = excluded.installed_changeset_id,
                installed_at = CURRENT_TIMESTAMP",
            params![
                self.trove_id,
                &self.source_format,
                &self.source_family,
                &self.source_profile,
                &self.source_release,
                &self.source_arch,
                &self.source_package,
                &self.source_version,
                &self.scriptlet_fidelity,
                &self.evidence_digest,
                self.lifecycle_state.as_str(),
                serde_json::to_string(&self.pending_triggers)
                    .context("pending Debian trigger serialization failed")?,
                serde_json::to_string(&self.awaited_packages)
                    .context("awaited Debian package serialization failed")?,
                &self.bundle_toml,
                &self.installed_changeset_id,
            ],
        )?;

        if let Some(found) = Self::find_by_trove(conn, self.trove_id)? {
            self.id = found.id;
            self.installed_at = found.installed_at;
        }
        Ok(())
    }

    pub fn set_lifecycle_state(&mut self, lifecycle_state: DebPackageState) {
        self.lifecycle_state = lifecycle_state;
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> anyhow::Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM installed_native_lifecycle_bundles WHERE trove_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_row([trove_id], Self::from_row).optional()?)
    }

    pub fn find_all(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM installed_native_lifecycle_bundles ORDER BY trove_id",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn bundle(&self) -> anyhow::Result<NativeLifecycleBundle> {
        let bundle: NativeLifecycleBundle = toml::from_str(&self.bundle_toml)
            .context("native lifecycle bundle TOML parse failed")?;
        bundle
            .validate()
            .context("native lifecycle bundle validation failed")?;
        if self.evidence_digest != bundle.evidence_digest {
            bail!(
                "native lifecycle bundle evidence_digest mismatch: row {:?}, bundle {:?}",
                self.evidence_digest,
                bundle.evidence_digest
            );
        }
        Ok(bundle)
    }

    pub fn update_runtime_state_by_trove(
        conn: &Connection,
        trove_id: i64,
        lifecycle_state: DebPackageState,
        pending_triggers: &[String],
        awaited_packages: &[NativePackageIdentity],
    ) -> anyhow::Result<usize> {
        let pending_triggers_json = serde_json::to_string(pending_triggers)
            .context("pending Debian trigger serialization failed")?;
        let awaited_packages_json = serde_json::to_string(awaited_packages)
            .context("awaited Debian package serialization failed")?;
        Ok(conn.execute(
            "UPDATE installed_native_lifecycle_bundles
             SET lifecycle_state = ?1,
                 pending_triggers_json = ?2,
                 awaited_packages_json = ?3
             WHERE trove_id = ?4",
            params![
                lifecycle_state.as_str(),
                pending_triggers_json,
                awaited_packages_json,
                trove_id,
            ],
        )?)
    }

    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> anyhow::Result<usize> {
        Ok(conn.execute(
            "DELETE FROM installed_native_lifecycle_bundles WHERE trove_id = ?1",
            [trove_id],
        )?)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            source_format: row.get(2)?,
            source_family: row.get(3)?,
            source_profile: row.get(4)?,
            source_release: row.get(5)?,
            source_arch: row.get(6)?,
            source_package: row.get(7)?,
            source_version: row.get(8)?,
            scriptlet_fidelity: row.get(9)?,
            evidence_digest: row.get(10)?,
            lifecycle_state: parse_lifecycle_state(row.get::<_, String>(11)?, 11)?,
            pending_triggers: parse_json_list(row.get::<_, String>(12)?, 12)?,
            awaited_packages: parse_json_list(row.get::<_, String>(13)?, 13)?,
            bundle_toml: row.get(14)?,
            installed_changeset_id: row.get(15)?,
            installed_at: row.get(16)?,
        })
    }
}

fn parse_lifecycle_state(value: String, column: usize) -> rusqlite::Result<DebPackageState> {
    DebPackageState::parse(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, error.into())
    })
}

fn parse_json_list<T: serde::de::DeserializeOwned>(
    value: String,
    column: usize,
) -> rusqlite::Result<Vec<T>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests;
