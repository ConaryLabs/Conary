// conary-core/src/db/models/payload_claim.rs
//! Format-owned many-to-many claims for materialized payload nodes.

use super::FileEntry;
use crate::error::{Error, Result};
use crate::payload::{
    PayloadContentAuthority, PayloadNodeKind, PayloadSharingPolicy, ResolvedPayloadNode,
};
use rusqlite::{Connection, Row, Transaction, params, types::Type};
use serde::{Deserialize, Serialize};
use std::path::{Component as PathComponent, Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadClaimAnchorPolicy {
    Exact,
    Directory,
    DirectoryOrSymlinkToDirectory,
}

impl PayloadClaimAnchorPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Directory => "directory",
            Self::DirectoryOrSymlinkToDirectory => "directory-or-symlink-to-directory",
        }
    }

    fn from_str(value: &str) -> rusqlite::Result<Self> {
        match value {
            "exact" => Ok(Self::Exact),
            "directory" => Ok(Self::Directory),
            "directory-or-symlink-to-directory" => Ok(Self::DirectoryOrSymlinkToDirectory),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                Type::Text,
                format!("unknown payload claim anchor policy {other:?}").into(),
            )),
        }
    }

    pub const fn accepts_kind(self, kind: &PayloadNodeKind) -> bool {
        match self {
            Self::Exact => !matches!(kind, PayloadNodeKind::Directory),
            Self::Directory => matches!(kind, PayloadNodeKind::Directory),
            Self::DirectoryOrSymlinkToDirectory => {
                matches!(
                    kind,
                    PayloadNodeKind::Directory | PayloadNodeKind::Symlink { .. }
                )
            }
        }
    }
}

/// One package's exact claim on a selected-root payload path.
///
/// `files` stores one materialized anchor. This row stores the independent
/// package declaration, including source-format sharing authority, even when
/// another package owns that anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadClaim {
    pub path: String,
    pub trove_id: i64,
    pub component_id: Option<i64>,
    pub sharing_policy: PayloadSharingPolicy,
    pub anchor_policy: PayloadClaimAnchorPolicy,
    pub materialization_target_path: Option<String>,
    pub node: ResolvedPayloadNode,
    pub content: Option<PayloadContentAuthority>,
    pub claimed_at: Option<String>,
}

impl PayloadClaim {
    const COLUMNS: &'static str = "path, trove_id, component_id, sharing_policy, \
         anchor_policy, materialization_target_path, payload_node_json, \
         content_sha256, content_size, claimed_at";

