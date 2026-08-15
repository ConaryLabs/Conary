// conary-core/src/db/models/package_transaction_staging/planning.rs

//! Typed staging validation and canonical-anchor decisions.

use super::{StagedAnchorDisposition, StagedPayloadRow};
use crate::db::models::{ExistingDirectoryMaterialization, PayloadClaimAnchorPolicy};
use crate::error::{Error, Result};
use crate::payload::{PayloadContentAuthority, PayloadNodeKind, ResolvedPayloadNode};

#[derive(Debug, Clone)]
pub(super) struct LoadedClaim {
    pub(super) claim: crate::db::models::PayloadClaim,
    pub(super) owner_name: String,
    pub(super) owner_install_source: String,
    pub(super) replacement_allowed: bool,
}

#[derive(Debug)]
pub(super) struct StagedClaimInput {
    pub(super) path: String,
    pub(super) trove_id: i64,
    pub(super) component_id: Option<i64>,
    pub(super) sharing_policy: String,
    pub(super) anchor_policy: String,
    pub(super) materialization_target_path: Option<String>,
    pub(super) node_json: String,
    pub(super) content_sha256: Option<String>,
    pub(super) content_size: Option<i64>,
    pub(super) owner_name: String,
    pub(super) owner_install_source: String,
    pub(super) replacement_allowed: bool,
}

#[derive(Debug)]
pub(super) struct StagedAnchorInput {
    pub(super) path: String,
    pub(super) trove_id: i64,
    pub(super) package_name: String,
    pub(super) incoming_node_json: String,
    pub(super) incoming_sha256: Option<String>,
    pub(super) incoming_size: Option<i64>,
    pub(super) incoming_policy: String,
    pub(super) anchor_policy: String,
    pub(super) disposition: String,
    pub(super) selected_root_node_json: Option<String>,
    pub(super) existing_id: Option<i64>,
    pub(super) existing_node_json: Option<String>,
    pub(super) existing_sha256: Option<String>,
    pub(super) existing_size: Option<i64>,
    pub(super) existing_trove_id: Option<i64>,
    pub(super) existing_owner_name: Option<String>,
    pub(super) existing_install_source: Option<String>,
}

#[derive(Debug)]
pub(super) struct PlannedDecision {
    pub(super) path: String,
    pub(super) trove_id: i64,
    pub(super) anchor_action: &'static str,
    pub(super) materialized: bool,
}

impl PlannedDecision {
    pub(super) fn replace(path: String, trove_id: i64) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "replace",
            materialized: true,
        }
    }

    pub(super) fn keep(path: String, trove_id: i64, materialized: bool) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "keep",
            materialized,
        }
    }

    pub(super) fn replace_without_materialization(path: String, trove_id: i64) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "replace",
            materialized: false,
        }
    }

    pub(super) fn insert_selected_root(path: String, trove_id: i64) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "insert-selected-root",
            materialized: false,
        }
    }

    pub(super) fn apply_directory(path: String, trove_id: i64) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "apply-directory",
            materialized: true,
        }
    }

    pub(super) fn reconcile_materialization(path: String, trove_id: i64) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "reconcile-materialization",
            materialized: false,
        }
    }

    pub(super) fn reconcile_owned(path: String, trove_id: i64) -> Self {
        Self {
            path,
            trove_id,
            anchor_action: "reconcile-owned",
            materialized: false,
        }
    }
}

pub(super) fn validate_staged_payload(row: &StagedPayloadRow) -> Result<()> {
    row.entry
        .node
        .validate()
        .and_then(|()| {
            row.entry
                .node
                .source
                .validate_content(row.entry.content.as_ref())
        })
        .map_err(|error| {
            Error::ParseError(format!(
                "staged payload authority for {} is invalid: {error}",
                row.entry.path
            ))
        })?;
    if let Some(selected_root_node) = row.selected_root_node.as_ref() {
        selected_root_node.validate().map_err(|error| {
            Error::ParseError(format!(
                "staged selected-root authority for {} is invalid: {error}",
                row.entry.path
            ))
        })?;
    }
    if let Some((_, action)) = row.history
        && !matches!(action, "add" | "modify")
    {
        return Err(Error::ConfigError(format!(
            "staged payload {} has invalid history action {action:?}",
            row.entry.path
        )));
    }
    Ok(())
}

