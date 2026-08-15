// conary-core/src/db/models/package_transaction_staging.rs

//! Transaction-local package row staging and set-based reconciliation.

use super::{ConfigFile, ExistingDirectoryMaterialization, FileEntry, PayloadClaim};
use crate::error::{Error, Result};
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

use self::planning::{
    PlannedDecision, StagedAnchorInput, anchor_policy, parse_anchor_policy, parse_content,
    parse_disposition, parse_node, persisted_content_size, validate_staged_payload,
};
use self::sql::{CLEAR_STAGING_SQL, DROP_STAGING_SQL, STAGING_SCHEMA_SQL};

mod planning;
mod sql;

/// Typed materialization action established before canonical package rows change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedAnchorDisposition {
    /// Derive insert/share/directory behavior from exact canonical claims.
    Auto,
    /// Replace an absent or explicitly superseded materialization anchor.
    Replace,
    /// Join an existing source-compatible materialization without rewriting it.
    Share,
    /// Apply incoming directory metadata while retaining compatible peer claims.
    ApplyDirectory,
    /// Preserve an existing selected-root directory or symlink-to-directory anchor.
    PreserveSelectedRoot,
}

impl StagedAnchorDisposition {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Replace => "replace",
            Self::Share => "share",
            Self::ApplyDirectory => "apply-directory",
            Self::PreserveSelectedRoot => "preserve-selected-root",
        }
    }
}

/// One prepared package payload row.
#[derive(Debug, Clone)]
pub struct StagedPayloadRow {
    pub entry: FileEntry,
    pub package_name: String,
    pub component_name: Option<String>,
    pub directory_materialization: ExistingDirectoryMaterialization,
    pub disposition: StagedAnchorDisposition,
    pub selected_root_node: Option<ResolvedPayloadNode>,
    pub materialization_target_path: Option<String>,
    pub history: Option<(i64, &'static str)>,
}

/// One prepared config projection. File identity is resolved set-wise by path.
#[derive(Debug, Clone)]
pub struct StagedConfigRow {
    pub config: ConfigFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedHistoryAction {
    Add,
    Modify,
    Delete,
}

impl StagedHistoryAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedHistoryRow {
    pub changeset_id: i64,
    pub path: String,
    pub sha256_hash: Option<String>,
    pub action: StagedHistoryAction,
}

/// Canonical identity returned after set-based reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedPayloadOutcome {
    pub file_id: i64,
    pub content_sha256: Option<String>,
    pub materialized: bool,
}

/// Counted SQLite work owned by the staging authority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PackageTransactionSqlWork {
    pub rows_loaded: u64,
    pub load_statement_executions: u64,
    pub validation_query_executions: u64,
    pub reconciliation_statement_executions: u64,
    pub query_shapes: u64,
}

impl PackageTransactionSqlWork {
    pub const fn total_statement_executions(self) -> u64 {
        self.load_statement_executions
            + self.validation_query_executions
            + self.reconciliation_statement_executions
    }
}

/// Ephemeral package-row authority scoped to one SQLite transaction.
pub struct PackageTransactionStaging<'tx> {
    tx: &'tx Transaction<'tx>,
    work: PackageTransactionSqlWork,
}

impl<'tx> PackageTransactionStaging<'tx> {
    pub fn begin(tx: &'tx Transaction<'tx>) -> Result<Self> {
        tx.execute_batch(STAGING_SCHEMA_SQL)?;
        Ok(Self {
            tx,
            work: PackageTransactionSqlWork {
                reconciliation_statement_executions: 1,
                query_shapes: 1,
                ..PackageTransactionSqlWork::default()
            },
        })
    }

    pub fn clear(&mut self) -> Result<()> {
        self.tx.execute_batch(CLEAR_STAGING_SQL)?;
        self.work.reconciliation_statement_executions += 1;
        self.work.query_shapes += 1;
        Ok(())
    }

    pub fn stage_component(
        &mut self,
        trove_id: i64,
        name: &str,
        description: Option<&str>,
        is_installed: bool,
    ) -> Result<()> {
        self.tx
            .prepare_cached(
                "INSERT INTO conary_tx_components (trove_id, name, description, is_installed)
             VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![trove_id, name, description, is_installed])?;
        self.count_loaded_row();
        Ok(())
    }