    pub fn new(
        path: String,
        trove_id: i64,
        node: ResolvedPayloadNode,
        content: Option<PayloadContentAuthority>,
        sharing_policy: PayloadSharingPolicy,
    ) -> Result<Self> {
        let anchor_policy = if matches!(node.source.kind, PayloadNodeKind::Directory) {
            PayloadClaimAnchorPolicy::Directory
        } else {
            PayloadClaimAnchorPolicy::Exact
        };
        let claim = Self {
            path,
            trove_id,
            component_id: None,
            sharing_policy,
            anchor_policy,
            materialization_target_path: None,
            node,
            content,
            claimed_at: None,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn new_directory(path: String, trove_id: i64, node: ResolvedPayloadNode) -> Result<Self> {
        Self::new(path, trove_id, node, None, PayloadSharingPolicy::Exclusive)
    }

    pub fn with_component(mut self, component_id: Option<i64>) -> Self {
        self.component_id = component_id;
        self
    }

    pub fn with_anchor_policy(mut self, anchor_policy: PayloadClaimAnchorPolicy) -> Self {
        self.anchor_policy = anchor_policy;
        self
    }

    pub fn with_materialization_target(mut self, target_path: Option<String>) -> Self {
        self.materialization_target_path = target_path;
        self
    }

    pub fn compatible_with(
        &self,
        other: &Self,
    ) -> std::result::Result<(), crate::payload::PayloadCompatibilityMismatch> {
        if matches!(self.node.source.kind, PayloadNodeKind::Directory)
            && matches!(other.node.source.kind, PayloadNodeKind::Directory)
        {
            return Ok(());
        }
        if (matches!(self.node.source.kind, PayloadNodeKind::Directory)
            && self.anchor_policy == PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
            && matches!(other.node.source.kind, PayloadNodeKind::Symlink { .. }))
            || (matches!(other.node.source.kind, PayloadNodeKind::Directory)
                && other.anchor_policy == PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
                && matches!(self.node.source.kind, PayloadNodeKind::Symlink { .. }))
        {
            return Ok(());
        }
        self.sharing_policy.compare(
            other.sharing_policy,
            &self.node.source,
            self.content.as_ref(),
            &other.node.source,
            other.content.as_ref(),
        )
    }

    /// Persist one package's independent payload claim.
    ///
    /// All existing direct claims must independently admit the incoming claim.
    /// Materialization-target retainers are not declarations for this path and
    /// therefore do not participate in source payload comparison.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        let anchor = FileEntry::find_by_path(conn, &self.path)?.ok_or_else(|| {
            Error::ConfigError(format!(
                "payload claim {} has no materialized file anchor",
                self.path
            ))
        })?;
        self.validate_anchor(&anchor)?;
        for peer in Self::find_by_path(conn, &self.path)?
            .into_iter()
            .filter(|peer| peer.trove_id != self.trove_id)
        {
            self.compatible_with(&peer).map_err(|mismatch| {
                Error::ConflictError(format!(
                    "payload claim {} from trove {} is incompatible with trove {}: {mismatch}",
                    self.path, self.trove_id, peer.trove_id
                ))
            })?;
        }
        if let Some(target_path) = self.materialization_target_path.as_deref() {
            let target = FileEntry::find_by_path(conn, target_path)?.ok_or_else(|| {
                Error::ConfigError(format!(
                    "payload claim {} materialization target {} has no file anchor",
                    self.path, target_path
                ))
            })?;
            if !matches!(anchor.node.source.kind, PayloadNodeKind::Symlink { .. })
                || !matches!(target.node.source.kind, PayloadNodeKind::Directory)
            {
                return Err(Error::ConfigError(format!(
                    "payload claim {} materialization target {} is not a typed symlink-to-directory edge",
                    self.path, target_path
                )));
            }
        }
        if let Some(component_id) = self.component_id {
            let component_matches_trove: bool = conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM components
                    WHERE id = ?1 AND parent_trove_id = ?2
                )",
                params![component_id, self.trove_id],
                |row| row.get(0),
            )?;
            if !component_matches_trove {
                return Err(Error::ConfigError(format!(
                    "payload claim {} component {} does not belong to trove {}",
                    self.path, component_id, self.trove_id
                )));
            }
        }

        let node_json = super::file_entry::canonical_node_json(&self.node)?;
        let content_size = persisted_content_size(self.content.as_ref())?;
        conn.execute(
            "INSERT INTO payload_claims (
                path, trove_id, component_id, sharing_policy, anchor_policy,
                materialization_target_path, payload_node_json,
                content_sha256, content_size
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(path, trove_id) DO UPDATE SET
                component_id = excluded.component_id,
                sharing_policy = excluded.sharing_policy,
                anchor_policy = excluded.anchor_policy,
                materialization_target_path = excluded.materialization_target_path,
                payload_node_json = excluded.payload_node_json,
                content_sha256 = excluded.content_sha256,
                content_size = excluded.content_size",
            params![
                &self.path,
                self.trove_id,
                self.component_id,
                self.sharing_policy.as_str(),
                self.anchor_policy.as_str(),
                &self.materialization_target_path,
                node_json,
                self.content.as_ref().map(|content| &content.sha256),
                content_size,
            ],
        )?;
        Ok(())
    }

    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM payload_claims WHERE path = ?1 ORDER BY trove_id",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([path], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM payload_claims WHERE trove_id = ?1 ORDER BY path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return every package claim that retains one materialized path.
    pub fn find_retaining_path(conn: &Connection, path: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM payload_claims
             WHERE path = ?1 OR materialization_target_path = ?1
             ORDER BY trove_id, path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([path], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_materialization_target(
        conn: &Connection,
        path: &str,
        trove_id: i64,
        target_path: &str,
    ) -> Result<()> {
        let mut claim = Self::find_by_path(conn, path)?
            .into_iter()
            .find(|claim| claim.trove_id == trove_id)
            .ok_or_else(|| Error::NotFound(format!("payload claim {path} for trove {trove_id}")))?;
        claim.materialization_target_path = Some(target_path.to_string());
        claim.insert(conn)
    }

    /// Delete one package claim inside a caller-owned transaction.
    ///
    /// Removing the anchor claimant first transfers the canonical `files` row
    /// to the lowest remaining retainer. The final claim deletes the anchor.
    pub fn delete(tx: &Transaction<'_>, path: &str, trove_id: i64) -> Result<()> {
        let claims = Self::find_by_path(tx, path)?;
        let claim = claims
            .into_iter()
            .find(|claim| claim.trove_id == trove_id)
            .ok_or_else(|| Error::NotFound(format!("payload claim {path} for trove {trove_id}")))?;
        let target_path = claim.materialization_target_path.clone();
        let deleted = tx.execute(
            "DELETE FROM payload_claims WHERE path = ?1 AND trove_id = ?2",
            params![path, trove_id],
        )?;
        if deleted != 1 {
            return Err(Error::InternalError(format!(
                "payload claim {path} for trove {trove_id} was not deleted exactly"
            )));
        }
        reconcile_anchor_after_claim_delete(tx, path, trove_id)?;
        if let Some(target_path) = target_path {
            reconcile_anchor_after_claim_delete(tx, &target_path, trove_id)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        validate_absolute_path(&self.path, "payload claim")?;
        if let Some(target_path) = self.materialization_target_path.as_deref() {
            validate_absolute_path(target_path, "payload materialization target")?;
            if target_path == self.path {
                return Err(Error::ConfigError(format!(
                    "payload claim {} cannot target itself",
                    self.path
                )));
            }
        }
        self.node
            .validate()
            .and_then(|()| self.node.source.validate_content(self.content.as_ref()))
            .map_err(|error| {
                Error::ConfigError(format!(
                    "payload claim {} has invalid payload authority: {error}",
                    self.path
                ))
            })?;
        match (&self.node.source.kind, self.anchor_policy) {
            (PayloadNodeKind::Directory, PayloadClaimAnchorPolicy::Directory)
            | (
                PayloadNodeKind::Directory,
                PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory,
            ) => {}
            (PayloadNodeKind::Directory, PayloadClaimAnchorPolicy::Exact) => {
                return Err(Error::ConfigError(format!(
                    "directory payload claim {} cannot use exact anchor policy",
                    self.path
                )));
            }
            (_, PayloadClaimAnchorPolicy::Exact) => {}
            (_, policy) => {
                return Err(Error::ConfigError(format!(
                    "non-directory payload claim {} cannot use anchor policy '{}'",
                    self.path,
                    policy.as_str()
                )));
            }
        }
        if self.materialization_target_path.is_some()
            && !matches!(self.node.source.kind, PayloadNodeKind::Directory)
        {
            return Err(Error::ConfigError(format!(
                "non-directory payload claim {} cannot carry a materialization target",
                self.path
            )));
        }
        Ok(())
    }

    fn validate_anchor(&self, anchor: &FileEntry) -> Result<()> {
        if !self.anchor_policy.accepts_kind(&anchor.node.source.kind) {
            return Err(Error::ConfigError(format!(
                "payload claim {} anchor kind is not accepted by policy '{}'",
                self.path,
                self.anchor_policy.as_str()
            )));
        }
        if self.anchor_policy == PayloadClaimAnchorPolicy::Exact
            && (self.node != anchor.node || self.content != anchor.content)
        {
            self.sharing_policy
                .compare(
                    self.sharing_policy,
                    &self.node.source,
                    self.content.as_ref(),
                    &anchor.node.source,
                    anchor.content.as_ref(),
                )
                .map_err(|mismatch| {
                    Error::ConfigError(format!(
                        "payload claim {} is incompatible with its materialized anchor: {mismatch}",
                        self.path
                    ))
                })?;
        }
        Ok(())
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let sharing_policy = PayloadSharingPolicy::parse(row.get::<_, String>(3)?.as_str())
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(3, Type::Text, error.into())
            })?;
        let anchor_policy = PayloadClaimAnchorPolicy::from_str(row.get::<_, String>(4)?.as_str())?;
        let node_json: String = row.get(6)?;
        let node = serde_json::from_str::<ResolvedPayloadNode>(&node_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(6, Type::Text, error.into())
        })?;
        node.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(6, Type::Text, error.into())
        })?;
        let sha256: Option<String> = row.get(7)?;
        let size: Option<i64> = row.get(8)?;
        let content = match (sha256, size) {
            (Some(sha256), Some(size)) => Some(PayloadContentAuthority {
                sha256,
                size: u64::try_from(size).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(8, Type::Integer, error.into())
                })?,
            }),
            (None, None) => None,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    7,
                    Type::Text,
                    "payload claim content authority is incomplete".into(),
                ));
            }
        };
        let claim = Self {
            path: row.get(0)?,
            trove_id: row.get(1)?,
            component_id: row.get(2)?,
            sharing_policy,
            anchor_policy,
            materialization_target_path: row.get(5)?,
            node,
            content,
            claimed_at: row.get(9)?,
        };
        claim.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(6, Type::Text, error.into())
        })?;
        Ok(claim)
    }
}

