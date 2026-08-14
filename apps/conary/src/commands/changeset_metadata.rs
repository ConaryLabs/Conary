// apps/conary/src/commands/changeset_metadata.rs

#[cfg(test)]
use super::FileSnapshot;
use super::MaterializedDirectorySnapshot;
use super::RollbackSystemAuthority;
use super::TroveSnapshot;
use anyhow::{Result, bail};
use conary_core::generation::root_manifest::SelectedRootSnapshot;
use serde::{Deserialize, Serialize};

pub(crate) const CHANGESET_METADATA_SCHEMA: &str = "conary.changeset.metadata.v7";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_system_authority: Option<RollbackSystemAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_materialized_directories: Option<Vec<MaterializedDirectorySnapshot>>,
    #[serde(default)]
    pub deferred_follow_up: Vec<DeferredFollowUp>,
    #[serde(default)]
    pub adoption_warnings: Vec<AdoptionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RollbackAuthority {
    pub removed_troves: Vec<TroveSnapshot>,
    pub selected_root: SelectedRootSnapshot,
    pub system: RollbackSystemAuthority,
    pub materialized_directories: Vec<MaterializedDirectorySnapshot>,
}

pub(crate) fn metadata_with_removed_troves(
    snapshots: Vec<TroveSnapshot>,
    rollback_materialized_directories: Vec<MaterializedDirectorySnapshot>,
    _rollback_root: SelectedRootSnapshot,
    rollback_system_authority: RollbackSystemAuthority,
) -> Result<String> {
    metadata_with_envelope_sections(
        snapshots,
        true,
        Some(rollback_system_authority),
        Some(rollback_materialized_directories),
        Vec::new(),
        Vec::new(),
    )
}

pub(crate) fn metadata_with_deferred_follow_up(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
) -> Result<String> {
    metadata_with_envelope_sections(snapshots, false, None, None, deferred_follow_up, Vec::new())
}

pub(crate) fn metadata_with_adoption_warnings(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
) -> Result<String> {
    metadata_with_envelope_sections(
        snapshots,
        false,
        None,
        None,
        deferred_follow_up,
        adoption_warnings,
    )
}

fn metadata_with_envelope_sections(
    snapshots: Vec<TroveSnapshot>,
    has_rollback_root: bool,
    rollback_system_authority: Option<RollbackSystemAuthority>,
    rollback_materialized_directories: Option<Vec<MaterializedDirectorySnapshot>>,
    deferred_follow_up: Vec<DeferredFollowUp>,
    adoption_warnings: Vec<AdoptionWarning>,
) -> Result<String> {
    validate_rollback_authority_binding(
        &snapshots,
        has_rollback_root,
        rollback_system_authority.as_ref(),
        rollback_materialized_directories.as_deref(),
    )?;
    serde_json::to_string(&ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: snapshots,
        rollback_system_authority,
        rollback_materialized_directories,
        deferred_follow_up,
        adoption_warnings,
    })
    .map_err(Into::into)
}

pub(crate) fn parse_rollback_snapshots(snapshot_json: &str) -> Result<Vec<TroveSnapshot>> {
    Ok(parse_changeset_metadata(Some(snapshot_json))?.removed_troves)
}

pub(crate) fn parse_rollback_authority(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    snapshot_json: &str,
) -> Result<Option<RollbackAuthority>> {
    let envelope = parse_changeset_metadata(Some(snapshot_json))?;
    let snapshot_id = conn.query_row(
        "SELECT rollback_selected_root_snapshot_id FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(
        match (
            snapshot_id,
            envelope.rollback_system_authority,
            envelope.rollback_materialized_directories,
        ) {
            (Some(snapshot_id), Some(system), Some(materialized_directories)) => {
                let selected_root = SelectedRootSnapshot::find(conn, snapshot_id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "changeset {changeset_id} references missing selected-root snapshot {snapshot_id}"
                )
            })?;
                Some(RollbackAuthority {
                    removed_troves: envelope.removed_troves,
                    selected_root,
                    system,
                    materialized_directories,
                })
            }
            (None, None, None) => None,
            _ => bail!(
                "changeset {changeset_id} rollback snapshot, materialized-directory, and native-system authority must be persisted together"
            ),
        },
    )
}

fn empty_changeset_metadata() -> ChangesetMetadataEnvelope {
    ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: Vec::new(),
        rollback_system_authority: None,
        rollback_materialized_directories: None,
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

    let envelope: ChangesetMetadataEnvelope = serde_json::from_value(value)?;
    validate_rollback_authority_binding(
        &envelope.removed_troves,
        envelope.rollback_system_authority.is_some(),
        envelope.rollback_system_authority.as_ref(),
        envelope.rollback_materialized_directories.as_deref(),
    )?;
    Ok(envelope)
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
        envelope.rollback_system_authority.is_some(),
        envelope.rollback_system_authority,
        envelope.rollback_materialized_directories,
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
        envelope.rollback_system_authority.is_some(),
        envelope.rollback_system_authority,
        envelope.rollback_materialized_directories,
        envelope.deferred_follow_up,
        envelope.adoption_warnings,
    )?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}

