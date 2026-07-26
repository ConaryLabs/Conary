// conary-core/src/db/models/native_lifecycle_residual_state.rs

//! Native lifecycle state retained after the installed trove disappears.

use crate::ccs::native_lifecycle::NativeLifecycleBundle;
use crate::ccs::native_transaction::{DebPackageState, NativePackageIdentity};
use anyhow::{Context, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLifecycleResidualState {
    pub identity_digest: String,
    pub source_format: String,
    pub source_package: String,
    pub source_version: String,
    pub source_arch: Option<String>,
    pub lifecycle_state: DebPackageState,
    pub pending_triggers: Vec<String>,
    pub awaited_packages: Vec<NativePackageIdentity>,
    pub bundle_toml: String,
    pub updated_at: String,
}

impl NativeLifecycleResidualState {
    const COLUMNS: &'static str = "identity_digest, source_format, source_package, \
        source_version, source_arch, lifecycle_state, pending_triggers_json, \
        awaited_packages_json, bundle_toml, updated_at";

    pub fn new(
        bundle: &NativeLifecycleBundle,
        lifecycle_state: DebPackageState,
        pending_triggers: Vec<String>,
        awaited_packages: Vec<NativePackageIdentity>,
    ) -> anyhow::Result<Self> {
        bundle
            .validate()
            .context("residual native lifecycle bundle validation failed")?;
        let identity_digest = Self::identity_digest(
            bundle.source_format.as_str(),
            &bundle.source_package,
            &bundle.source_version,
            bundle.source_arch.as_deref(),
        )?;
        Ok(Self {
            identity_digest,
            source_format: bundle.source_format.as_str().to_string(),
            source_package: bundle.source_package.clone(),
            source_version: bundle.source_version.clone(),
            source_arch: bundle.source_arch.clone(),
            lifecycle_state,
            pending_triggers,
            awaited_packages,
            bundle_toml: toml::to_string_pretty(bundle)
                .context("serialize residual native lifecycle bundle")?,
            updated_at: String::new(),
        })
    }

    pub fn identity(&self) -> NativePackageIdentity {
        NativePackageIdentity {
            package_name: self.source_package.clone(),
            package_version: self.source_version.clone(),
            package_arch: self.source_arch.clone(),
        }
    }

    pub fn bundle(&self) -> anyhow::Result<NativeLifecycleBundle> {
        let bundle: NativeLifecycleBundle =
            toml::from_str(&self.bundle_toml).context("decode residual native lifecycle bundle")?;
        bundle
            .validate()
            .context("residual native lifecycle bundle validation failed")?;
        if bundle.source_format.as_str() != self.source_format
            || bundle.source_package != self.source_package
            || bundle.source_version != self.source_version
            || bundle.source_arch != self.source_arch
        {
            bail!("residual native lifecycle bundle identity fields do not match its row");
        }
        let digest = Self::identity_digest(
            bundle.source_format.as_str(),
            &bundle.source_package,
            &bundle.source_version,
            bundle.source_arch.as_deref(),
        )?;
        if digest != self.identity_digest {
            bail!("residual native lifecycle bundle identity does not match its row");
        }
        Ok(bundle)
    }

    pub fn find_all(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM native_lifecycle_residual_states ORDER BY \
             source_format, source_package, source_version, source_arch",
            Self::COLUMNS
        );
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map([], Self::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_exact(
        conn: &Connection,
        source_format: &str,
        identity: &NativePackageIdentity,
    ) -> anyhow::Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM native_lifecycle_residual_states
             WHERE source_format = ?1 AND source_package = ?2
               AND source_version = ?3 AND source_arch IS ?4",
            Self::COLUMNS
        );
        Ok(conn
            .query_row(
                &sql,
                params![
                    source_format,
                    &identity.package_name,
                    &identity.package_version,
                    &identity.package_arch,
                ],
                Self::from_row,
            )
            .optional()?)
    }

    pub fn upsert(&mut self, conn: &Connection) -> anyhow::Result<()> {
        self.bundle()?;
        conn.execute(
            "INSERT INTO native_lifecycle_residual_states (
                identity_digest, source_format, source_package, source_version,
                source_arch, lifecycle_state, pending_triggers_json,
                awaited_packages_json, bundle_toml
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(identity_digest) DO UPDATE SET
                lifecycle_state = excluded.lifecycle_state,
                pending_triggers_json = excluded.pending_triggers_json,
                awaited_packages_json = excluded.awaited_packages_json,
                bundle_toml = excluded.bundle_toml,
                updated_at = CURRENT_TIMESTAMP",
            params![
                &self.identity_digest,
                &self.source_format,
                &self.source_package,
                &self.source_version,
                &self.source_arch,
                self.lifecycle_state.as_str(),
                serde_json::to_string(&self.pending_triggers)
                    .context("serialize residual pending triggers")?,
                serde_json::to_string(&self.awaited_packages)
                    .context("serialize residual awaited packages")?,
                &self.bundle_toml,
            ],
        )?;
        if let Some(found) = Self::find_exact(conn, &self.source_format, &self.identity())? {
            self.updated_at = found.updated_at;
        }
        Ok(())
    }

    pub fn delete_exact(
        conn: &Connection,
        source_format: &str,
        identity: &NativePackageIdentity,
    ) -> anyhow::Result<usize> {
        Ok(conn.execute(
            "DELETE FROM native_lifecycle_residual_states
             WHERE source_format = ?1 AND source_package = ?2
               AND source_version = ?3 AND source_arch IS ?4",
            params![
                source_format,
                &identity.package_name,
                &identity.package_version,
                &identity.package_arch,
            ],
        )?)
    }

    fn identity_digest(
        source_format: &str,
        source_package: &str,
        source_version: &str,
        source_arch: Option<&str>,
    ) -> anyhow::Result<String> {
        let identity =
            serde_json::to_vec(&(source_format, source_package, source_version, source_arch))
                .context("serialize residual native lifecycle identity")?;
        Ok(crate::hash::sha256_prefixed(&identity))
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            identity_digest: row.get(0)?,
            source_format: row.get(1)?,
            source_package: row.get(2)?,
            source_version: row.get(3)?,
            source_arch: row.get(4)?,
            lifecycle_state: DebPackageState::parse(&row.get::<_, String>(5)?).map_err(
                |error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                },
            )?,
            pending_triggers: parse_json(row.get::<_, String>(6)?, 6)?,
            awaited_packages: parse_json(row.get::<_, String>(7)?, 7)?,
            bundle_toml: row.get(8)?,
            updated_at: row.get(9)?,
        })
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(value: String, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::native_lifecycle::{
        NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, ScriptletFidelity,
        SourceFormat, VersionScheme,
    };
    use crate::db::testing::create_test_db;

    fn deb_bundle() -> NativeLifecycleBundle {
        NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: SourceFormat::Deb,
            source_family: "debian".to_string(),
            source_profile: Some("ubuntu-26.04".to_string()),
            source_release: Some("13".to_string()),
            source_arch: Some("amd64".to_string()),
            source_package: "fixture".to_string(),
            source_version: "1.0-1".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Deb,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "1".to_string(),
            conversion_policy: "test".to_string(),
            evidence_digest: None,
            scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
            entries: Vec::new(),
        }
    }

    #[test]
    fn residual_state_round_trips_typed_multiarch_await_edges() {
        let (_temp, conn) = create_test_db();
        let awaited = NativePackageIdentity::new("cache-owner", "2", Some("arm64"));
        let mut state = NativeLifecycleResidualState::new(
            &deb_bundle(),
            DebPackageState::ConfigFiles,
            vec!["refresh-cache".to_string()],
            vec![awaited.clone()],
        )
        .unwrap();
        state.upsert(&conn).unwrap();

        let found = NativeLifecycleResidualState::find_exact(
            &conn,
            "deb",
            &NativePackageIdentity::new("fixture", "1.0-1", Some("amd64")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.lifecycle_state, DebPackageState::ConfigFiles);
        assert_eq!(found.pending_triggers, ["refresh-cache"]);
        assert_eq!(found.awaited_packages, [awaited]);
        assert_eq!(found.bundle().unwrap(), deb_bundle());
    }

    #[test]
    fn residual_lookup_does_not_cross_architectures() {
        let (_temp, conn) = create_test_db();
        let mut state = NativeLifecycleResidualState::new(
            &deb_bundle(),
            DebPackageState::ConfigFiles,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        state.upsert(&conn).unwrap();

        assert!(
            NativeLifecycleResidualState::find_exact(
                &conn,
                "deb",
                &NativePackageIdentity::new("fixture", "1.0-1", Some("arm64")),
            )
            .unwrap()
            .is_none()
        );
    }
}