pub(super) fn anchor_policy(
    materialization: ExistingDirectoryMaterialization,
    node: &ResolvedPayloadNode,
) -> PayloadClaimAnchorPolicy {
    if !matches!(node.source.kind, PayloadNodeKind::Directory) {
        return PayloadClaimAnchorPolicy::Exact;
    }
    match materialization {
        ExistingDirectoryMaterialization::ApplyIncoming
        | ExistingDirectoryMaterialization::PreserveExistingDirectory => {
            PayloadClaimAnchorPolicy::Directory
        }
        ExistingDirectoryMaterialization::ApplyIncomingDirectoryOrSymlinkToDirectory
        | ExistingDirectoryMaterialization::PreserveExistingDirectoryOrSymlinkToDirectory => {
            PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
        }
    }
}

pub(super) fn parse_anchor_policy(value: &str) -> Result<PayloadClaimAnchorPolicy> {
    match value {
        "exact" => Ok(PayloadClaimAnchorPolicy::Exact),
        "directory" => Ok(PayloadClaimAnchorPolicy::Directory),
        "directory-or-symlink-to-directory" => {
            Ok(PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        }
        other => Err(Error::ParseError(format!(
            "unknown staged payload anchor policy {other:?}"
        ))),
    }
}

pub(super) fn parse_disposition(value: &str) -> Result<StagedAnchorDisposition> {
    match value {
        "auto" => Ok(StagedAnchorDisposition::Auto),
        "replace" => Ok(StagedAnchorDisposition::Replace),
        "share" => Ok(StagedAnchorDisposition::Share),
        "apply-directory" => Ok(StagedAnchorDisposition::ApplyDirectory),
        "preserve-selected-root" => Ok(StagedAnchorDisposition::PreserveSelectedRoot),
        "reconcile-owned" => Ok(StagedAnchorDisposition::ReconcileOwned),
        "reconcile-captured-root" => Ok(StagedAnchorDisposition::ReconcileCapturedRoot),
        other => Err(Error::ParseError(format!(
            "unknown staged payload disposition {other:?}"
        ))),
    }
}

pub(super) fn parse_node(path: &str, json: &str) -> Result<ResolvedPayloadNode> {
    let node = serde_json::from_str::<ResolvedPayloadNode>(json).map_err(|error| {
        Error::ParseError(format!(
            "staged payload {path} has invalid node JSON: {error}"
        ))
    })?;
    node.validate().map_err(|error| {
        Error::ParseError(format!("staged payload {path} has invalid node: {error}"))
    })?;
    Ok(node)
}

pub(super) fn parse_content(
    path: &str,
    sha256: Option<String>,
    size: Option<i64>,
) -> Result<Option<PayloadContentAuthority>> {
    match (sha256, size) {
        (Some(sha256), Some(size)) => {
            let content = PayloadContentAuthority {
                sha256,
                size: u64::try_from(size).map_err(|_| {
                    Error::ParseError(format!("staged payload {path} has negative content size"))
                })?,
            };
            content.validate().map_err(|error| {
                Error::ParseError(format!(
                    "staged payload {path} has invalid content authority: {error}"
                ))
            })?;
            Ok(Some(content))
        }
        (None, None) => Ok(None),
        _ => Err(Error::ParseError(format!(
            "staged payload {path} has incomplete content authority"
        ))),
    }
}

pub(super) fn persisted_content_size(
    content: Option<&PayloadContentAuthority>,
) -> Result<Option<i64>> {
    content
        .map(|content| {
            i64::try_from(content.size).map_err(|_| {
                Error::ParseError("staged payload content size exceeds SQLite range".to_string())
            })
        })
        .transpose()
}
