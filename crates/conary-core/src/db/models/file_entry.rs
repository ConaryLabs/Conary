// conary-core/src/db/models/file_entry.rs

//! Exact installed payload-node state.

use crate::error::{Error, Result};
use crate::payload::{
    PayloadContentAuthority, PayloadNodeKind, PayloadSharingPolicy, ResolvedPayloadNode,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, types::Type};

/// One package-owned node in the selected filesystem.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: Option<i64>,
    pub path: String,
    pub node: ResolvedPayloadNode,
    pub content: Option<PayloadContentAuthority>,
    pub trove_id: i64,
    pub installed_at: Option<String>,
    pub component_id: Option<i64>,
    /// Source policy used only when this entry creates or joins a claim set.
    ///
    /// The durable copy lives on each `payload_claims` row; `files` remains
    /// only the selected-root materialization anchor.
    pub claim_policy: PayloadSharingPolicy,
}

impl PartialEq for FileEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.path == other.path
            && self.node == other.node
            && self.content == other.content
            && self.trove_id == other.trove_id
            && self.installed_at == other.installed_at
            && self.component_id == other.component_id
    }
}

impl Eq for FileEntry {}

/// Source-format behavior when a directory already exists at install time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingDirectoryMaterialization {
    /// Apply the incoming directory node and persist it as materialized state.
    ApplyIncoming,
    /// Apply the incoming directory node while retaining source-format
    /// authority to anchor the claim on a verified symlink-to-directory.
    ApplyIncomingDirectoryOrSymlinkToDirectory,
    /// Record the claim while retaining a real existing directory.
    PreserveExistingDirectory,
    /// Record the claim while retaining either a real directory or a safely
    /// proven selected-root symlink whose target is a directory.
    PreserveExistingDirectoryOrSymlinkToDirectory,
}

impl ExistingDirectoryMaterialization {
    const fn anchor_policy(self) -> super::PayloadClaimAnchorPolicy {
        match self {
            Self::ApplyIncoming | Self::PreserveExistingDirectory => {
                super::PayloadClaimAnchorPolicy::Directory
            }
            Self::ApplyIncomingDirectoryOrSymlinkToDirectory
            | Self::PreserveExistingDirectoryOrSymlinkToDirectory => {
                super::PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
            }
        }
    }

    pub const fn accepts_anchor_kind(self, kind: &PayloadNodeKind) -> bool {
        self.anchor_policy().accepts_kind(kind)
    }

    pub const fn applies_incoming(self) -> bool {
        matches!(
            self,
            Self::ApplyIncoming | Self::ApplyIncomingDirectoryOrSymlinkToDirectory
        )
    }
}

impl FileEntry {
    const COLUMNS: &'static str = "id, path, payload_node_json, content_sha256, \
         content_size, trove_id, installed_at, component_id";

    pub fn new(
        path: String,
        node: ResolvedPayloadNode,
        content: Option<PayloadContentAuthority>,
        trove_id: i64,
    ) -> Self {
        Self {
            id: None,
            path,
            node,
            content,
            trove_id,
            installed_at: None,
            component_id: None,
            claim_policy: PayloadSharingPolicy::Exclusive,
        }
    }

    #[must_use]
    pub fn with_claim_policy(mut self, claim_policy: PayloadSharingPolicy) -> Self {
        self.claim_policy = claim_policy;
        self
    }