fn reconcile_anchor_after_claim_delete(
    tx: &Transaction<'_>,
    path: &str,
    deleted_trove_id: i64,
) -> Result<()> {
    let Some(anchor) = FileEntry::find_by_path(tx, path)? else {
        return Ok(());
    };
    if anchor.trove_id != deleted_trove_id {
        return Ok(());
    }
    let retainers = PayloadClaim::find_retaining_path(tx, path)?;
    if let Some(next_owner) = retainers
        .iter()
        .min_by_key(|claim| (claim.trove_id, claim.path.as_str()))
    {
        let component_id = (next_owner.path == path)
            .then_some(next_owner.component_id)
            .flatten();
        tx.execute(
            "UPDATE files
             SET trove_id = ?1, component_id = ?2
             WHERE path = ?3",
            params![next_owner.trove_id, component_id, path],
        )?;
    } else {
        FileEntry::delete(tx, path)?;
    }
    Ok(())
}

fn persisted_content_size(content: Option<&PayloadContentAuthority>) -> Result<Option<i64>> {
    content
        .map(|content| {
            i64::try_from(content.size).map_err(|_| {
                Error::ConfigError("payload claim content size exceeds SQLite range".to_string())
            })
        })
        .transpose()
}

fn validate_absolute_path(path: &str, authority: &str) -> Result<()> {
    let parsed = Path::new(path);
    if !parsed.is_absolute()
        || path == "/"
        || parsed.components().any(|component| {
            matches!(
                component,
                PathComponent::CurDir | PathComponent::ParentDir | PathComponent::Prefix(_)
            )
        })
    {
        return Err(Error::ConfigError(format!(
            "{authority} path must be an absolute normalized path below root: {path:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "payload_claim/tests.rs"]
mod tests;
