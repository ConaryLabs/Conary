// conary-core/src/db/models/package_transaction_staging.rs

//! Transaction-local package row staging and set-based reconciliation.

use super::{ConfigFile, ExistingDirectoryMaterialization, FileEntry, PayloadClaim};
use crate::error::{Error, Result};
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

use self::planning::{
    LoadedClaim, PlannedDecision, StagedAnchorInput, StagedClaimInput, anchor_policy,
    parse_anchor_policy, parse_content, parse_disposition, parse_node, persisted_content_size,
    validate_staged_payload,
};
use self::sql::{CLEAR_STAGING_SQL, DROP_STAGING_SQL, STAGING_SCHEMA_SQL};

mod planning;
mod reconcile;
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
    /// Reconcile an anchor already owned by the staged package without replacing its claim.
    ReconcileOwned,
    /// Reconcile complete selected-root materialization without claiming owned paths.
    ReconcileCapturedRoot,
}

impl StagedAnchorDisposition {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Replace => "replace",
            Self::Share => "share",
            Self::ApplyDirectory => "apply-directory",
            Self::PreserveSelectedRoot => "preserve-selected-root",
            Self::ReconcileOwned => "reconcile-owned",
            Self::ReconcileCapturedRoot => "reconcile-captured-root",
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
        let claims = self.load_claims()?;