    pub fn new_with_component(
        path: String,
        node: ResolvedPayloadNode,
        content: Option<PayloadContentAuthority>,
        trove_id: i64,
        component_id: i64,
    ) -> Self {
        Self {
            component_id: Some(component_id),
            ..Self::new(path, node, content, trove_id)
        }
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate()?;
        let id = with_file_entry_savepoint(conn, || {
            let id = self.insert_row(conn, false)?;
            self.insert_payload_claim(conn, ExistingDirectoryMaterialization::ApplyIncoming)?;
            Ok(id)
        })?;
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or replace this path's exact installed authority.
    pub fn insert_or_replace(
        &mut self,
        conn: &Connection,
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<i64> {
        self.insert_or_replace_inner(conn, directory_materialization, false)
    }

    /// Insert or share exact payload authority inside a caller-owned savepoint.
    ///
    /// Bulk import uses one savepoint per package. Avoiding another savepoint
    /// per file preserves the same package-atomic rollback boundary without
    /// issuing hundreds of thousands of redundant SQLite control statements.
    pub fn insert_or_replace_in_savepoint(
        &mut self,
        conn: &Connection,
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<i64> {
        self.insert_or_replace_inner(conn, directory_materialization, true)
    }

    fn insert_or_replace_inner(
        &mut self,
        conn: &Connection,
        directory_materialization: ExistingDirectoryMaterialization,
        caller_owns_savepoint: bool,
    ) -> Result<i64> {
        self.validate()?;
        let existing = Self::find_by_path(conn, &self.path)?;
        if let Some(existing) = existing.as_ref() {
            let existing_id = existing.id.ok_or_else(|| {
                Error::InternalError(format!(
                    "persisted file entry {} has no database identity",
                    self.path
                ))
            })?;
            if self.can_share_existing_with(conn, existing, directory_materialization)? {
                let incoming_claim = self.payload_claim(directory_materialization)?;
                with_optional_file_entry_savepoint(conn, caller_owns_savepoint, || {
                    incoming_claim.insert(conn)?;
                    if matches!(self.node.source.kind, PayloadNodeKind::Directory)
                        && directory_materialization.applies_incoming()
                    {
                        let node_json = canonical_node_json(&self.node)?;
                        conn.execute(
                            "UPDATE files
                             SET payload_node_json = ?1,
                                 content_sha256 = NULL,
                                 content_size = NULL
                             WHERE path = ?2",
                            params![node_json, &self.path],
                        )?;
                    }
                    Ok(())
                })?;
                self.id = Some(existing_id);
                return Ok(existing_id);
            }
        }
        if let Some(existing) = existing
            && super::PayloadClaim::find_retaining_path(conn, &self.path)?
                .iter()
                .any(|claim| claim.trove_id != existing.trove_id)
        {
            return Err(Error::ConflictError(format!(
                "cannot replace shared payload materialization {} while peer claims remain",
                self.path
            )));
        }
        let id = with_optional_file_entry_savepoint(conn, caller_owns_savepoint, || {
            let id = self.insert_row(conn, true)?;
            self.payload_claim(directory_materialization)?
                .insert_for_new_anchor(conn)?;
            Ok(id)
        })?;
        self.id = Some(id);
        Ok(id)
    }

    pub fn can_share_existing(
        &self,
        conn: &Connection,
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<bool> {
        let Some(existing) = Self::find_by_path(conn, &self.path)? else {
            return Ok(false);
        };
        self.can_share_existing_with(conn, &existing, directory_materialization)
    }

    fn can_share_existing_with(
        &self,
        conn: &Connection,
        existing: &Self,
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<bool> {
        let direct_claims = super::PayloadClaim::find_by_path(conn, &self.path)?;
        if direct_claims.is_empty() {
            return Err(Error::ConfigError(format!(
                "materialized payload {} has no package claim",
                self.path
            )));
        }
        if matches!(self.node.source.kind, PayloadNodeKind::Directory)
            && (matches!(existing.node.source.kind, PayloadNodeKind::Directory)
                || directory_materialization.accepts_anchor_kind(&existing.node.source.kind))
        {
            return Ok(true);
        }
        let incoming_claim = self.payload_claim(directory_materialization)?;
        Ok(direct_claims
            .iter()
            .all(|claim| incoming_claim.compatible_with(claim).is_ok()))
    }

    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM files WHERE path = ?1", Self::COLUMNS);
        conn.prepare_cached(&sql)?
            .query_row([path], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Reconcile a preserved directory materialization to the selected-root
    /// publication authority without changing package claims or anchor owner.
    pub fn reconcile_directory_materialization(
        tx: &Transaction<'_>,
        path: &str,
        node: &ResolvedPayloadNode,
        materialization: ExistingDirectoryMaterialization,
    ) -> Result<()> {
        node.validate().map_err(|error| {
            Error::ParseError(format!(
                "selected-root directory materialization {path} is invalid: {error}"
            ))
        })?;
        let anchor_policy = materialization.anchor_policy();
        if !anchor_policy.accepts_kind(&node.source.kind) {
            return Err(Error::ConfigError(format!(
                "selected-root node {path} is not accepted by directory materialization policy '{}'",
                anchor_policy.as_str()
            )));
        }
        Self::reconcile_selected_root_materialization(tx, path, node, None)?;
        Ok(())
    }

    /// Reconcile one tracked path to exact selected-root materialization
    /// without changing its package owner, component, timestamp, or directory
    /// claim graph.
    ///
    /// This is used when one complete root scan establishes hardlink topology
    /// and node/content authority across package ownership boundaries.
    pub fn reconcile_selected_root_materialization(
        conn: &Connection,
        path: &str,
        node: &ResolvedPayloadNode,
        content: Option<&PayloadContentAuthority>,
    ) -> Result<bool> {
        let candidate = Self::new(path.to_string(), node.clone(), content.cloned(), i64::MIN);
        candidate.validate()?;
        let existing = Self::find_by_path(conn, path)?.ok_or_else(|| {
            Error::NotFound(format!(
                "selected-root materialization {path} has no files anchor"
            ))
        })?;
        if existing.node == *node && existing.content.as_ref() == content {
            return Ok(false);
        }
        for claim in super::PayloadClaim::find_retaining_path(conn, path)? {
            if claim.path == path {
                claim.validate_anchor(&candidate)?;
            } else if !matches!(node.source.kind, PayloadNodeKind::Directory) {
                return Err(Error::ConflictError(format!(
                    "selected-root node {path} is incompatible with claim {} anchor policy '{}'",
                    claim.trove_id,
                    claim.anchor_policy.as_str()
                )));
            }
        }
        let node_json = canonical_node_json(node)?;
        let content_size = persisted_content_size(content)?;
        let updated = conn.execute(
            "UPDATE files
             SET payload_node_json = ?1, content_sha256 = ?2, content_size = ?3
             WHERE id = ?4",
            params![
                node_json,
                content.map(|authority| &authority.sha256),
                content_size,
                existing.id
            ],
        )?;
        if updated != 1 {
            return Err(Error::InternalError(format!(
                "selected-root materialization {path} was not reconciled exactly"
            )));
        }
        Ok(true)
    }

    /// Replace one package's claim and the selected-root materialization
    /// together without changing the anchor row's owner or history.
    ///
    /// Full adoption uses this when a track-only package acquires exact
    /// captured payload authority. Peer claims must independently admit the
    /// replacement before either durable authority is changed.
    pub fn replace_claimed_selected_root_materialization(
        conn: &Connection,
        path: &str,
        claimant_trove_id: i64,
        node: &ResolvedPayloadNode,
        content: Option<&PayloadContentAuthority>,
    ) -> Result<bool> {
        let candidate = Self::new(
            path.to_string(),
            node.clone(),
            content.cloned(),
            claimant_trove_id,
        );
        candidate.validate()?;
        let existing = Self::find_by_path(conn, path)?.ok_or_else(|| {
            Error::NotFound(format!(
                "selected-root materialization {path} has no files anchor"
            ))
        })?;
        let mut replacement_claim = super::PayloadClaim::find_by_path(conn, path)?
            .into_iter()
            .find(|claim| claim.trove_id == claimant_trove_id)
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "payload claim {path} for trove {claimant_trove_id}"
                ))
            })?;
        let claim_changed =
            replacement_claim.node != *node || replacement_claim.content.as_ref() != content;
        replacement_claim.node = node.clone();
        replacement_claim.content = content.cloned();
        replacement_claim.validate()?;
        replacement_claim.validate_anchor(&candidate)?;

        for peer in super::PayloadClaim::find_retaining_path(conn, path)?
            .into_iter()
            .filter(|claim| claim.path != path || claim.trove_id != replacement_claim.trove_id)
        {
            if peer.path == path {
                replacement_claim
                    .compatible_with(&peer)
                    .map_err(|mismatch| {
                        Error::ConflictError(format!(
                            "replacement payload claim {path} from trove {claimant_trove_id} is incompatible with trove {}: {mismatch}",
                            peer.trove_id
                        ))
                    })?;
                peer.validate_anchor(&candidate)?;
            } else if !matches!(node.source.kind, PayloadNodeKind::Directory) {
                return Err(Error::ConflictError(format!(
                    "selected-root node {path} is incompatible with claim {} anchor policy '{}'",
                    peer.trove_id,
                    peer.anchor_policy.as_str()
                )));
            }
        }

        let materialization_changed =
            existing.node != *node || existing.content.as_ref() != content;
        if !materialization_changed && !claim_changed {
            return Ok(false);
        }
        let existing_id = existing.id.ok_or_else(|| {
            Error::MissingId(format!(
                "selected-root materialization {path} has no database identity"
            ))
        })?;
        with_file_entry_savepoint(conn, || {
            let node_json = canonical_node_json(node)?;
            let content_size = persisted_content_size(content)?;
            let updated = conn.execute(
                "UPDATE files
                 SET payload_node_json = ?1, content_sha256 = ?2, content_size = ?3
                 WHERE id = ?4",
                params![
                    node_json,
                    content.map(|authority| &authority.sha256),
                    content_size,
                    existing_id
                ],
            )?;
            if updated != 1 {
                return Err(Error::InternalError(format!(
                    "selected-root materialization {path} was not replaced exactly"
                )));
            }
            replacement_claim.insert(conn)
        })?;
        Ok(true)
    }

    /// Canonicalize one materialized hardlink group around its target path.
    ///
    /// Source claims retain their native archive identities. The selected-root
    /// anchor graph instead uses a path-derived identity so independently
    /// converted packages that extend the same compatible RPM inode group
    /// converge on one deterministic physical topology.
    pub fn reconcile_hardlink_materialization(conn: &Connection, target_path: &str) -> Result<()> {
        let target = Self::find_by_path(conn, target_path)?.ok_or_else(|| {
            Error::NotFound(format!(
                "hardlink materialization target {target_path} has no files anchor"
            ))
        })?;
        if !matches!(target.node.source.kind, PayloadNodeKind::Regular { .. }) {
            return Err(Error::ConflictError(format!(
                "hardlink materialization target {target_path} is not a regular file"
            )));
        }

        let identity = format!("path:{target_path}");
        let mut target_node = target.node.clone();
        target_node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone()),
        };
        Self::reconcile_selected_root_materialization(
            conn,
            target_path,
            &target_node,
            target.content.as_ref(),
        )?;

        let linked = Self::find_all_ordered(conn)?
            .into_iter()
            .filter(|entry| {
                matches!(
                    &entry.node.source.kind,
                    PayloadNodeKind::Hardlink { target, .. } if target == target_path
                )
            })
            .collect::<Vec<_>>();
        if linked.is_empty() {
            return Err(Error::ConfigError(format!(
                "hardlink materialization target {target_path} has no linked file anchor"
            )));
        }
        for entry in linked {
            let mut node = target_node.clone();
            node.source.kind = PayloadNodeKind::Hardlink {
                target: target_path.to_string(),
                identity: identity.clone(),
            };
            Self::reconcile_selected_root_materialization(conn, &entry.path, &node, None)?;
        }
        Ok(())
    }

