// apps/conary/src/commands/changeset_metadata.rs

#[cfg(test)]
use super::FileSnapshot;
use super::{RevertMetadata, TroveSnapshot};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) const CHANGESET_METADATA_SCHEMA: &str = "conary.changeset.metadata.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeferredFollowUp {
    pub kind: String,
    pub status: String,
    pub message: String,
    pub retry_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredFollowUpKind {
    GenerationPublication,
    LegacyGenerationRebuild,
    Other,
}

pub(crate) fn classify_deferred_follow_up_kind(
    follow_up: &DeferredFollowUp,
) -> DeferredFollowUpKind {
    match follow_up.kind.as_str() {
        "generation_publication" => DeferredFollowUpKind::GenerationPublication,
        "generation_rebuild" => DeferredFollowUpKind::LegacyGenerationRebuild,
        _ => DeferredFollowUpKind::Other,
    }
}

pub(crate) fn publication_deferred_follow_up(message: String) -> DeferredFollowUp {
    DeferredFollowUp {
        kind: "generation_publication".to_string(),
        status: "pending".to_string(),
        message,
        retry_command: Some(
            "conary --allow-live-system-mutation system generation publish".to_string(),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdoptionWarning {
    pub package: String,
    pub reason: String,
    pub total_inserts: usize,
    pub failed_inserts: usize,
}

impl AdoptionWarning {
    pub(crate) fn partial_insert_failure(
        package: impl Into<String>,
        total_inserts: usize,
        failed_inserts: usize,
    ) -> Self {
        Self {
            package: package.into(),
            reason: "partial_metadata_insert_failure".to_string(),
            total_inserts,
            failed_inserts,
        }
    }

    pub(crate) fn all_insert_failure(package: impl Into<String>, total_inserts: usize) -> Self {
        Self {
            package: package.into(),
            reason: "all_metadata_inserts_failed".to_string(),
            total_inserts,
            failed_inserts: total_inserts,
        }
    }

    pub(crate) fn refresh_replacement_failure(
        package: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            package: package.into(),
            reason: format!("refresh_replacement_failed: {}", message.into()),
            total_inserts: 0,
            failed_inserts: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScriptletWarning {
    pub kind: String,
    pub phase: String,
    pub package: String,
    pub failure_kind: String,
    pub requested_sandbox_mode: String,
    pub effective_sandbox: String,
    pub message: String,
}

impl ScriptletWarning {
    pub(crate) fn new(
        phase: impl Into<String>,
        package: impl Into<String>,
        failure_kind: impl Into<String>,
        requested_sandbox_mode: impl Into<String>,
        effective_sandbox: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: "scriptlet_warning".to_string(),
            phase: phase.into(),
            package: package.into(),
            failure_kind: failure_kind.into(),
            requested_sandbox_mode: requested_sandbox_mode.into(),
            effective_sandbox: effective_sandbox.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChangesetMetadataEnvelope {
    pub schema: String,
    #[serde(default)]
    pub removed_troves: Vec<TroveSnapshot>,
    #[serde(default)]
    pub deferred_follow_up: Vec<DeferredFollowUp>,
    #[serde(default)]
    pub adoption_warnings: Vec<AdoptionWarning>,
    #[serde(default)]
    pub scriptlet_warnings: Vec<ScriptletWarning>,
}

pub(crate) fn metadata_with_removed_troves(snapshots: Vec<TroveSnapshot>) -> Result<String> {
    metadata_with_full_envelope(snapshots, Vec::new(), Vec::new(), Vec::new())
}

pub(crate) fn metadata_with_deferred_follow_up(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
) -> Result<String> {
    metadata_with_full_envelope(snapshots, deferred_follow_up, Vec::new(), Vec::new())
}

pub(crate) fn metadata_with_adoption_warnings(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
) -> Result<String> {
    metadata_with_full_envelope(snapshots, deferred_follow_up, adoption_warnings, Vec::new())
}

pub(crate) fn metadata_with_full_envelope(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
    scriptlet_warnings: Vec<ScriptletWarning>,
) -> Result<String> {
    serde_json::to_string(&ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: snapshots,
        deferred_follow_up,
        adoption_warnings,
        scriptlet_warnings,
    })
    .map_err(Into::into)
}

pub(crate) fn parse_rollback_snapshots(snapshot_json: &str) -> Result<Vec<TroveSnapshot>> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(snapshot_json)
        && let Some(schema_value) = value.get("schema")
    {
        let Some(schema) = schema_value.as_str() else {
            bail!(
                "Unsupported changeset metadata schema: non-string schema; expected {CHANGESET_METADATA_SCHEMA}"
            );
        };
        if schema != CHANGESET_METADATA_SCHEMA {
            bail!(
                "Unsupported changeset metadata schema {schema}; expected {CHANGESET_METADATA_SCHEMA}"
            );
        }

        let envelope: ChangesetMetadataEnvelope = serde_json::from_value(value)?;
        return Ok(envelope.removed_troves);
    }
    if let Ok(wrapper) = serde_json::from_str::<RevertMetadata>(snapshot_json) {
        return Ok(wrapper.removed_troves);
    }
    Ok(vec![serde_json::from_str(snapshot_json)?])
}

pub(crate) fn deferred_follow_up(snapshot_json: Option<&str>) -> Vec<DeferredFollowUp> {
    snapshot_json
        .and_then(|raw| serde_json::from_str::<ChangesetMetadataEnvelope>(raw).ok())
        .filter(|envelope| envelope.schema == CHANGESET_METADATA_SCHEMA)
        .map(|envelope| envelope.deferred_follow_up)
        .unwrap_or_default()
}

pub(crate) fn adoption_warnings(snapshot_json: Option<&str>) -> Vec<AdoptionWarning> {
    snapshot_json
        .and_then(|raw| serde_json::from_str::<ChangesetMetadataEnvelope>(raw).ok())
        .filter(|envelope| envelope.schema == CHANGESET_METADATA_SCHEMA)
        .map(|envelope| envelope.adoption_warnings)
        .unwrap_or_default()
}

pub(crate) fn scriptlet_warnings(snapshot_json: Option<&str>) -> Vec<ScriptletWarning> {
    snapshot_json
        .and_then(|raw| serde_json::from_str::<ChangesetMetadataEnvelope>(raw).ok())
        .filter(|envelope| envelope.schema == CHANGESET_METADATA_SCHEMA)
        .map(|envelope| envelope.scriptlet_warnings)
        .unwrap_or_default()
}

pub(crate) fn append_deferred_follow_up_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    follow_up: DeferredFollowUp,
) -> Result<()> {
    let existing: Option<String> = conn.query_row(
        "SELECT metadata FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;
    let mut removed_troves = existing
        .as_deref()
        .map(parse_rollback_snapshots)
        .transpose()?
        .unwrap_or_default();
    let mut deferred = deferred_follow_up(existing.as_deref());
    deferred.push(follow_up);
    let metadata = metadata_with_full_envelope(
        std::mem::take(&mut removed_troves),
        deferred,
        adoption_warnings(existing.as_deref()),
        scriptlet_warnings(existing.as_deref()),
    )?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}

pub(crate) fn append_adoption_warning_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    warnings: Vec<AdoptionWarning>,
) -> Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }

    let existing: Option<String> = conn.query_row(
        "SELECT metadata FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;
    let removed_troves = existing
        .as_deref()
        .map(parse_rollback_snapshots)
        .transpose()?
        .unwrap_or_default();
    let deferred = deferred_follow_up(existing.as_deref());
    let mut existing_warnings = adoption_warnings(existing.as_deref());
    existing_warnings.extend(warnings);
    let metadata = metadata_with_full_envelope(
        removed_troves,
        deferred,
        existing_warnings,
        scriptlet_warnings(existing.as_deref()),
    )?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}

pub(crate) fn append_scriptlet_warning_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    warnings: Vec<ScriptletWarning>,
) -> Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }

    let existing: Option<String> = conn.query_row(
        "SELECT metadata FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;
    let removed_troves = existing
        .as_deref()
        .map(parse_rollback_snapshots)
        .transpose()?
        .unwrap_or_default();
    let deferred = deferred_follow_up(existing.as_deref());
    let adoption = adoption_warnings(existing.as_deref());
    let mut existing_warnings = scriptlet_warnings(existing.as_deref());
    existing_warnings.extend(warnings);
    let metadata =
        metadata_with_full_envelope(removed_troves, deferred, adoption, existing_warnings)?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(name: &str) -> TroveSnapshot {
        TroveSnapshot {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: None,
            install_source: "repository".to_string(),
            installed_from_repository_id: None,
            files: vec![FileSnapshot {
                path: "/usr/bin/fixture".to_string(),
                sha256_hash: "0".repeat(64),
                size: 7,
                permissions: 0o100755,
                symlink_target: None,
            }],
        }
    }

    #[test]
    fn parses_legacy_single_trove_snapshot() {
        let raw = serde_json::to_string(&snapshot("fixture")).unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "fixture");
    }

    #[test]
    fn parses_legacy_revert_metadata_wrapper() {
        let raw = serde_json::to_string(&RevertMetadata {
            removed_troves: vec![snapshot("one"), snapshot("two")],
        })
        .unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();

        assert_eq!(
            parsed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["one", "two"]
        );
    }

    #[test]
    fn parses_versioned_envelope_snapshots_and_deferred_follow_up() {
        let warning = DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some(
                "conary --allow-live-system-mutation system generation build --summary \"Retry deferred package follow-up\""
                    .to_string(),
            ),
        };
        let raw =
            metadata_with_deferred_follow_up(vec![snapshot("fixture")], vec![warning.clone()])
                .unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();
        let deferred = deferred_follow_up(Some(&raw));

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "fixture");
        assert_eq!(deferred, vec![warning]);
    }

    #[test]
    fn publication_deferred_follow_up_uses_publish_retry() {
        let follow_up = publication_deferred_follow_up("forced".to_string());
        assert_eq!(follow_up.kind, "generation_publication");
        assert_eq!(follow_up.status, "pending");
        assert_eq!(
            follow_up.retry_command.as_deref(),
            Some("conary --allow-live-system-mutation system generation publish")
        );
    }

    #[test]
    fn classify_legacy_generation_rebuild_follow_up() {
        let follow_up = DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "old failure".to_string(),
            retry_command: Some("conary system generation build --summary retry".to_string()),
        };
        assert_eq!(
            classify_deferred_follow_up_kind(&follow_up),
            DeferredFollowUpKind::LegacyGenerationRebuild
        );
    }

    #[test]
    fn rejects_unknown_schema_without_legacy_fallback() {
        let raw = serde_json::json!({
            "schema": "conary.changeset.metadata.v2",
            "removed_troves": [snapshot("fixture")],
        })
        .to_string();

        let err = parse_rollback_snapshots(&raw).unwrap_err().to_string();

        assert!(err.contains("Unsupported changeset metadata schema"));
        assert!(err.contains("conary.changeset.metadata.v2"));
    }

    #[test]
    fn malformed_or_legacy_metadata_has_no_deferred_follow_up() {
        let raw = serde_json::to_string(&snapshot("fixture")).unwrap();

        assert!(deferred_follow_up(Some(&raw)).is_empty());
        assert!(deferred_follow_up(Some("not-json")).is_empty());
        assert!(deferred_follow_up(None).is_empty());
    }

    #[test]
    fn append_deferred_follow_up_preserves_removed_troves() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut changeset = conary_core::db::models::Changeset::new("Remove fixture".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        let initial = metadata_with_removed_troves(vec![snapshot("fixture")]).unwrap();
        conn.execute(
            "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
            rusqlite::params![initial, changeset_id],
        )
        .unwrap();

        append_deferred_follow_up_metadata(
            &conn,
            changeset_id,
            DeferredFollowUp {
                kind: "state_snapshot".to_string(),
                status: "failed".to_string(),
                message: "snapshot failed".to_string(),
                retry_command: Some("conary system state create \"Remove fixture\"".to_string()),
            },
        )
        .unwrap();

        let raw: String = conn
            .query_row(
                "SELECT metadata FROM changesets WHERE id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parse_rollback_snapshots(&raw).unwrap()[0].name, "fixture");
        assert_eq!(deferred_follow_up(Some(&raw)).len(), 1);
    }

    #[test]
    fn scriptlet_warning_metadata_preserves_existing_envelope_sections() {
        let warning = ScriptletWarning {
            kind: "scriptlet_warning".to_string(),
            phase: "post-install".to_string(),
            package: "fixture".to_string(),
            failure_kind: "ScriptExited".to_string(),
            requested_sandbox_mode: "auto".to_string(),
            effective_sandbox: "direct".to_string(),
            message: "post-install scriptlet failed after package files were installed".to_string(),
        };
        let deferred = DeferredFollowUp {
            kind: "generation_publication".to_string(),
            status: "pending".to_string(),
            message: "publication pending".to_string(),
            retry_command: None,
        };
        let raw = metadata_with_full_envelope(
            vec![snapshot("fixture")],
            vec![deferred.clone()],
            vec![AdoptionWarning::partial_insert_failure("fixture", 2, 1)],
            vec![warning.clone()],
        )
        .unwrap();

        assert_eq!(parse_rollback_snapshots(&raw).unwrap()[0].name, "fixture");
        assert_eq!(deferred_follow_up(Some(&raw)), vec![deferred]);
        assert_eq!(scriptlet_warnings(Some(&raw)), vec![warning]);
        assert_eq!(adoption_warnings(Some(&raw)).len(), 1);
    }
}
