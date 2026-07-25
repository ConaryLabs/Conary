// apps/conary/src/commands/changeset_metadata.rs

#[cfg(test)]
use super::FileSnapshot;
use super::TroveSnapshot;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) const CHANGESET_METADATA_SCHEMA: &str = "conary.changeset.metadata.v4";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeferredFollowUp {
    pub kind: String,
    pub status: String,
    pub message: String,
    pub retry_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredFollowUpKind {
    GenerationPublication,
    Other,
}

pub(crate) fn classify_deferred_follow_up_kind(
    follow_up: &DeferredFollowUp,
) -> DeferredFollowUpKind {
    match follow_up.kind.as_str() {
        "generation_publication" => DeferredFollowUpKind::GenerationPublication,
        _ => DeferredFollowUpKind::Other,
    }
}

pub(crate) fn publication_deferred_follow_up(message: String) -> DeferredFollowUp {
    DeferredFollowUp {
        kind: "generation_publication".to_string(),
        status: "pending".to_string(),
        message,
        retry_command: Some("conary system generation publish --yes".to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub(crate) struct ChangesetMetadataEnvelope {
    pub schema: String,
    #[serde(default)]
    pub removed_troves: Vec<TroveSnapshot>,
    #[serde(default)]
    pub deferred_follow_up: Vec<DeferredFollowUp>,
    #[serde(default)]
    pub adoption_warnings: Vec<AdoptionWarning>,
}

pub(crate) fn metadata_with_removed_troves(snapshots: Vec<TroveSnapshot>) -> Result<String> {
    metadata_with_envelope_sections(snapshots, Vec::new(), Vec::new())
}

pub(crate) fn metadata_with_deferred_follow_up(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
) -> Result<String> {
    metadata_with_envelope_sections(snapshots, deferred_follow_up, Vec::new())
}

pub(crate) fn metadata_with_adoption_warnings(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
) -> Result<String> {
    metadata_with_envelope_sections(snapshots, deferred_follow_up, adoption_warnings)
}

fn metadata_with_envelope_sections(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
) -> Result<String> {
    serde_json::to_string(&ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: snapshots,
        deferred_follow_up,
        adoption_warnings,
    })
    .map_err(Into::into)
}

pub(crate) fn parse_rollback_snapshots(snapshot_json: &str) -> Result<Vec<TroveSnapshot>> {
    Ok(parse_changeset_metadata(Some(snapshot_json))?.removed_troves)
}

fn empty_changeset_metadata() -> ChangesetMetadataEnvelope {
    ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: Vec::new(),
        deferred_follow_up: Vec::new(),
        adoption_warnings: Vec::new(),
    }
}

fn parse_changeset_metadata(snapshot_json: Option<&str>) -> Result<ChangesetMetadataEnvelope> {
    let Some(snapshot_json) = snapshot_json else {
        return Ok(empty_changeset_metadata());
    };
    let value = serde_json::from_str::<serde_json::Value>(snapshot_json)?;
    let Some(schema) = value.get("schema").and_then(serde_json::Value::as_str) else {
        bail!(
            "Unsupported changeset metadata: missing string schema; expected {CHANGESET_METADATA_SCHEMA}"
        );
    };
    if schema != CHANGESET_METADATA_SCHEMA {
        bail!(
            "Unsupported changeset metadata schema {schema}; expected {CHANGESET_METADATA_SCHEMA}"
        );
    }

    serde_json::from_value(value).map_err(Into::into)
}

pub(crate) fn deferred_follow_up(snapshot_json: Option<&str>) -> Result<Vec<DeferredFollowUp>> {
    Ok(parse_changeset_metadata(snapshot_json)?.deferred_follow_up)
}

pub(crate) fn adoption_warnings(snapshot_json: Option<&str>) -> Result<Vec<AdoptionWarning>> {
    Ok(parse_changeset_metadata(snapshot_json)?.adoption_warnings)
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
    let mut envelope = parse_changeset_metadata(existing.as_deref())?;
    envelope.deferred_follow_up.push(follow_up);
    let metadata = metadata_with_envelope_sections(
        envelope.removed_troves,
        envelope.deferred_follow_up,
        envelope.adoption_warnings,
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
    let mut envelope = parse_changeset_metadata(existing.as_deref())?;
    envelope.adoption_warnings.extend(warnings);
    let metadata = metadata_with_envelope_sections(
        envelope.removed_troves,
        envelope.deferred_follow_up,
        envelope.adoption_warnings,
    )?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};

    fn snapshot(name: &str) -> TroveSnapshot {
        TroveSnapshot {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: None,
            install_source: "repository".to_string(),
            source_distro: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
            native_lifecycle: None,
            ccs_remove_hook: None,
            installed_from_repository_id: None,
            files: vec![FileSnapshot {
                path: "/usr/bin/fixture".to_string(),
                node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755))
                    .unwrap(),
                content: Some(PayloadContentAuthority {
                    sha256: "0".repeat(64),
                    size: 7,
                }),
            }],
        }
    }

    #[test]
    fn parses_versioned_envelope_snapshots_and_deferred_follow_up() {
        let warning = DeferredFollowUp {
            kind: "state_snapshot".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some("conary system state create \"retry\"".to_string()),
        };
        let raw =
            metadata_with_deferred_follow_up(vec![snapshot("fixture")], vec![warning.clone()])
                .unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();
        let deferred = deferred_follow_up(Some(&raw)).unwrap();

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
            Some("conary system generation publish --yes")
        );
    }

    #[test]
    fn rejects_superseded_schema_without_fallback() {
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
    fn unversioned_or_malformed_metadata_is_rejected() {
        let raw = serde_json::to_string(&snapshot("fixture")).unwrap();

        assert!(parse_rollback_snapshots(&raw).is_err());
        assert!(deferred_follow_up(Some(&raw)).is_err());
        assert!(deferred_follow_up(Some("not-json")).is_err());
        assert!(deferred_follow_up(None).unwrap().is_empty());
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
        assert_eq!(deferred_follow_up(Some(&raw)).unwrap().len(), 1);
    }

    #[test]
    fn append_rejects_corrupt_metadata_without_overwriting_it() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut changeset = conary_core::db::models::Changeset::new("Corrupt fixture".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        conn.execute(
            "UPDATE changesets SET metadata = 'not-json' WHERE id = ?1",
            [changeset_id],
        )
        .unwrap();

        let error = append_deferred_follow_up_metadata(
            &conn,
            changeset_id,
            publication_deferred_follow_up("pending".to_string()),
        )
        .expect_err("corrupt persisted metadata must stop the append");
        assert!(error.to_string().contains("expected ident"));

        let raw: String = conn
            .query_row(
                "SELECT metadata FROM changesets WHERE id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, "not-json");
    }

    #[test]
    fn unknown_envelope_fields_are_rejected() {
        let raw = serde_json::json!({
            "schema": CHANGESET_METADATA_SCHEMA,
            "removed_troves": [],
            "deferred_follow_up": [],
            "adoption_warnings": [],
            "invented": true,
        })
        .to_string();

        let error = deferred_follow_up(Some(&raw))
            .expect_err("unknown current-schema fields must not be ignored");
        assert!(error.to_string().contains("unknown field `invented`"));
    }
}