    /// Restore one exact materialized directory anchor inside a caller-owned
    /// transaction.
    ///
    /// Rollback restores anchors before the global claim graph, so this API
    /// deliberately does not fabricate a claim from the materialized node.
    /// The caller must restore the captured independent claim set before
    /// committing the transaction.
    pub fn restore_materialized_anchor(
        tx: &Transaction<'_>,
        path: &str,
        node: &ResolvedPayloadNode,
        content: Option<&PayloadContentAuthority>,
        trove_id: i64,
        component_id: Option<i64>,
        installed_at: &str,
    ) -> Result<i64> {
        node.validate()
            .and_then(|()| node.source.validate_content(content))
            .map_err(|error| {
                Error::ParseError(format!(
                    "restored payload anchor {path} has invalid authority: {error}"
                ))
            })?;
        if installed_at.is_empty() {
            return Err(Error::ConfigError(format!(
                "restored payload anchor {path} has no installed-at authority"
            )));
        }
        let owner_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM troves WHERE id = ?1)",
            [trove_id],
            |row| row.get(0),
        )?;
        if !owner_exists {
            return Err(Error::ConfigError(format!(
                "restored payload anchor {path} references missing trove {trove_id}"
            )));
        }
        validate_component_owner(tx, path, trove_id, component_id)?;

        let node_json = canonical_node_json(node)?;
        let content_size = persisted_content_size(content)?;
        if let Some(existing) = Self::find_by_path(tx, path)? {
            if existing.node != *node || existing.content.as_ref() != content {
                return Err(Error::ConflictError(format!(
                    "cannot restore payload anchor {path} over a different materialization"
                )));
            }
            let id = existing.id.ok_or_else(|| {
                Error::MissingId(format!(
                    "restored payload anchor {path} has no database identity"
                ))
            })?;
            tx.execute(
                "UPDATE files
                 SET payload_node_json = ?1, content_sha256 = ?2,
                     content_size = ?3, trove_id = ?4, component_id = ?5,
                     installed_at = ?6
                 WHERE id = ?7",
                params![
                    node_json,
                    content.map(|authority| &authority.sha256),
                    content_size,
                    trove_id,
                    component_id,
                    installed_at,
                    id
                ],
            )?;
            Ok(id)
        } else {
            tx.execute(
                "INSERT INTO files (
                    path, payload_node_json, content_sha256, content_size,
                    trove_id, installed_at, component_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    path,
                    node_json,
                    content.map(|authority| &authority.sha256),
                    content_size,
                    trove_id,
                    installed_at,
                    component_id
                ],
            )?;
            Ok(tx.last_insert_rowid())
        }
    }

    /// Restore a captured directory or verified symlink-to-directory
    /// materialization without fabricating package ownership.
    ///
    /// Directory metadata is mutable under the source package ABI, so rollback
    /// may intentionally replace the current materialized node before the
    /// independent payload claim graph is restored.
    pub fn restore_materialized_directory_anchor(
        tx: &Transaction<'_>,
        path: &str,
        node: &ResolvedPayloadNode,
        anchor_policy: super::PayloadClaimAnchorPolicy,
        trove_id: i64,
        component_id: Option<i64>,
        installed_at: &str,
    ) -> Result<i64> {
        node.validate().map_err(|error| {
            Error::ParseError(format!(
                "restored directory anchor {path} has invalid payload authority: {error}"
            ))
        })?;
        if !anchor_policy.accepts_kind(&node.source.kind) {
            return Err(Error::ConfigError(format!(
                "restored directory anchor {path} is incompatible with policy '{}'",
                anchor_policy.as_str()
            )));
        }
        if installed_at.is_empty() {
            return Err(Error::ConfigError(format!(
                "restored directory anchor {path} has no installed-at authority"
            )));
        }
        let owner_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM troves WHERE id = ?1)",
            [trove_id],
            |row| row.get(0),
        )?;
        if !owner_exists {
            return Err(Error::ConfigError(format!(
                "restored directory anchor {path} references missing trove {trove_id}"
            )));
        }
        validate_component_owner(tx, path, trove_id, component_id)?;
        let node_json = canonical_node_json(node)?;
        if let Some(existing) = Self::find_by_path(tx, path)? {
            if !matches!(
                existing.node.source.kind,
                PayloadNodeKind::Directory | PayloadNodeKind::Symlink { .. }
            ) {
                return Err(Error::ConflictError(format!(
                    "cannot restore directory anchor {path} over a non-directory materialization"
                )));
            }
            let id = existing.id.ok_or_else(|| {
                Error::MissingId(format!(
                    "restored directory anchor {path} has no database identity"
                ))
            })?;
            tx.execute(
                "UPDATE files
                 SET payload_node_json = ?1, content_sha256 = NULL,
                     content_size = NULL, trove_id = ?2, component_id = ?3,
                     installed_at = ?4
                 WHERE id = ?5",
                params![node_json, trove_id, component_id, installed_at, id],
            )?;
            Ok(id)
        } else {
            tx.execute(
                "INSERT INTO files (
                    path, payload_node_json, content_sha256, content_size,
                    trove_id, installed_at, component_id
                 ) VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5)",
                params![path, node_json, trove_id, installed_at, component_id],
            )?;
            Ok(tx.last_insert_rowid())
        }
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!("SELECT {} FROM files WHERE trove_id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_component(conn: &Connection, component_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM files WHERE component_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([component_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_all_ordered(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!("SELECT {} FROM files ORDER BY path", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_path_pattern(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM files WHERE path LIKE ?1 ORDER BY path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([pattern], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete(conn: &Connection, path: &str) -> Result<()> {
        if let Some(existing) = Self::find_by_path(conn, path)? {
            let claims = super::PayloadClaim::find_retaining_path(conn, path)?;
            if claims
                .iter()
                .any(|claim| claim.trove_id != existing.trove_id)
            {
                return Err(Error::ConflictError(format!(
                    "cannot delete payload materialization anchor {path} while peer package claims remain"
                )));
            }
        }
        conn.execute("DELETE FROM files WHERE path = ?1", [path])?;
        Ok(())
    }

    pub fn batch_insert(
        conn: &Connection,
        entries: &[Self],
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<usize> {
        for entry in entries {
            entry
                .clone()
                .insert_or_replace(conn, directory_materialization)?;
        }
        Ok(entries.len())
    }

    fn insert_payload_claim(
        &self,
        conn: &Connection,
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<()> {
        self.payload_claim(directory_materialization)?.insert(conn)
    }

    fn payload_claim(
        &self,
        directory_materialization: ExistingDirectoryMaterialization,
    ) -> Result<super::PayloadClaim> {
        let claim = super::PayloadClaim::new(
            self.path.clone(),
            self.trove_id,
            self.node.clone(),
            self.content.clone(),
            self.claim_policy,
        )?
        .with_component(self.component_id);
        Ok(
            if matches!(self.node.source.kind, PayloadNodeKind::Directory) {
                claim.with_anchor_policy(directory_materialization.anchor_policy())
            } else {
                claim
            },
        )
    }

    fn insert_row(&self, conn: &Connection, replace: bool) -> Result<i64> {
        let node_json = canonical_node_json(&self.node)?;
        let content_size = persisted_content_size(self.content.as_ref())?;
        let operation = if replace {
            "INSERT OR REPLACE INTO files (
                path, payload_node_json, content_sha256, content_size,
                trove_id, component_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        } else {
            "INSERT INTO files (
                path, payload_node_json, content_sha256, content_size,
                trove_id, component_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        };
        conn.prepare_cached(operation)?.execute(params![
            &self.path,
            node_json,
            self.content.as_ref().map(|content| &content.sha256),
            content_size,
            self.trove_id,
            self.component_id,
        ])?;
        Ok(conn.last_insert_rowid())
    }

    fn validate(&self) -> Result<()> {
        self.node
            .validate()
            .and_then(|()| self.node.source.validate_content(self.content.as_ref()))
            .map_err(|error| {
                Error::ParseError(format!(
                    "installed payload authority for {} is invalid: {error}",
                    self.path
                ))
            })
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let node_json: String = row.get(2)?;
        let node = serde_json::from_str::<ResolvedPayloadNode>(&node_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(2, Type::Text, error.into())
        })?;
        node.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(2, Type::Text, error.into())
        })?;
        let sha256: Option<String> = row.get(3)?;
        let size: Option<i64> = row.get(4)?;
        let content = match (sha256, size) {
            (Some(sha256), Some(size)) => Some(PayloadContentAuthority {
                sha256,
                size: u64::try_from(size).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(4, Type::Integer, error.into())
                })?,
            }),
            (None, None) => None,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    3,
                    Type::Text,
                    "installed content digest and size must both be NULL or both be present".into(),
                ));
            }
        };
        node.source
            .validate_content(content.as_ref())
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(3, Type::Text, error.into())
            })?;
        Ok(Self {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            node,
            content,
            trove_id: row.get(5)?,
            installed_at: row.get(6)?,
            component_id: row.get(7)?,
            claim_policy: PayloadSharingPolicy::Exclusive,
        })
    }
}

