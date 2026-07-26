// conary-core/src/db/models/package_payload_ownership.rs
//! Claim-aware installed package payload projection.

use super::{DirectoryClaim, FileEntry, Trove};
use crate::error::{Error, Result};
use crate::payload::{PayloadContentAuthority, PayloadNodeKind, ResolvedPayloadNode};
use rusqlite::Connection;
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet};

/// One installed trove's exact package-owned payload.
///
/// `files` stores one materialized anchor per path, while
/// `directory_claims` stores every package's independent directory node.
/// Package-facing consumers must use this projection so a non-anchor
/// directory never disappears from query, runtime, SBOM, derivation, or
/// lifecycle behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePayloadOwnership {
    entries: Vec<PackagePayloadEntry>,
    lifecycle_paths: Vec<String>,
    materialized_removal_paths: Vec<String>,
}

/// One package-declared payload node, independent of the materialized anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePayloadEntry {
    pub path: String,
    pub node: ResolvedPayloadNode,
    pub content: Option<PayloadContentAuthority>,
    pub trove_id: i64,
    pub installed_at: Option<String>,
    pub component_id: Option<i64>,
    /// Identity of the separate materialized `files` anchor, when present.
    pub materialized_file_id: Option<i64>,
}

impl PackagePayloadEntry {
    pub fn format_permissions(&self) -> String {
        let mode = self.node.source.mode;
        let file_type = match &self.node.source.kind {
            PayloadNodeKind::Regular { .. } => '-',
            PayloadNodeKind::Directory => 'd',
            PayloadNodeKind::Symlink { .. } => 'l',
            PayloadNodeKind::Hardlink { .. } => 'h',
            PayloadNodeKind::BlockDevice { .. } => 'b',
            PayloadNodeKind::CharacterDevice { .. } => 'c',
            PayloadNodeKind::Fifo => 'p',
            PayloadNodeKind::Socket => 's',
        };
        let bit = |mask, set, unset| if mode & mask != 0 { set } else { unset };
        let owner_x = if mode & 0o4000 != 0 {
            bit(0o100, 's', 'S')
        } else {
            bit(0o100, 'x', '-')
        };
        let group_x = if mode & 0o2000 != 0 {
            bit(0o010, 's', 'S')
        } else {
            bit(0o010, 'x', '-')
        };
        let other_x = if mode & 0o1000 != 0 {
            bit(0o001, 't', 'T')
        } else {
            bit(0o001, 'x', '-')
        };
        format!(
            "{}{}{}{}{}{}{}{}{}{}",
            file_type,
            bit(0o400, 'r', '-'),
            bit(0o200, 'w', '-'),
            owner_x,
            bit(0o040, 'r', '-'),
            bit(0o020, 'w', '-'),
            group_x,
            bit(0o004, 'r', '-'),
            bit(0o002, 'w', '-'),
            other_x
        )
    }

    pub fn size_human(&self) -> String {
        let size = self
            .content
            .as_ref()
            .and_then(|content| i64::try_from(content.size).ok())
            .unwrap_or(0);
        super::format_size(size)
    }
}

impl PackagePayloadOwnership {
    pub fn installed_paths_excluding(
        conn: &Connection,
        excluded_trove_ids: &HashSet<i64>,
    ) -> Result<BTreeSet<String>> {
        let mut paths = BTreeSet::new();
        for trove in Trove::list_packages(conn)? {
            let trove_id = trove.id.ok_or_else(|| {
                Error::MissingId(format!(
                    "installed package '{}' has no trove id",
                    trove.name
                ))
            })?;
            if !excluded_trove_ids.contains(&trove_id) {
                paths.extend(Self::load(conn, trove_id)?.lifecycle_paths);
            }
        }
        Ok(paths)
    }

