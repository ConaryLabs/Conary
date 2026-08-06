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
mod tests {
    use crate::ccs::native_lifecycle::{
        ArchInstallMetadata, DebMaintainerMetadata, LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1,
        NativeInvocation, NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind,
        RpmTriggerMetadata, ScriptletFidelity, ScriptletSandboxRequirements, SourceFormat,
        TransactionOrder, VersionScheme,
    };
    use crate::ccs::native_transaction::{DebPackageState, NativePackageIdentity};
    use crate::db::models::{
        Changeset, ChangesetStatus, InstalledNativeLifecycleBundle, Trove, TroveType,
    };
    use crate::db::testing::create_test_db;
    use rusqlite::params;

    fn fixture_changeset(conn: &rusqlite::Connection) -> i64 {
        let mut changeset = Changeset::new("Install native lifecycle fixture".to_string());
        let id = changeset.insert(conn).expect("insert changeset");
        changeset
            .update_status(conn, ChangesetStatus::Applied)
            .expect("mark changeset applied");
        id
    }

    fn fixture_trove(conn: &rusqlite::Connection) -> i64 {
        let mut trove = Trove::new(
            "native-lifecycle-fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.insert(conn).expect("insert trove")
    }

    fn sha256_prefixed(body: &str) -> String {
        crate::hash::sha256_prefixed(body.as_bytes())
    }

    fn fixture_entry(id: &str, body: &str) -> NativeLifecycleEntry {
        NativeLifecycleEntry {
            id: id.to_string(),
            native_slot: "%post".to_string(),
            kind: NativeLifecycleEntryKind::Executable,
            phase: LifecyclePath::PostInstall,
            lifecycle_paths: vec!["install:last".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: sha256_prefixed(body),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: Vec::new(),
                after: vec!["payload".to_string()],
            },
            timeout_ms: 30_000,
            sandbox: Some(ScriptletSandboxRequirements {
                network: false,
                namespaces: vec!["mount".to_string()],
                seccomp_profile: Some("native-lifecycle/default".to_string()),
            }),
            capabilities: Vec::new(),
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            rpm_trigger: None::<RpmTriggerMetadata>,
            rpm_runtime: Some(crate::ccs::native_lifecycle::RpmRuntimeMetadata {
                program: crate::ccs::native_lifecycle::RpmProgram::External,
                body_transforms: Vec::new(),
                critical: false,
                criticality: crate::ccs::native_lifecycle::RpmCriticality::WarningOnly,
                raw_flags: 0,
                unknown_flags: 0,
                install_prefixes: Vec::new(),
                macro_context: Default::default(),
                header_context: Default::default(),
                package_rpm_version: None,
            }),
            rpm_sysusers: None,
            deb_maintainer: None::<DebMaintainerMetadata>,
            arch_install: None::<ArchInstallMetadata>,
            arch_hook: None,
            residual_lifecycle: None,
        }
    }

    fn fixture_bundle() -> NativeLifecycleBundle {
        NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: crate::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: SourceFormat::Rpm,
            source_family: "fedora".to_string(),
            source_profile: Some("fedora-44".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "native-lifecycle-fixture".to_string(),
            source_version: "1.0-1".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-test".to_string(),
            evidence_digest: Some(crate::hash::sha256_prefixed(b"fixture-evidence")),
            scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
            entries: vec![fixture_entry("rpm:%post", "systemctl daemon-reload\n")],
        }
    }

    fn insert_fixture(
        conn: &rusqlite::Connection,
    ) -> (
        i64,
        i64,
        NativeLifecycleBundle,
        InstalledNativeLifecycleBundle,
    ) {
        let changeset_id = fixture_changeset(conn);
        let trove_id = fixture_trove(conn);
        let bundle = fixture_bundle();
        let mut installed =
            InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), &bundle)
                .expect("build installed bundle");
        installed
            .insert_or_replace(conn)
            .expect("insert installed bundle");
        (trove_id, changeset_id, bundle, installed)
    }

    #[test]
    fn insert_and_find_by_trove_round_trips_scalars_and_bundle() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, changeset_id, bundle, installed) = insert_fixture(&conn);

        assert!(installed.id.is_some());
        let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .expect("find installed bundle")
            .expect("installed bundle row");

        assert_eq!(found.trove_id, trove_id);
        assert_eq!(found.installed_changeset_id, Some(changeset_id));
        assert_eq!(found.source_format, "rpm");
        assert_eq!(found.source_family, "fedora");
        assert_eq!(found.source_package, "native-lifecycle-fixture");
        assert_eq!(found.scriptlet_fidelity, "native-lifecycle");
        assert_eq!(found.evidence_digest, bundle.evidence_digest);
        assert!(found.installed_at.is_some());
        assert_eq!(found.bundle().expect("decode bundle"), bundle);
    }

    #[test]
    fn insert_or_replace_updates_existing_trove_row() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, mut bundle, _installed) = insert_fixture(&conn);
        bundle.evidence_digest = Some(crate::hash::sha256_prefixed(b"replacement-evidence"));
        bundle.entries[0].body = "echo replacement\n".to_string();
        bundle.entries[0].body_sha256 = sha256_prefixed(&bundle.entries[0].body);

        let mut installed = InstalledNativeLifecycleBundle::new(trove_id, None, &bundle)
            .expect("build replacement");
        installed
            .insert_or_replace(&conn)
            .expect("replace installed bundle");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM installed_native_lifecycle_bundles WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.installed_changeset_id, None);
        assert_eq!(found.bundle().unwrap(), bundle);
    }

    #[test]
    fn bundle_rejects_evidence_digest_mismatch() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        conn.execute(
            "UPDATE installed_native_lifecycle_bundles SET evidence_digest = ?1 WHERE trove_id = ?2",
            params![
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                trove_id
            ],
        )
        .unwrap();

        let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap();
        let error = found.bundle().expect_err("mismatched evidence must fail");

        assert!(error.to_string().contains("evidence_digest mismatch"));
    }

    #[test]
    fn malformed_bundle_toml_is_loaded_but_bundle_returns_error() {
        let (_tmp, conn) = create_test_db();
        let changeset_id = fixture_changeset(&conn);
        let trove_id = fixture_trove(&conn);

        conn.execute(
            "INSERT INTO installed_native_lifecycle_bundles (
                trove_id, source_format, source_family, source_package, source_version,
                scriptlet_fidelity, evidence_digest, bundle_toml,
                installed_changeset_id
             ) VALUES (?1, 'rpm', 'fedora', 'native-lifecycle-fixture', '1.0-1',
                'native-lifecycle', NULL, ?2, ?3)",
            params![trove_id, "not = [valid toml", changeset_id],
        )
        .unwrap();

        let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap();
        let error = found.bundle().expect_err("malformed TOML must fail");

        assert!(error.to_string().contains("native lifecycle bundle TOML"));
    }

    #[test]
    fn debian_runtime_state_round_trips_by_exact_trove_identity() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        let updated = InstalledNativeLifecycleBundle::update_runtime_state_by_trove(
            &conn,
            trove_id,
            DebPackageState::TriggersAwaited,
            &["ldconfig".to_string(), "update-mime".to_string()],
            &[NativePackageIdentity {
                package_name: "consumer-a".to_string(),
                package_version: "1".to_string(),
                package_arch: Some("x86_64".to_string()),
            }],
        )
        .expect("update runtime state");
        assert_eq!(updated, 1);

        let found = InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .expect("read runtime state")
            .expect("installed bundle");
        assert_eq!(found.lifecycle_state, DebPackageState::TriggersAwaited);
        assert_eq!(found.pending_triggers, ["ldconfig", "update-mime"]);
        assert_eq!(
            found.awaited_packages,
            [NativePackageIdentity {
                package_name: "consumer-a".to_string(),
                package_version: "1".to_string(),
                package_arch: Some("x86_64".to_string()),
            }]
        );
    }

    #[test]
    fn malformed_runtime_state_json_is_rejected_during_row_materialization() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);
        conn.execute(
            "UPDATE installed_native_lifecycle_bundles
             SET pending_triggers_json = '{\"not\":\"a-list\"}'
             WHERE trove_id = ?1",
            [trove_id],
        )
        .unwrap();

        InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
            .expect_err("malformed trigger state must fail closed");
    }

    #[test]
    fn deleting_trove_cascades_installed_bundle_row() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        Trove::delete(&conn, trove_id).expect("delete trove");

        assert!(
            InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn delete_by_trove_removes_installed_bundle_row() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        let deleted = InstalledNativeLifecycleBundle::delete_by_trove(&conn, trove_id)
            .expect("delete installed bundle");

        assert_eq!(deleted, 1);
        assert!(
            InstalledNativeLifecycleBundle::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_none()
        );
    }
}