    pub fn allow_replacement_of(&mut self, trove_id: i64) -> Result<()> {
        self.tx
            .prepare_cached(
                "INSERT OR IGNORE INTO conary_tx_replacement_owners (trove_id) VALUES (?1)",
            )?
            .execute([trove_id])?;
        self.count_loaded_row();
        Ok(())
    }

    pub fn stage_payload(&mut self, row: &StagedPayloadRow) -> Result<()> {
        validate_staged_payload(row)?;
        let node_json = super::file_entry::canonical_node_json(&row.entry.node)?;
        let selected_root_node_json = row
            .selected_root_node
            .as_ref()
            .map(super::file_entry::canonical_node_json)
            .transpose()?;
        let content_size = persisted_content_size(row.entry.content.as_ref())?;
        let anchor_policy = anchor_policy(row.directory_materialization, &row.entry.node);
        let (history_changeset_id, history_action) =
            row.history.map_or((None, None), |(changeset_id, action)| {
                (Some(changeset_id), Some(action))
            });
        self.tx
            .prepare_cached(
                "INSERT INTO conary_tx_payload (
                path, trove_id, package_name, component_name, payload_node_json,
                content_sha256, content_size, sharing_policy, anchor_policy,
                disposition, selected_root_node_json, materialization_target_path,
                history_changeset_id, history_action
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )?
            .execute(params![
                &row.entry.path,
                row.entry.trove_id,
                &row.package_name,
                &row.component_name,
                node_json,
                row.entry.content.as_ref().map(|content| &content.sha256),
                content_size,
                row.entry.claim_policy.as_str(),
                anchor_policy.as_str(),
                row.disposition.as_str(),
                selected_root_node_json,
                &row.materialization_target_path,
                history_changeset_id,
                history_action,
            ])?;
        self.count_loaded_row();
        Ok(())
    }

    pub fn stage_config(&mut self, row: &StagedConfigRow) -> Result<()> {
        let config = &row.config;
        let trove_id = config.trove_id.ok_or_else(|| {
            Error::ConfigError(format!(
                "staged config {} has no trove authority",
                config.path
            ))
        })?;
        self.tx
            .prepare_cached(
                "INSERT INTO conary_tx_configs (
                path, trove_id, original_hash, original_md5, current_hash,
                noreplace, ghost, materialized, remove_on_upgrade, status,
                modified_at, source
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?
            .execute(params![
                &config.path,
                trove_id,
                &config.original_hash,
                &config.original_md5,
                &config.current_hash,
                config.noreplace,
                config.ghost,
                config.materialized,
                config.remove_on_upgrade,
                config.status.as_str(),
                &config.modified_at,
                config.source.as_str(),
            ])?;
        self.count_loaded_row();
        Ok(())
    }

    pub fn stage_history(&mut self, row: &StagedHistoryRow) -> Result<()> {
        if let Some(hash) = row.sha256_hash.as_deref()
            && (hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(Error::ParseError(format!(
                "staged history {} has invalid SHA-256 identity",
                row.path
            )));
        }
        self.tx
            .prepare_cached(
                "INSERT INTO conary_tx_history (changeset_id, path, sha256_hash, action)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                row.changeset_id,
                &row.path,
                &row.sha256_hash,
                row.action.as_str(),
            ])?;
        self.count_loaded_row();
        Ok(())
    }

    pub fn validate_and_reconcile(&mut self) -> Result<BTreeMap<String, StagedPayloadOutcome>> {
        self.validate_references()?;
        let decisions = self.plan_anchor_decisions()?;
        self.persist_decisions(&decisions)?;
        self.reconcile_components()?;
        self.reconcile_contents()?;
        self.reconcile_anchors()?;
        self.reconcile_claims()?;
        self.reconcile_hardlinks()?;
        self.reconcile_history()?;
        self.reconcile_configs()?;
        self.load_outcomes()
    }

    pub fn finish(mut self) -> Result<PackageTransactionSqlWork> {
        self.tx.execute_batch(DROP_STAGING_SQL)?;
        self.work.reconciliation_statement_executions += 1;
        self.work.query_shapes += 1;
        Ok(self.work)
    }

