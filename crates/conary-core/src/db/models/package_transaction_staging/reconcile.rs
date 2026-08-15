// conary-core/src/db/models/package_transaction_staging/reconcile.rs

//! Set-based projection from transaction staging into canonical package state.

use super::{PackageTransactionStaging, StagedPayloadOutcome};
use crate::error::{Error, Result};
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

impl PackageTransactionStaging<'_> {
    pub(super) fn reconcile_components(&mut self) -> Result<()> {
        self.tx.execute(
            "INSERT INTO components (parent_trove_id, name, description, is_installed)
             SELECT trove_id, name, description, is_installed
             FROM conary_tx_components
             ORDER BY trove_id, name
             ON CONFLICT(parent_trove_id, name) DO UPDATE SET
                description = excluded.description,
                is_installed = excluded.is_installed",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn reconcile_contents(&mut self) -> Result<()> {
        self.tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             SELECT DISTINCT content_sha256,
                    'objects/' || substr(content_sha256, 1, 2) || '/' || substr(content_sha256, 3),
                    content_size
             FROM conary_tx_payload
             WHERE content_sha256 IS NOT NULL
             ORDER BY content_sha256",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn reconcile_anchors(&mut self) -> Result<()> {
        self.tx.execute(
            "DELETE FROM payload_claims
             WHERE path IN (
                 SELECT path FROM conary_tx_decisions WHERE anchor_action = 'replace'
             )
             AND (
                 trove_id IN (SELECT trove_id FROM conary_tx_replacement_owners)
                 OR trove_id IN (
                     SELECT t.id FROM troves t
                     JOIN conary_tx_payload p ON p.package_name = t.name
                     WHERE p.path = payload_claims.path
                 )
                 OR trove_id IN (
                     SELECT id FROM troves WHERE install_source = 'captured-root'
                 )
             )",
            [],
        )?;
        self.count_reconciliation_statement();

        self.tx.execute(
            "UPDATE files
             SET payload_node_json = (
                     SELECT p.payload_node_json
                     FROM conary_tx_payload p
                     JOIN conary_tx_decisions d USING(path, trove_id)
                     WHERE p.path = files.path
                       AND d.anchor_action IN ('reconcile-materialization', 'reconcile-owned')
                 ),
                 content_sha256 = (
                     SELECT p.content_sha256
                     FROM conary_tx_payload p
                     JOIN conary_tx_decisions d USING(path, trove_id)
                     WHERE p.path = files.path
                       AND d.anchor_action IN ('reconcile-materialization', 'reconcile-owned')
                 ),
                 content_size = (
                     SELECT p.content_size
                     FROM conary_tx_payload p
                     JOIN conary_tx_decisions d USING(path, trove_id)
                     WHERE p.path = files.path
                       AND d.anchor_action IN ('reconcile-materialization', 'reconcile-owned')
                 )
             WHERE path IN (
                 SELECT path FROM conary_tx_decisions
                 WHERE anchor_action IN ('reconcile-materialization', 'reconcile-owned')
             )",
            [],
        )?;
        self.count_reconciliation_statement();

        self.tx.execute(
            "INSERT INTO files (
                path, payload_node_json, content_sha256, content_size,
                trove_id, component_id
             )
             SELECT p.path,
                    CASE d.anchor_action
                        WHEN 'insert-selected-root' THEN p.selected_root_node_json
                        ELSE p.payload_node_json
                    END,
                    CASE d.anchor_action
                        WHEN 'insert-selected-root' THEN NULL
                        ELSE p.content_sha256
                    END,
                    CASE d.anchor_action
                        WHEN 'insert-selected-root' THEN NULL
                        ELSE p.content_size
                    END,
                    p.trove_id,
                    c.id
             FROM conary_tx_payload p
             JOIN conary_tx_decisions d USING(path, trove_id)
             LEFT JOIN components c
               ON c.parent_trove_id = p.trove_id AND c.name = p.component_name
             WHERE d.anchor_action IN ('replace', 'apply-directory', 'insert-selected-root')
             ORDER BY p.path, p.trove_id
             ON CONFLICT(path) DO UPDATE SET
                payload_node_json = excluded.payload_node_json,
                content_sha256 = excluded.content_sha256,
                content_size = excluded.content_size,
                trove_id = excluded.trove_id,
                component_id = excluded.component_id",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn reconcile_claims(&mut self) -> Result<()> {
        self.tx.execute(
            "INSERT INTO payload_claims (
                path, trove_id, component_id, sharing_policy, anchor_policy,
                materialization_target_path, payload_node_json,
                content_sha256, content_size
             )
             SELECT p.path, p.trove_id, c.id, p.sharing_policy, p.anchor_policy,
                    p.materialization_target_path, p.payload_node_json,
                    p.content_sha256, p.content_size
             FROM conary_tx_payload p
             JOIN conary_tx_decisions d USING(path, trove_id)
             LEFT JOIN components c
               ON c.parent_trove_id = p.trove_id AND c.name = p.component_name
             WHERE d.anchor_action != 'reconcile-materialization'
             ORDER BY p.path, p.trove_id
             ON CONFLICT(path, trove_id) DO UPDATE SET
                component_id = excluded.component_id,
                sharing_policy = excluded.sharing_policy,
                anchor_policy = excluded.anchor_policy,
                materialization_target_path = excluded.materialization_target_path,
                payload_node_json = excluded.payload_node_json,
                content_sha256 = excluded.content_sha256,
                content_size = excluded.content_size",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn reconcile_hardlinks(&mut self) -> Result<()> {
        let missing_target = self
            .tx
            .query_row(
                "SELECT p.path, json_extract(p.payload_node_json, '$.source.kind.target')
             FROM conary_tx_payload p
             LEFT JOIN files target
               ON target.path = json_extract(p.payload_node_json, '$.source.kind.target')
             WHERE json_extract(p.payload_node_json, '$.source.kind.type') = 'hardlink'
               AND p.disposition != 'reconcile-captured-root'
               AND (
                   target.path IS NULL
                   OR json_extract(target.payload_node_json, '$.source.kind.type') != 'regular'
               )
             ORDER BY p.path LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        self.count_validation_query();
        if let Some((path, target)) = missing_target {
            return Err(Error::ConflictError(format!(
                "staged hardlink {path} has no regular materialization target {target}"
            )));
        }

        self.tx.execute(
            "UPDATE files
             SET payload_node_json = json_set(
                 payload_node_json,
                 '$.source.kind.hardlink_identity',
                 'path:' || path
             )
             WHERE path IN (
                 SELECT DISTINCT json_extract(payload_node_json, '$.source.kind.target')
                 FROM conary_tx_payload
                 WHERE json_extract(payload_node_json, '$.source.kind.type') = 'hardlink'
                   AND disposition != 'reconcile-captured-root'
             )",
            [],
        )?;
        self.count_reconciliation_statement();
        self.tx.execute(
            "UPDATE files
             SET payload_node_json = json_set(
                 payload_node_json,
                 '$.source.kind.identity',
                 'path:' || json_extract(payload_node_json, '$.source.kind.target')
             )
             WHERE json_extract(payload_node_json, '$.source.kind.type') = 'hardlink'
               AND json_extract(payload_node_json, '$.source.kind.target') IN (
                   SELECT DISTINCT json_extract(payload_node_json, '$.source.kind.target')
                   FROM conary_tx_payload
                   WHERE json_extract(payload_node_json, '$.source.kind.type') = 'hardlink'
                     AND disposition != 'reconcile-captured-root'
               )",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn reconcile_history(&mut self) -> Result<()> {
        self.tx.execute(
            "INSERT INTO file_history (changeset_id, path, sha256_hash, action)
             SELECT p.history_changeset_id, p.path, p.content_sha256, p.history_action
             FROM conary_tx_payload p
             JOIN conary_tx_decisions d USING(path, trove_id)
             WHERE p.history_changeset_id IS NOT NULL AND d.materialized = 1
             ORDER BY p.path, p.trove_id",
            [],
        )?;
        self.count_reconciliation_statement();
        self.tx.execute(
            "INSERT INTO file_history (changeset_id, path, sha256_hash, action)
             SELECT h.changeset_id, h.path,
                    CASE WHEN content.sha256_hash IS NULL THEN NULL ELSE h.sha256_hash END,
                    h.action
             FROM conary_tx_history h
             LEFT JOIN file_contents content ON content.sha256_hash = h.sha256_hash
             ORDER BY h.path, h.action",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn reconcile_configs(&mut self) -> Result<()> {
        self.tx.execute(
            "INSERT INTO config_files (
                file_id, path, trove_id, package_name, package_version,
                package_architecture, original_hash, original_md5, current_hash,
                noreplace, ghost, materialized, remove_on_upgrade, status,
                modified_at, source
             )
             SELECT CASE WHEN s.materialized = 1 THEN f.id ELSE NULL END,
                    s.path, s.trove_id, t.name, t.version, t.architecture,
                    s.original_hash, s.original_md5, s.current_hash,
                    s.noreplace, s.ghost, s.materialized, s.remove_on_upgrade,
                    s.status, s.modified_at, s.source
             FROM conary_tx_configs s
             JOIN troves t ON t.id = s.trove_id
             LEFT JOIN files f ON f.path = s.path
             ORDER BY s.path
             ON CONFLICT(path) DO UPDATE SET
                file_id = excluded.file_id,
                trove_id = excluded.trove_id,
                package_name = excluded.package_name,
                package_version = excluded.package_version,
                package_architecture = excluded.package_architecture,
                original_hash = excluded.original_hash,
                original_md5 = excluded.original_md5,
                current_hash = excluded.current_hash,
                noreplace = excluded.noreplace,
                ghost = excluded.ghost,
                materialized = excluded.materialized,
                remove_on_upgrade = excluded.remove_on_upgrade,
                status = excluded.status,
                modified_at = excluded.modified_at,
                source = excluded.source",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    pub(super) fn load_outcomes(&mut self) -> Result<BTreeMap<String, StagedPayloadOutcome>> {
        let mut statement = self.tx.prepare(
            "SELECT p.path, f.id, f.content_sha256, d.materialized
             FROM conary_tx_payload p
             JOIN conary_tx_decisions d USING(path, trove_id)
             JOIN files f ON f.path = p.path
             ORDER BY p.path, p.trove_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    StagedPayloadOutcome {
                        file_id: row.get(1)?,
                        content_sha256: row.get(2)?,
                        materialized: row.get(3)?,
                    },
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        self.count_validation_query();
        Ok(rows.into_iter().collect())
    }
}
