// conary-core/src/db/models/directory_claim.rs
//! Format-neutral many-to-many ownership for materialized directories.

use super::FileEntry;
use crate::error::{Error, Result};
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use rusqlite::{Connection, Row, Transaction, params, types::Type};
use serde::{Deserialize, Serialize};
use std::path::{Component as PathComponent, Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DirectoryClaimAnchorPolicy {
    Directory,
    DirectoryOrSymlinkToDirectory,
}

impl DirectoryClaimAnchorPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::DirectoryOrSymlinkToDirectory => "directory-or-symlink-to-directory",
        }
    }

    fn from_str(value: &str) -> rusqlite::Result<Self> {
        match value {
            "directory" => Ok(Self::Directory),
            "directory-or-symlink-to-directory" => Ok(Self::DirectoryOrSymlinkToDirectory),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                3,
                Type::Text,
                format!("unknown directory claim anchor policy {other:?}").into(),
            )),
        }
    }

    pub const fn accepts(self, kind: &PayloadNodeKind) -> bool {
        matches!(kind, PayloadNodeKind::Directory)
            || (matches!(self, Self::DirectoryOrSymlinkToDirectory)
                && matches!(kind, PayloadNodeKind::Symlink { .. }))
    }
}

/// One package's exact claim on a shared materialized directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryClaim {
    pub path: String,
    pub trove_id: i64,
    pub component_id: Option<i64>,
    pub anchor_policy: DirectoryClaimAnchorPolicy,
    pub materialization_target_path: Option<String>,
    pub node: ResolvedPayloadNode,
    pub claimed_at: Option<String>,
}

impl DirectoryClaim {
    const COLUMNS: &'static str = "path, trove_id, component_id, anchor_policy, \
         materialization_target_path, payload_node_json, claimed_at";

    pub fn new(path: String, trove_id: i64, node: ResolvedPayloadNode) -> Result<Self> {
        let claim = Self {
            path,
            trove_id,
            component_id: None,
            anchor_policy: DirectoryClaimAnchorPolicy::Directory,
            materialization_target_path: None,
            node,
            claimed_at: None,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn with_component(mut self, component_id: Option<i64>) -> Self {
        self.component_id = component_id;
        self
    }

    pub fn with_anchor_policy(mut self, anchor_policy: DirectoryClaimAnchorPolicy) -> Self {
        self.anchor_policy = anchor_policy;
        self
    }

    pub fn with_materialization_target(mut self, target_path: Option<String>) -> Self {
        self.materialization_target_path = target_path;
        self
    }

    /// Persist one package's independent exact directory claim.
    ///
    /// Claim metadata need not match the current materialized node. Native
    /// package managers permit multiple packages to list the same directory,
    /// and the latest unpack may change that directory's metadata.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        let anchor = FileEntry::find_by_path(conn, &self.path)?.ok_or_else(|| {
            Error::ConfigError(format!(
                "directory claim {} has no materialized file anchor",
                self.path
            ))
        })?;
        if !self.anchor_policy.accepts(&anchor.node.source.kind) {
            return Err(Error::ConfigError(format!(
                "directory claim {} anchor kind is not accepted by policy '{}'",
                self.path,
                self.anchor_policy.as_str()
            )));
        }
        if let Some(target_path) = self.materialization_target_path.as_deref() {
            let target = FileEntry::find_by_path(conn, target_path)?.ok_or_else(|| {
                Error::ConfigError(format!(
                    "directory claim {} materialization target {} has no file anchor",
                    self.path, target_path
                ))
            })?;
            if !matches!(anchor.node.source.kind, PayloadNodeKind::Symlink { .. })
                || !matches!(target.node.source.kind, PayloadNodeKind::Directory)
            {
                return Err(Error::ConfigError(format!(
                    "directory claim {} materialization target {} is not a typed symlink-to-directory edge",
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
                    "directory claim {} component {} does not belong to trove {}",
                    self.path, component_id, self.trove_id
                )));
            }
        }