    pub fn load(conn: &Connection, trove_id: i64) -> Result<Self> {
        let files = FileEntry::find_by_trove(conn, trove_id)?;
        let claims = DirectoryClaim::find_by_trove(conn, trove_id)?;
        let claims_by_path = claims
            .iter()
            .map(|claim| (claim.path.as_str(), claim))
            .collect::<BTreeMap<_, _>>();

        let mut entries = Vec::with_capacity(files.len() + claims.len());
        let mut included_paths = BTreeSet::new();
        let mut materialized_removal_paths = BTreeSet::new();

        for file in files {
            let path_retainers = DirectoryClaim::find_retaining_path(conn, &file.path)?;
            if let Some(claim) = claims_by_path.get(file.path.as_str()) {
                if !claim.anchor_policy.accepts(&file.node.source.kind) {
                    return Err(Error::ConfigError(format!(
                        "materialized node {} violates trove {trove_id} claim anchor policy '{}'",
                        file.path,
                        claim.anchor_policy.as_str()
                    )));
                }
                continue;
            }
            if matches!(file.node.source.kind, PayloadNodeKind::Directory) {
                let retained_by_package_target = claims.iter().any(|claim| {
                    claim.materialization_target_path.as_deref() == Some(file.path.as_str())
                });
                if !retained_by_package_target {
                    return Err(Error::ConfigError(format!(
                        "materialized directory {} owned by trove {trove_id} has no exact package claim or materialization-target authority",
                        file.path
                    )));
                }
                if path_retainers
                    .iter()
                    .all(|claim| claim.trove_id == trove_id)
                {
                    materialized_removal_paths.insert(file.path);
                }
                continue;
            }
            if path_retainers.is_empty() {
                materialized_removal_paths.insert(file.path.clone());
            }
            included_paths.insert(file.path.clone());
            entries.push(PackagePayloadEntry {
                path: file.path,
                node: file.node,
                content: file.content,
                trove_id: file.trove_id,
                installed_at: file.installed_at,
                component_id: file.component_id,
                materialized_file_id: file.id,
            });
        }

        for claim in claims {
            let anchor = FileEntry::find_by_path(conn, &claim.path)?.ok_or_else(|| {
                Error::ConfigError(format!(
                    "directory claim {} for trove {trove_id} has no materialized anchor",
                    claim.path
                ))
            })?;
            if !claim.anchor_policy.accepts(&anchor.node.source.kind) {
                return Err(Error::ConfigError(format!(
                    "directory claim {} for trove {trove_id} violates anchor policy '{}'",
                    claim.path,
                    claim.anchor_policy.as_str()
                )));
            }
            if DirectoryClaim::find_retaining_path(conn, &claim.path)?
                .iter()
                .all(|retainer| retainer.trove_id == trove_id)
                && anchor.trove_id == trove_id
            {
                materialized_removal_paths.insert(claim.path.clone());
            }
            if let Some(target_path) = claim.materialization_target_path.as_deref() {
                let target = FileEntry::find_by_path(conn, target_path)?.ok_or_else(|| {
                    Error::ConfigError(format!(
                        "directory claim {} for trove {trove_id} has no materialization target {}",
                        claim.path, target_path
                    ))
                })?;
                if !matches!(target.node.source.kind, PayloadNodeKind::Directory) {
                    return Err(Error::ConfigError(format!(
                        "directory claim {} for trove {trove_id} materialization target {} is not a directory",
                        claim.path, target_path
                    )));
                }
            }
            included_paths.insert(claim.path.clone());
            entries.push(PackagePayloadEntry {
                path: claim.path,
                node: claim.node,
                content: None,
                trove_id: claim.trove_id,
                installed_at: claim.claimed_at,
                component_id: claim.component_id,
                materialized_file_id: anchor.id,
            });
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let lifecycle_paths = included_paths.into_iter().collect();
        Ok(Self {
            entries,
            lifecycle_paths,
            materialized_removal_paths: materialized_removal_paths.into_iter().collect(),
        })
    }

    pub fn entries(&self) -> &[PackagePayloadEntry] {
        &self.entries
    }

    pub fn lifecycle_paths(&self) -> &[String] {
        &self.lifecycle_paths
    }

    pub fn materialized_removal_paths(&self) -> &[String] {
        &self.materialized_removal_paths
    }
}

#[cfg(test)]
#[path = "package_payload_ownership/tests.rs"]
mod tests;