    fn validate_references(&mut self) -> Result<()> {
        let missing_trove = self
            .tx
            .query_row(
                "SELECT p.path
             FROM conary_tx_payload p
             LEFT JOIN troves t ON t.id = p.trove_id
             WHERE t.id IS NULL
             ORDER BY p.path LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        self.count_validation_query();
        if let Some(path) = missing_trove {
            return Err(Error::ConfigError(format!(
                "staged payload {path} references a missing trove"
            )));
        }

        let missing_component = self
            .tx
            .query_row(
                "SELECT p.path, p.component_name
             FROM conary_tx_payload p
             LEFT JOIN conary_tx_components c
               ON c.trove_id = p.trove_id AND c.name = p.component_name
             LEFT JOIN components existing
               ON existing.parent_trove_id = p.trove_id AND existing.name = p.component_name
             WHERE p.component_name IS NOT NULL
               AND c.name IS NULL AND existing.name IS NULL
             ORDER BY p.path LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        self.count_validation_query();
        if let Some((path, component)) = missing_component {
            return Err(Error::ConfigError(format!(
                "staged payload {path} references missing component {component}"
            )));
        }
        Ok(())
    }

    fn plan_anchor_decisions(&mut self) -> Result<Vec<PlannedDecision>> {
        let mut statement = self.tx.prepare(
            "SELECT p.path, p.trove_id, p.package_name, p.payload_node_json,
                    p.content_sha256, p.content_size, p.sharing_policy,
                    p.anchor_policy, p.disposition, p.selected_root_node_json,
                    f.id, f.payload_node_json, f.content_sha256, f.content_size,
                    f.trove_id, owner.name, owner.install_source
             FROM conary_tx_payload p
             LEFT JOIN files f ON f.path = p.path
             LEFT JOIN troves owner ON owner.id = f.trove_id
             ORDER BY p.path, p.trove_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(StagedAnchorInput {
                    path: row.get(0)?,
                    trove_id: row.get(1)?,
                    package_name: row.get(2)?,
                    incoming_node_json: row.get(3)?,
                    incoming_sha256: row.get(4)?,
                    incoming_size: row.get(5)?,
                    incoming_policy: row.get(6)?,
                    anchor_policy: row.get(7)?,
                    disposition: row.get(8)?,
                    selected_root_node_json: row.get(9)?,
                    existing_id: row.get(10)?,
                    existing_node_json: row.get(11)?,
                    existing_sha256: row.get(12)?,
                    existing_size: row.get(13)?,
                    existing_trove_id: row.get(14)?,
                    existing_owner_name: row.get(15)?,
                    existing_install_source: row.get(16)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        self.count_validation_query();

        let mut decisions = Vec::with_capacity(rows.len());
        for row in rows {
            decisions.push(self.plan_anchor_decision(row)?);
        }
        Ok(decisions)
    }

    fn plan_anchor_decision(&mut self, row: StagedAnchorInput) -> Result<PlannedDecision> {
        let incoming_node = parse_node(&row.path, &row.incoming_node_json)?;
        let incoming_content = parse_content(&row.path, row.incoming_sha256, row.incoming_size)?;
        let incoming_claim = PayloadClaim::new(
            row.path.clone(),
            row.trove_id,
            incoming_node.clone(),
            incoming_content,
            crate::payload::PayloadSharingPolicy::parse(&row.incoming_policy)
                .map_err(|error| Error::ParseError(error.to_string()))?,
        )?
        .with_anchor_policy(parse_anchor_policy(&row.anchor_policy)?);
        let disposition = parse_disposition(&row.disposition)?;

        let existing = match (
            row.existing_id,
            row.existing_node_json,
            row.existing_trove_id,
        ) {
            (Some(id), Some(node_json), Some(trove_id)) => Some(FileEntry {
                id: Some(id),
                path: row.path.clone(),
                node: parse_node(&row.path, &node_json)?,
                content: parse_content(&row.path, row.existing_sha256, row.existing_size)?,
                trove_id,
                installed_at: None,
                component_id: None,
                claim_policy: crate::payload::PayloadSharingPolicy::Exclusive,
            }),
            (None, None, None) => None,
            _ => {
                return Err(Error::ConfigError(format!(
                    "materialized payload {} has incomplete canonical authority",
                    row.path
                )));
            }
        };

        match disposition {
            StagedAnchorDisposition::Auto => {
                let Some(existing) = existing.as_ref() else {
                    return Ok(PlannedDecision::replace(row.path, row.trove_id));
                };
                if matches!(incoming_node.source.kind, PayloadNodeKind::Directory)
                    && incoming_claim
                        .anchor_policy
                        .accepts_kind(&existing.node.source.kind)
                {
                    incoming_claim.validate_anchor(existing)?;
                    return Ok(PlannedDecision::apply_directory(row.path, row.trove_id));
                }
                if self.claims_are_compatible(&incoming_claim)? {
                    incoming_claim.validate_anchor(existing)?;
                    return Ok(PlannedDecision::keep(row.path, row.trove_id, false));
                }
                self.require_replaceable_claims(
                    &row.path,
                    &row.package_name,
                    existing.trove_id,
                    row.existing_owner_name.as_deref(),
                    row.existing_install_source.as_deref(),
                )?;
                Ok(PlannedDecision::replace(row.path, row.trove_id))
            }
            StagedAnchorDisposition::Replace => {
                if let Some(existing) = existing.as_ref() {
                    self.require_replaceable_claims(
                        &row.path,
                        &row.package_name,
                        existing.trove_id,
                        row.existing_owner_name.as_deref(),
                        row.existing_install_source.as_deref(),
                    )?;
                }
                Ok(PlannedDecision::replace(row.path, row.trove_id))
            }
            StagedAnchorDisposition::Share => {
                let Some(existing) = existing else {
                    // Upgrade finalization may delete the old trove and its
                    // compatible anchor after preflight. Re-anchor the exact
                    // incoming authority without claiming another root write.
                    return Ok(PlannedDecision::replace_without_materialization(
                        row.path,
                        row.trove_id,
                    ));
                };
                self.require_compatible_claims(&incoming_claim)?;
                incoming_claim.validate_anchor(&existing)?;
                Ok(PlannedDecision::keep(row.path, row.trove_id, false))
            }
            StagedAnchorDisposition::ApplyDirectory => {
                if !matches!(incoming_node.source.kind, PayloadNodeKind::Directory) {
                    return Err(Error::ConfigError(format!(
                        "staged directory application {} is not a directory",
                        row.path
                    )));
                }
                if let Some(existing) = existing.as_ref() {
                    incoming_claim.validate_anchor(existing)?;
                } else if row.selected_root_node_json.is_none() {
                    return Err(Error::ConflictError(format!(
                        "staged directory application {} has no selected-root anchor",
                        row.path
                    )));
                }
                Ok(PlannedDecision::apply_directory(row.path, row.trove_id))
            }
            StagedAnchorDisposition::PreserveSelectedRoot => {
                if let Some(existing) = existing.as_ref() {
                    incoming_claim.validate_anchor(existing)?;
                    Ok(PlannedDecision::keep(row.path, row.trove_id, false))
                } else {
                    let node_json = row.selected_root_node_json.ok_or_else(|| {
                        Error::ConflictError(format!(
                            "staged preserved payload {} has no selected-root node",
                            row.path
                        ))
                    })?;
                    let anchor = FileEntry {
                        id: None,
                        path: row.path.clone(),
                        node: parse_node(&row.path, &node_json)?,
                        content: None,
                        trove_id: row.trove_id,
                        installed_at: None,
                        component_id: None,
                        claim_policy: crate::payload::PayloadSharingPolicy::Exclusive,
                    };
                    incoming_claim.validate_anchor(&anchor)?;
                    Ok(PlannedDecision::insert_selected_root(
                        row.path,
                        row.trove_id,
                    ))
                }
            }
        }
    }

    fn require_replaceable_claims(
        &mut self,
        path: &str,
        package_name: &str,
        existing_trove_id: i64,
        existing_owner_name: Option<&str>,
        existing_install_source: Option<&str>,
    ) -> Result<()> {
        let blocked = self
            .tx
            .query_row(
                "SELECT pc.trove_id, t.name
             FROM payload_claims pc
             JOIN troves t ON t.id = pc.trove_id
             LEFT JOIN conary_tx_replacement_owners allowed ON allowed.trove_id = pc.trove_id
             WHERE pc.path = ?1
               AND allowed.trove_id IS NULL
               AND t.name != ?2
               AND t.install_source != 'captured-root'
             ORDER BY pc.trove_id LIMIT 1",
                params![path, package_name],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        self.count_validation_query();
        if let Some((trove_id, owner)) = blocked {
            return Err(Error::ConflictError(format!(
                "staged payload {path} cannot replace claim from package {owner} ({trove_id})"
            )));
        }
        if existing_owner_name.is_none() {
            return Err(Error::ConfigError(format!(
                "materialized payload {path} references missing trove {existing_trove_id}"
            )));
        }
        let anchor_allowed = existing_owner_name == Some(package_name)
            || existing_install_source == Some("captured-root")
            || self.tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM conary_tx_replacement_owners WHERE trove_id = ?1
                 )",
                [existing_trove_id],
                |row| row.get::<_, bool>(0),
            )?;
        self.count_validation_query();
        if !anchor_allowed {
            return Err(Error::ConflictError(format!(
                "staged payload {path} cannot replace package {}",
                existing_owner_name.unwrap_or("<missing>")
            )));
        }
        Ok(())
    }

    fn require_compatible_claims(&mut self, incoming: &PayloadClaim) -> Result<()> {
        let claims = PayloadClaim::find_by_path(self.tx, &incoming.path)?;
        self.count_validation_query();
        if claims.is_empty() {
            return Err(Error::ConfigError(format!(
                "materialized payload {} has no package claim",
                incoming.path
            )));
        }
        for claim in claims {
            incoming.compatible_with(&claim).map_err(|mismatch| {
                Error::ConflictError(format!(
                    "staged payload {} is incompatible with trove {}: {mismatch}",
                    incoming.path, claim.trove_id
                ))
            })?;
        }
        Ok(())
    }

    fn claims_are_compatible(&mut self, incoming: &PayloadClaim) -> Result<bool> {
        let claims = PayloadClaim::find_by_path(self.tx, &incoming.path)?;
        self.count_validation_query();
        if claims.is_empty() {
            return Err(Error::ConfigError(format!(
                "materialized payload {} has no package claim",
                incoming.path
            )));
        }
        Ok(claims
            .iter()
            .all(|claim| incoming.compatible_with(claim).is_ok()))
    }

    fn persist_decisions(&mut self, decisions: &[PlannedDecision]) -> Result<()> {
        let mut statement = self.tx.prepare_cached(
            "INSERT INTO conary_tx_decisions (path, trove_id, anchor_action, materialized)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for decision in decisions {
            statement.execute(params![
                &decision.path,
                decision.trove_id,
                decision.anchor_action,
                decision.materialized,
            ])?;
            self.work.rows_loaded += 1;
            self.work.load_statement_executions += 1;
        }
        self.work.query_shapes += 1;
        Ok(())
    }

    fn reconcile_components(&mut self) -> Result<()> {
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

    fn reconcile_contents(&mut self) -> Result<()> {
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

    fn reconcile_anchors(&mut self) -> Result<()> {
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

    fn reconcile_claims(&mut self) -> Result<()> {
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
             LEFT JOIN components c
               ON c.parent_trove_id = p.trove_id AND c.name = p.component_name
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

    fn reconcile_hardlinks(&mut self) -> Result<()> {
        let missing_target = self
            .tx
            .query_row(
                "SELECT p.path, json_extract(p.payload_node_json, '$.source.kind.target')
             FROM conary_tx_payload p
             LEFT JOIN files target
               ON target.path = json_extract(p.payload_node_json, '$.source.kind.target')
             WHERE json_extract(p.payload_node_json, '$.source.kind.type') = 'hardlink'
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
               )",
            [],
        )?;
        self.count_reconciliation_statement();
        Ok(())
    }

    fn reconcile_history(&mut self) -> Result<()> {
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

    fn reconcile_configs(&mut self) -> Result<()> {
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

    fn load_outcomes(&mut self) -> Result<BTreeMap<String, StagedPayloadOutcome>> {
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

    fn count_loaded_row(&mut self) {
        self.work.rows_loaded += 1;
        self.work.load_statement_executions += 1;
        self.work.query_shapes = self.work.query_shapes.max(5);
    }

    fn count_validation_query(&mut self) {
        self.work.validation_query_executions += 1;
        self.work.query_shapes += 1;
    }

    fn count_reconciliation_statement(&mut self) {
        self.work.reconciliation_statement_executions += 1;
        self.work.query_shapes += 1;
    }
}

#[cfg(test)]
#[path = "package_transaction_staging/tests.rs"]
mod tests;