        let node_json = super::file_entry::canonical_node_json(&self.node)?;
        conn.execute(
            "INSERT INTO directory_claims (
                path, trove_id, component_id, anchor_policy,
                materialization_target_path, payload_node_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(path, trove_id) DO UPDATE SET
                component_id = excluded.component_id,
                anchor_policy = excluded.anchor_policy,
                materialization_target_path = excluded.materialization_target_path,
                payload_node_json = excluded.payload_node_json",
            params![
                &self.path,
                self.trove_id,
                self.component_id,
                self.anchor_policy.as_str(),
                &self.materialization_target_path,
                node_json
            ],
        )?;
        Ok(())
    }

    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM directory_claims WHERE path = ?1 ORDER BY trove_id",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([path], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM directory_claims WHERE trove_id = ?1 ORDER BY path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return every exact package claim that retains one materialized path.
    ///
    /// A claim retains its declared path directly. RPM may additionally carry
    /// one parsed ApplyThroughSymlink target, which retains that target
    /// directory without reclassifying it as a package-declared path.
    pub fn find_retaining_path(conn: &Connection, path: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM directory_claims
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
            .ok_or_else(|| {
                Error::NotFound(format!("directory claim {path} for trove {trove_id}"))
            })?;
        claim.materialization_target_path = Some(target_path.to_string());
        claim.insert(conn)
    }

    /// Delete one package's directory claim inside a caller-owned transaction.
    ///
    /// Removing an anchor claim first transfers the canonical `files` row to
    /// the lowest remaining claimant without rewriting its payload node. The
    /// last claim deletes the materialized anchor as one atomic transaction.
    pub fn delete(tx: &Transaction<'_>, path: &str, trove_id: i64) -> Result<()> {
        let claims = Self::find_by_path(tx, path)?;
        if !claims.iter().any(|claim| claim.trove_id == trove_id) {
            return Err(Error::NotFound(format!(
                "directory claim {path} for trove {trove_id}"
            )));
        }
        let retainers = Self::find_retaining_path(tx, path)?;
        let anchor = FileEntry::find_by_path(tx, path)?.ok_or_else(|| {
            Error::ConfigError(format!(
                "directory claim {path} has no materialized file anchor"
            ))
        })?;

        if anchor.trove_id == trove_id {
            let next_owner = retainers
                .iter()
                .filter(|claim| !(claim.path == path && claim.trove_id == trove_id))
                .min_by_key(|claim| (claim.trove_id, claim.path.as_str()));
            if let Some(next_owner) = next_owner {
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
                return FileEntry::delete(tx, path);
            }
        }
        let deleted = tx.execute(
            "DELETE FROM directory_claims WHERE path = ?1 AND trove_id = ?2",
            params![path, trove_id],
        )?;
        if deleted != 1 {
            return Err(Error::InternalError(format!(
                "directory claim {path} for trove {trove_id} was not deleted exactly"
            )));
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        validate_absolute_path(&self.path, "directory claim")?;
        if let Some(target_path) = self.materialization_target_path.as_deref() {
            validate_absolute_path(target_path, "directory materialization target")?;
            if target_path == self.path {
                return Err(Error::ConfigError(format!(
                    "directory claim {} cannot target itself",
                    self.path
                )));
            }
        }
        self.node.validate().map_err(|error| {
            Error::ConfigError(format!(
                "directory claim {} has invalid payload authority: {error}",
                self.path
            ))
        })?;
        if !matches!(self.node.source.kind, PayloadNodeKind::Directory) {
            return Err(Error::ConfigError(format!(
                "directory claim {} must carry a directory payload node",
                self.path
            )));
        }
        Ok(())
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let anchor_policy =
            DirectoryClaimAnchorPolicy::from_str(row.get::<_, String>(3)?.as_str())?;
        let node_json: String = row.get(5)?;
        let node = serde_json::from_str::<ResolvedPayloadNode>(&node_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(5, Type::Text, error.into())
        })?;
        node.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(5, Type::Text, error.into())
        })?;
        if !matches!(node.source.kind, PayloadNodeKind::Directory) {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                Type::Text,
                "directory claim row does not carry a directory payload node".into(),
            ));
        }
        Ok(Self {
            path: row.get(0)?,
            trove_id: row.get(1)?,
            component_id: row.get(2)?,
            anchor_policy,
            materialization_target_path: row.get(4)?,
            node,
            claimed_at: row.get(6)?,
        })
    }
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
#[path = "directory_claim/tests.rs"]
mod tests;