fn validate_rollback_authority_binding(
    snapshots: &[TroveSnapshot],
    has_rollback_root: bool,
    rollback_system_authority: Option<&RollbackSystemAuthority>,
    rollback_materialized_directories: Option<&[MaterializedDirectorySnapshot]>,
) -> Result<()> {
    if !snapshots.is_empty()
        && (!has_rollback_root
            || rollback_system_authority.is_none()
            || rollback_materialized_directories.is_none())
    {
        bail!(
            "changeset rollback trove snapshots require exact pre-mutation selected-root, materialized-directory, and native-system authority"
        );
    }
    if has_rollback_root != rollback_system_authority.is_some()
        || has_rollback_root != rollback_materialized_directories.is_some()
    {
        bail!(
            "changeset rollback root, materialized-directory, and native-system authority must be persisted together"
        );
    }
    if let Some(system) = rollback_system_authority {
        system.validate()?;
    }
    if let Some(directories) = rollback_materialized_directories {
        let mut paths = std::collections::BTreeSet::new();
        for directory in directories {
            directory.validate()?;
            if !paths.insert(directory.path.as_str()) {
                bail!(
                    "changeset rollback authority repeats materialized directory '{}'",
                    directory.path
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::generation::root_manifest::{
        CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest,
        MutableStateManifest,
    };
    use conary_core::payload::{
        PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };

    fn rollback_root(conn: &rusqlite::Connection) -> SelectedRootSnapshot {
        let mut root = PayloadNode::regular(0o755);
        root.kind = PayloadNodeKind::Directory;
        root.mode = libc::S_IFDIR | 0o755;
        let captured = CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: ResolvedPayloadNode::from_numeric_source(root).unwrap(),
                entries: Vec::new(),
            },
            state: MutableStateManifest::empty(),
        };
        SelectedRootSnapshot::capture(conn, &captured).unwrap()
    }

    fn snapshot(name: &str) -> TroveSnapshot {
        let mut snapshot = TroveSnapshot::test_package(
            name,
            "1.0.0",
            vec![FileSnapshot {
                path: "/usr/bin/fixture".to_string(),
                node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755))
                    .unwrap(),
                content: Some(PayloadContentAuthority {
                    sha256: "0".repeat(64),
                    size: 7,
                }),
                installed_at: "2026-01-01T00:00:00Z".to_string(),
                component: None,
            }],
        );
        snapshot.architecture = Some("x86_64".to_string());
        snapshot.install_source = conary_core::db::models::InstallSource::Repository;
        snapshot
    }

    #[test]
    fn parses_versioned_envelope_snapshots_and_deferred_follow_up() {
        let warning = DeferredFollowUp {
            kind: "state_snapshot".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some("conary system state create \"retry\"".to_string()),
        };
        let raw = metadata_with_envelope_sections(
            vec![snapshot("fixture")],
            true,
            Some(RollbackSystemAuthority::default()),
            Some(Vec::new()),
            vec![warning.clone()],
            Vec::new(),
        )
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
            "schema": "conary.changeset.metadata.v5",
            "removed_troves": [snapshot("fixture")],
        })
        .to_string();

        let err = parse_rollback_snapshots(&raw).unwrap_err().to_string();

        assert!(err.contains("Unsupported changeset metadata schema"));
        assert!(err.contains("conary.changeset.metadata.v5"));
    }

    #[test]
    fn rollback_authority_sections_are_required_as_one_exact_v7_contract() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let snapshot_id = rollback_root(&conn).id();
        conn.execute(
            "INSERT INTO changesets (
                 description, status, rollback_selected_root_snapshot_id
             ) VALUES ('fixture', 'applied', ?1)",
            [snapshot_id],
        )
        .unwrap();
        let changeset_id = conn.last_insert_rowid();
        let complete = metadata_with_removed_troves(
            Vec::new(),
            Vec::new(),
            SelectedRootSnapshot::find(&conn, snapshot_id)
                .unwrap()
                .unwrap(),
            RollbackSystemAuthority::default(),
        )
        .unwrap();
        let complete: serde_json::Value = serde_json::from_str(&complete).unwrap();

        for missing in [
            "rollback_system_authority",
            "rollback_materialized_directories",
        ] {
            let mut incomplete = complete.clone();
            incomplete.as_object_mut().unwrap().remove(missing);
            let error = parse_rollback_authority(&conn, changeset_id, &incomplete.to_string())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(
                    "rollback root, materialized-directory, and native-system authority must be persisted together"
                ),
                "{missing}: {error}"
            );
        }

        let snapshots_without_authority = serde_json::json!({
            "schema": CHANGESET_METADATA_SCHEMA,
            "removed_troves": [snapshot("fixture")],
            "deferred_follow_up": [],
            "adoption_warnings": [],
        });
        let error = parse_rollback_snapshots(&snapshots_without_authority.to_string())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "trove snapshots require exact pre-mutation selected-root, materialized-directory, and native-system authority"
            ),
            "{error}"
        );
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
        let rollback_root = rollback_root(&conn);
        let initial = metadata_with_removed_troves(
            vec![snapshot("fixture")],
            Vec::new(),
            rollback_root,
            RollbackSystemAuthority::default(),
        )
        .unwrap();
        conn.execute(
            "UPDATE changesets
             SET metadata = ?1, rollback_selected_root_snapshot_id = ?2
             WHERE id = ?3",
            rusqlite::params![initial, rollback_root.id(), changeset_id],
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