fn with_file_entry_savepoint<T>(
    conn: &Connection,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    const SAVEPOINT: &str = "conary_file_entry_payload_authority";
    conn.execute_batch(&format!("SAVEPOINT {SAVEPOINT}"))?;
    match operation() {
        Ok(value) => {
            conn.execute_batch(&format!("RELEASE {SAVEPOINT}"))?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch(&format!("ROLLBACK TO {SAVEPOINT}"));
            let _ = conn.execute_batch(&format!("RELEASE {SAVEPOINT}"));
            Err(error)
        }
    }
}

fn with_optional_file_entry_savepoint<T>(
    conn: &Connection,
    caller_owns_savepoint: bool,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if caller_owns_savepoint {
        operation()
    } else {
        with_file_entry_savepoint(conn, operation)
    }
}

fn validate_component_owner(
    conn: &Connection,
    path: &str,
    trove_id: i64,
    component_id: Option<i64>,
) -> Result<()> {
    let Some(component_id) = component_id else {
        return Ok(());
    };
    let component_matches_trove: bool = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM components
            WHERE id = ?1 AND parent_trove_id = ?2
        )",
        params![component_id, trove_id],
        |row| row.get(0),
    )?;
    if !component_matches_trove {
        return Err(Error::ConfigError(format!(
            "file authority {path} component {component_id} does not belong to trove {trove_id}"
        )));
    }
    Ok(())
}

pub(super) fn canonical_node_json(node: &ResolvedPayloadNode) -> Result<String> {
    let bytes = crate::ccs::attestation::canonical_json_bytes(node).map_err(|error| {
        Error::InternalError(format!("canonicalize installed payload node: {error}"))
    })?;
    String::from_utf8(bytes).map_err(|error| {
        Error::InternalError(format!("canonical payload node is not UTF-8: {error}"))
    })
}

fn persisted_content_size(content: Option<&PayloadContentAuthority>) -> Result<Option<i64>> {
    content
        .map(|content| {
            i64::try_from(content.size).map_err(|_| {
                Error::ParseError("payload content size exceeds SQLite INTEGER".to_string())
            })
        })
        .transpose()
}
