// conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs

//! Installed legacy scriptlet bundle persistence for safe replay.

use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use anyhow::{Context, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledLegacyScriptletBundle {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub source_format: String,
    pub source_family: String,
    pub source_distro: Option<String>,
    pub source_release: Option<String>,
    pub source_arch: Option<String>,
    pub source_package: String,
    pub source_version: String,
    pub target_id: String,
    pub target_compatibility: String,
    pub foreign_replay_policy: String,
    pub scriptlet_fidelity: String,
    pub publication_status: String,
    pub evidence_digest: Option<String>,
    pub replay_policy: String,
    pub replay_enabled: bool,
    pub bundle_toml: String,
    pub installed_changeset_id: Option<i64>,
    pub installed_at: Option<String>,
}

impl InstalledLegacyScriptletBundle {
    const COLUMNS: &'static str = "id, trove_id, source_format, source_family, source_distro, \
         source_release, source_arch, source_package, source_version, target_id, \
         target_compatibility, foreign_replay_policy, scriptlet_fidelity, publication_status, \
         evidence_digest, replay_policy, replay_enabled, bundle_toml, installed_changeset_id, \
         installed_at";

    pub fn new(
        trove_id: i64,
        installed_changeset_id: Option<i64>,
        target_id: String,
        replay_policy: String,
        replay_enabled: bool,
        bundle: &LegacyScriptletBundle,
    ) -> anyhow::Result<Self> {
        bundle
            .validate()
            .context("legacy scriptlet bundle validation failed")?;
        let bundle_toml = toml::to_string_pretty(bundle)
            .context("legacy scriptlet bundle TOML serialization failed")?;

        Ok(Self {
            id: None,
            trove_id,
            source_format: bundle.source_format.as_str().to_string(),
            source_family: bundle.source_family.clone(),
            source_distro: bundle.source_distro.clone(),
            source_release: bundle.source_release.clone(),
            source_arch: bundle.source_arch.clone(),
            source_package: bundle.source_package.clone(),
            source_version: bundle.source_version.clone(),
            target_id,
            target_compatibility: bundle.target_compatibility.as_str().to_string(),
            foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
            scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
            publication_status: bundle.publication_status.as_str().to_string(),
            evidence_digest: bundle.evidence_digest.clone(),
            replay_policy,
            replay_enabled,
            bundle_toml,
            installed_changeset_id,
            installed_at: None,
        })
    }

    pub fn insert_or_replace(&mut self, conn: &Connection) -> anyhow::Result<()> {
        self.bundle()
            .context("installed legacy scriptlet bundle cannot be persisted")?;

        conn.execute(
            "INSERT INTO installed_legacy_scriptlet_bundles (
                trove_id, source_format, source_family, source_distro, source_release,
                source_arch, source_package, source_version, target_id, target_compatibility,
                foreign_replay_policy, scriptlet_fidelity, publication_status, evidence_digest,
                replay_policy, replay_enabled, bundle_toml, installed_changeset_id
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
             )
             ON CONFLICT(trove_id) DO UPDATE SET
                source_format = excluded.source_format,
                source_family = excluded.source_family,
                source_distro = excluded.source_distro,
                source_release = excluded.source_release,
                source_arch = excluded.source_arch,
                source_package = excluded.source_package,
                source_version = excluded.source_version,
                target_id = excluded.target_id,
                target_compatibility = excluded.target_compatibility,
                foreign_replay_policy = excluded.foreign_replay_policy,
                scriptlet_fidelity = excluded.scriptlet_fidelity,
                publication_status = excluded.publication_status,
                evidence_digest = excluded.evidence_digest,
                replay_policy = excluded.replay_policy,
                replay_enabled = excluded.replay_enabled,
                bundle_toml = excluded.bundle_toml,
                installed_changeset_id = excluded.installed_changeset_id,
                installed_at = CURRENT_TIMESTAMP",
            params![
                self.trove_id,
                &self.source_format,
                &self.source_family,
                &self.source_distro,
                &self.source_release,
                &self.source_arch,
                &self.source_package,
                &self.source_version,
                &self.target_id,
                &self.target_compatibility,
                &self.foreign_replay_policy,
                &self.scriptlet_fidelity,
                &self.publication_status,
                &self.evidence_digest,
                &self.replay_policy,
                self.replay_enabled,
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

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> anyhow::Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM installed_legacy_scriptlet_bundles WHERE trove_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_row([trove_id], Self::from_row).optional()?)
    }

    pub fn bundle(&self) -> anyhow::Result<LegacyScriptletBundle> {
        let bundle: LegacyScriptletBundle = toml::from_str(&self.bundle_toml)
            .context("legacy scriptlet bundle TOML parse failed")?;
        bundle
            .validate()
            .context("legacy scriptlet bundle validation failed")?;
        if self.evidence_digest != bundle.evidence_digest {
            bail!(
                "legacy scriptlet bundle evidence_digest mismatch: row {:?}, bundle {:?}",
                self.evidence_digest,
                bundle.evidence_digest
            );
        }
        Ok(bundle)
    }

    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> anyhow::Result<usize> {
        Ok(conn.execute(
            "DELETE FROM installed_legacy_scriptlet_bundles WHERE trove_id = ?1",
            [trove_id],
        )?)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            source_format: row.get(2)?,
            source_family: row.get(3)?,
            source_distro: row.get(4)?,
            source_release: row.get(5)?,
            source_arch: row.get(6)?,
            source_package: row.get(7)?,
            source_version: row.get(8)?,
            target_id: row.get(9)?,
            target_compatibility: row.get(10)?,
            foreign_replay_policy: row.get(11)?,
            scriptlet_fidelity: row.get(12)?,
            publication_status: row.get(13)?,
            evidence_digest: row.get(14)?,
            replay_policy: row.get(15)?,
            replay_enabled: row.get::<_, i64>(16)? != 0,
            bundle_toml: row.get(17)?,
            installed_changeset_id: row.get(18)?,
            installed_at: row.get(19)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::ccs::legacy_scriptlets::{
        ArchInstallMetadata, DebMaintainerMetadata, DecisionCounts, EffectConfidence,
        EffectReplacement, EffectSource, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1,
        LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath, NativeInvocation,
        PublicationPolicy, PublicationStatus, RpmTriggerMetadata, ScriptletDecision,
        ScriptletEffect, ScriptletFidelity, ScriptletSandboxRequirements, SourceFormat,
        TargetCompatibility, TransactionOrder, VersionScheme,
    };
    use crate::db::models::{
        Changeset, ChangesetStatus, InstalledLegacyScriptletBundle, Trove, TroveType,
    };
    use crate::db::testing::create_test_db;
    use rusqlite::params;
    use std::collections::BTreeMap;

    fn fixture_changeset(conn: &rusqlite::Connection) -> i64 {
        let mut changeset = Changeset::new("Install legacy fixture".to_string());
        let id = changeset.insert(conn).expect("insert changeset");
        changeset
            .update_status(conn, ChangesetStatus::Applied)
            .expect("mark changeset applied");
        id
    }

    fn fixture_trove(conn: &rusqlite::Connection) -> i64 {
        let mut trove = Trove::new(
            "legacy-fixture".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.insert(conn).expect("insert trove")
    }

    fn sha256_prefixed(body: &str) -> String {
        crate::hash::sha256_prefixed(body.as_bytes())
    }

    fn fixture_effect() -> ScriptletEffect {
        ScriptletEffect {
            kind: "daemon-reload".to_string(),
            source: EffectSource::StaticSignal,
            confidence: EffectConfidence::Declared,
            replacement: EffectReplacement::None,
            adapter_id: None,
            adapter_digest: None,
            command: Some("systemctl".to_string()),
            args: vec!["daemon-reload".to_string()],
            path: None,
            reason_code: Some("systemd-daemon-reload".to_string()),
            extra: BTreeMap::new(),
        }
    }

    fn fixture_entry(id: &str, body: &str) -> LegacyScriptletEntry {
        LegacyScriptletEntry {
            id: id.to_string(),
            native_slot: "%post".to_string(),
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
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: Some(ScriptletSandboxRequirements {
                network: false,
                namespaces: vec!["mount".to_string()],
                seccomp_profile: Some("legacy-scriptlet/default".to_string()),
                extra: BTreeMap::new(),
            }),
            capabilities: Vec::new(),
            decision: ScriptletDecision::Legacy,
            reason_code: "legacy-replay-required".to_string(),
            human_reason: Some("fixture legacy entry".to_string()),
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            effects: vec![fixture_effect()],
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            rpm_trigger: None::<RpmTriggerMetadata>,
            deb_maintainer: None::<DebMaintainerMetadata>,
            arch_install: None::<ArchInstallMetadata>,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn fixture_bundle() -> LegacyScriptletBundle {
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "legacy-fixture".to_string(),
            source_version: "1.0-1".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-test".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(crate::hash::sha256_prefixed(b"fixture-evidence")),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::LocalOnly,
            publication_status: PublicationStatus::LocalOnly,
            scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
            decision_counts: DecisionCounts {
                replaced: 0,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![fixture_entry("rpm:%post", "systemctl daemon-reload\n")],
            extra: BTreeMap::new(),
        }
    }

    fn insert_fixture(
        conn: &rusqlite::Connection,
    ) -> (
        i64,
        i64,
        LegacyScriptletBundle,
        InstalledLegacyScriptletBundle,
    ) {
        let changeset_id = fixture_changeset(conn);
        let trove_id = fixture_trove(conn);
        let bundle = fixture_bundle();
        let mut installed = InstalledLegacyScriptletBundle::new(
            trove_id,
            Some(changeset_id),
            "rpm/fedora/44/x86_64".to_string(),
            "allow-legacy-replay".to_string(),
            true,
            &bundle,
        )
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
        let found = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .expect("find installed bundle")
            .expect("installed bundle row");

        assert_eq!(found.trove_id, trove_id);
        assert_eq!(found.installed_changeset_id, Some(changeset_id));
        assert_eq!(found.source_format, "rpm");
        assert_eq!(found.source_family, "fedora");
        assert_eq!(found.source_package, "legacy-fixture");
        assert_eq!(found.target_id, "rpm/fedora/44/x86_64");
        assert_eq!(found.target_compatibility, "source-native");
        assert_eq!(found.foreign_replay_policy, "deny");
        assert_eq!(found.scriptlet_fidelity, "legacy-replay");
        assert_eq!(found.publication_status, "local-only");
        assert_eq!(found.evidence_digest, bundle.evidence_digest);
        assert_eq!(found.replay_policy, "allow-legacy-replay");
        assert!(found.replay_enabled);
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

        let mut installed = InstalledLegacyScriptletBundle::new(
            trove_id,
            None,
            "rpm/fedora/44/x86_64".to_string(),
            "replacement-policy".to_string(),
            false,
            &bundle,
        )
        .expect("build replacement");
        installed
            .insert_or_replace(&conn)
            .expect("replace installed bundle");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM installed_legacy_scriptlet_bundles WHERE trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let found = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.installed_changeset_id, None);
        assert_eq!(found.replay_policy, "replacement-policy");
        assert!(!found.replay_enabled);
        assert_eq!(found.bundle().unwrap(), bundle);
    }

    #[test]
    fn bundle_rejects_evidence_digest_mismatch() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        conn.execute(
            "UPDATE installed_legacy_scriptlet_bundles SET evidence_digest = ?1 WHERE trove_id = ?2",
            params![
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                trove_id
            ],
        )
        .unwrap();

        let found = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
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
            "INSERT INTO installed_legacy_scriptlet_bundles (
                trove_id, source_format, source_family, source_package, source_version,
                target_id, target_compatibility, foreign_replay_policy, scriptlet_fidelity,
                publication_status, evidence_digest, replay_policy, replay_enabled,
                bundle_toml, installed_changeset_id
             ) VALUES (?1, 'rpm', 'fedora', 'legacy-fixture', '1.0-1',
                'rpm/fedora/44/x86_64', 'source-native', 'deny', 'legacy-replay',
                'local-only', NULL, 'allow-legacy-replay', 1, ?2, ?3)",
            params![trove_id, "not = [valid toml", changeset_id],
        )
        .unwrap();

        let found = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap();
        let error = found.bundle().expect_err("malformed TOML must fail");

        assert!(error.to_string().contains("legacy scriptlet bundle TOML"));
    }

    #[test]
    fn bundle_extra_fields_survive_database_round_trip() {
        let (_tmp, conn) = create_test_db();
        let changeset_id = fixture_changeset(&conn);
        let trove_id = fixture_trove(&conn);
        let mut bundle = fixture_bundle();
        bundle.extra.insert(
            "future_bundle_field".to_string(),
            toml::Value::String("kept".to_string()),
        );
        bundle.entries[0].extra.insert(
            "future_entry_field".to_string(),
            toml::Value::String("also-kept".to_string()),
        );

        let mut installed = InstalledLegacyScriptletBundle::new(
            trove_id,
            Some(changeset_id),
            "rpm/fedora/44/x86_64".to_string(),
            "allow-legacy-replay".to_string(),
            true,
            &bundle,
        )
        .unwrap();
        installed.insert_or_replace(&conn).unwrap();

        let decoded = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
            .unwrap()
            .unwrap()
            .bundle()
            .unwrap();
        assert_eq!(
            decoded.extra.get("future_bundle_field"),
            Some(&toml::Value::String("kept".to_string()))
        );
        assert_eq!(
            decoded.entries[0].extra.get("future_entry_field"),
            Some(&toml::Value::String("also-kept".to_string()))
        );
    }

    #[test]
    fn deleting_trove_cascades_installed_bundle_row() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        Trove::delete(&conn, trove_id).expect("delete trove");

        assert!(
            InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn delete_by_trove_removes_installed_bundle_row() {
        let (_tmp, conn) = create_test_db();
        let (trove_id, _changeset_id, _bundle, _installed) = insert_fixture(&conn);

        let deleted = InstalledLegacyScriptletBundle::delete_by_trove(&conn, trove_id)
            .expect("delete installed bundle");

        assert_eq!(deleted, 1);
        assert!(
            InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_none()
        );
    }
}