        let mut decisions = Vec::with_capacity(rows.len());
        for row in rows {
            decisions.push(Self::plan_anchor_decision(row, &claims)?);
        }
        Ok(decisions)
    }

    fn load_claims(&mut self) -> Result<BTreeMap<String, Vec<LoadedClaim>>> {
        let mut statement = self.tx.prepare(
            "SELECT pc.path, pc.trove_id, pc.component_id, pc.sharing_policy,
                    pc.anchor_policy, pc.materialization_target_path,
                    pc.payload_node_json, pc.content_sha256, pc.content_size,
                    owner.name, owner.install_source,
                    allowed.trove_id IS NOT NULL
             FROM payload_claims pc
             JOIN troves owner ON owner.id = pc.trove_id
             LEFT JOIN conary_tx_replacement_owners allowed ON allowed.trove_id = pc.trove_id
             WHERE pc.path IN (SELECT path FROM conary_tx_payload)
                OR pc.materialization_target_path IN (SELECT path FROM conary_tx_payload)
             ORDER BY pc.path, pc.trove_id",
        )?;
        let raw = statement
            .query_map([], |row| {
                Ok(StagedClaimInput {
                    path: row.get(0)?,
                    trove_id: row.get(1)?,
                    component_id: row.get(2)?,
                    sharing_policy: row.get(3)?,
                    anchor_policy: row.get(4)?,
                    materialization_target_path: row.get(5)?,
                    node_json: row.get(6)?,
                    content_sha256: row.get(7)?,
                    content_size: row.get(8)?,
                    owner_name: row.get(9)?,
                    owner_install_source: row.get(10)?,
                    replacement_allowed: row.get(11)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        self.count_validation_query();

        let mut claims = BTreeMap::<String, Vec<LoadedClaim>>::new();
        for row in raw {
            let claim = PayloadClaim::new(
                row.path.clone(),
                row.trove_id,
                parse_node(&row.path, &row.node_json)?,
                parse_content(&row.path, row.content_sha256, row.content_size)?,
                crate::payload::PayloadSharingPolicy::parse(&row.sharing_policy)
                    .map_err(|error| Error::ParseError(error.to_string()))?,
            )?
            .with_component(row.component_id)
            .with_anchor_policy(parse_anchor_policy(&row.anchor_policy)?)
            .with_materialization_target(row.materialization_target_path.clone());
            let loaded = LoadedClaim {
                claim,
                owner_name: row.owner_name,
                owner_install_source: row.owner_install_source,
                replacement_allowed: row.replacement_allowed,
            };
            if let Some(target) = row.materialization_target_path
                && target != row.path
            {
                claims.entry(target).or_default().push(loaded.clone());
            }
            claims.entry(row.path).or_default().push(loaded);
        }
        Ok(claims)
    }

    fn plan_anchor_decision(
        row: StagedAnchorInput,
        claims_by_path: &BTreeMap<String, Vec<LoadedClaim>>,
    ) -> Result<PlannedDecision> {
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
                if Self::claims_are_compatible(&incoming_claim, claims_by_path)? {
                    incoming_claim.validate_anchor(existing)?;
                    return Ok(PlannedDecision::keep(row.path, row.trove_id, false));
                }
                Self::require_replaceable_claims(
                    &row.path,
                    &row.package_name,
                    existing.trove_id,
                    row.existing_owner_name.as_deref(),
                    row.existing_install_source.as_deref(),
                    claims_by_path,
                )?;
                Ok(PlannedDecision::replace(row.path, row.trove_id))
            }
            StagedAnchorDisposition::Replace => {
                if let Some(existing) = existing.as_ref() {
                    Self::require_replaceable_claims(
                        &row.path,
                        &row.package_name,
                        existing.trove_id,
                        row.existing_owner_name.as_deref(),
                        row.existing_install_source.as_deref(),
                        claims_by_path,
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
                Self::require_compatible_claims(&incoming_claim, claims_by_path)?;
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
            StagedAnchorDisposition::ReconcileOwned => {
                let existing = existing.ok_or_else(|| {
                    Error::NotFound(format!(
                        "owned staged payload {} has no materialized anchor",
                        row.path
                    ))
                })?;
                let candidate = FileEntry {
                    id: existing.id,
                    path: row.path.clone(),
                    node: incoming_claim.node.clone(),
                    content: incoming_claim.content.clone(),
                    trove_id: existing.trove_id,
                    installed_at: existing.installed_at.clone(),
                    component_id: existing.component_id,
                    claim_policy: crate::payload::PayloadSharingPolicy::Exclusive,
                };
                Self::validate_owned_reconciled_anchor(
                    &candidate,
                    &incoming_claim,
                    claims_by_path,
                )?;
                Ok(PlannedDecision::reconcile_owned(row.path, row.trove_id))
            }
            StagedAnchorDisposition::ReconcileCapturedRoot => {
                let Some(existing) = existing.as_ref() else {
                    return Ok(PlannedDecision::replace(row.path, row.trove_id));
                };
                let candidate = FileEntry {
                    id: existing.id,
                    path: row.path.clone(),
                    node: incoming_claim.node.clone(),
                    content: incoming_claim.content.clone(),
                    trove_id: existing.trove_id,
                    installed_at: existing.installed_at.clone(),
                    component_id: existing.component_id,
                    claim_policy: crate::payload::PayloadSharingPolicy::Exclusive,
                };
                Self::validate_reconciled_anchor(&candidate, claims_by_path)?;
                Ok(PlannedDecision::reconcile_materialization(
                    row.path,
                    row.trove_id,
                ))
            }
        }
    }

    fn validate_reconciled_anchor(
        candidate: &FileEntry,
        claims_by_path: &BTreeMap<String, Vec<LoadedClaim>>,
    ) -> Result<()> {
        let retainers = claims_by_path
            .get(&candidate.path)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if retainers.is_empty() {
            return Err(Error::ConfigError(format!(
                "materialized payload {} has no package claim",
                candidate.path
            )));
        }
        for loaded in retainers {
            if loaded.claim.path == candidate.path {
                loaded.claim.validate_anchor(candidate)?;
            } else if !matches!(candidate.node.source.kind, PayloadNodeKind::Directory) {
                return Err(Error::ConflictError(format!(
                    "selected-root node {} is incompatible with claim {} anchor policy '{}'",
                    candidate.path,
                    loaded.claim.trove_id,
                    loaded.claim.anchor_policy.as_str()
                )));
            }
        }
        Ok(())
    }

    fn validate_owned_reconciled_anchor(
        candidate: &FileEntry,
        incoming: &PayloadClaim,
        claims_by_path: &BTreeMap<String, Vec<LoadedClaim>>,
    ) -> Result<()> {
        let retainers = claims_by_path
            .get(&candidate.path)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let mut owns_path = false;
        for loaded in retainers {
            if loaded.claim.path == candidate.path && loaded.claim.trove_id == incoming.trove_id {
                owns_path = true;
                incoming.validate_anchor(candidate)?;
            } else if loaded.claim.path == candidate.path {
                incoming.compatible_with(&loaded.claim).map_err(|mismatch| {
                    Error::ConflictError(format!(
                        "replacement payload claim {} from trove {} is incompatible with trove {}: {mismatch}",
                        candidate.path, incoming.trove_id, loaded.claim.trove_id
                    ))
                })?;
                loaded.claim.validate_anchor(candidate)?;
            } else if !matches!(candidate.node.source.kind, PayloadNodeKind::Directory) {
                return Err(Error::ConflictError(format!(
                    "selected-root node {} is incompatible with claim {} anchor policy '{}'",
                    candidate.path,
                    loaded.claim.trove_id,
                    loaded.claim.anchor_policy.as_str()
                )));
            }
        }
        if !owns_path {
            return Err(Error::NotFound(format!(
                "staged package {} does not own materialized payload {}",
                incoming.trove_id, candidate.path
            )));
        }
        Ok(())
    }

    fn require_replaceable_claims(
        path: &str,
        package_name: &str,
        existing_trove_id: i64,
        existing_owner_name: Option<&str>,
        existing_install_source: Option<&str>,
        claims_by_path: &BTreeMap<String, Vec<LoadedClaim>>,
    ) -> Result<()> {
        let blocked = claims_by_path
            .get(path)
            .into_iter()
            .flatten()
            .filter(|loaded| loaded.claim.path == path)
            .find(|loaded| {
                !loaded.replacement_allowed
                    && loaded.owner_name != package_name
                    && loaded.owner_install_source != "captured-root"
            });
        if let Some(blocked) = blocked {
            return Err(Error::ConflictError(format!(
                "staged payload {path} cannot replace claim from package {} ({})",
                blocked.owner_name, blocked.claim.trove_id
            )));
        }
        if existing_owner_name.is_none() {
            return Err(Error::ConfigError(format!(
                "materialized payload {path} references missing trove {existing_trove_id}"
            )));
        }
        let anchor_allowed = existing_owner_name == Some(package_name)
            || existing_install_source == Some("captured-root")
            || claims_by_path
                .get(path)
                .into_iter()
                .flatten()
                .any(|loaded| {
                    loaded.claim.trove_id == existing_trove_id && loaded.replacement_allowed
                });
        if !anchor_allowed {
            return Err(Error::ConflictError(format!(
                "staged payload {path} cannot replace package {}",
                existing_owner_name.unwrap_or("<missing>")
            )));
        }
        Ok(())
    }

    fn require_compatible_claims(
        incoming: &PayloadClaim,
        claims_by_path: &BTreeMap<String, Vec<LoadedClaim>>,
    ) -> Result<()> {
        let claims = claims_by_path
            .get(&incoming.path)
            .into_iter()
            .flatten()
            .filter(|loaded| loaded.claim.path == incoming.path)
            .collect::<Vec<_>>();
        if claims.is_empty() {
            return Err(Error::ConfigError(format!(
                "materialized payload {} has no package claim",
                incoming.path
            )));
        }
        for loaded in claims {
            incoming
                .compatible_with(&loaded.claim)
                .map_err(|mismatch| {
                    Error::ConflictError(format!(
                        "staged payload {} is incompatible with trove {}: {mismatch}",
                        incoming.path, loaded.claim.trove_id
                    ))
                })?;
        }
        Ok(())
    }

    fn claims_are_compatible(
        incoming: &PayloadClaim,
        claims_by_path: &BTreeMap<String, Vec<LoadedClaim>>,
    ) -> Result<bool> {
        let claims = claims_by_path
            .get(&incoming.path)
            .into_iter()
            .flatten()
            .filter(|loaded| loaded.claim.path == incoming.path)
            .collect::<Vec<_>>();
        if claims.is_empty() {
            return Err(Error::ConfigError(format!(
                "materialized payload {} has no package claim",
                incoming.path
            )));
        }
        Ok(claims
            .iter()
            .all(|loaded| incoming.compatible_with(&loaded.claim).is_ok()))
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
