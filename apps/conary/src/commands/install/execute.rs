// src/commands/install/execute.rs
//! Transaction execution helpers - CAS storage and file tracking
//!
//! In the composefs-native model, files are stored in CAS and tracked in the DB.
//! Filesystem deployment happens via EROFS image build + composefs mount, not
//! direct file deployment. These helpers handle the CAS/DB side.

use super::ExtractionResult;
use super::inner;
use super::shared_directory::DirectoryInstallPlan;
use crate::commands::{LiveRootContent, LiveRootFile};
use anyhow::{Context, Result, bail};
use conary_core::db::models::FileEntry;
use conary_core::filesystem::CasStore;
use conary_core::packages::PackageFormat;
use conary_core::payload::PayloadNodeKind;
use conary_core::transaction::PackageRelationRemoval;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tracing::info;

pub(super) fn preflight_extracted_file_ownership(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    relation_removals: &[PackageRelationRemoval],
) -> Result<()> {
    inner::preflight_file_ownership(
        conn,
        extraction
            .extracted_files
            .iter()
            .map(|file| file.path.as_str()),
        pkg.name(),
        relation_removals,
    )
}

pub(super) fn live_root_files_from_stored_files(
    cas: &CasStore,
    stored_files: &[inner::ResolvedInstallFile],
) -> Result<Vec<LiveRootFile>> {
    stored_files
        .iter()
        .map(|file| {
            let content = match &file.node.source.kind {
                PayloadNodeKind::Regular { .. } => {
                    let authority = file.content.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("regular payload {} has no content authority", file.path)
                    })?;
                    let hash = file.cas_hash.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("regular payload {} has no CAS identity", file.path)
                    })?;
                    LiveRootContent::from_cas(cas, authority.clone(), hash)
                        .with_context(|| format!("Failed to open {} from CAS", file.path))?
                }
                PayloadNodeKind::Symlink { target } => {
                    let hash = file.cas_hash.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("symlink payload {} has no CAS identity", file.path)
                    })?;
                    let stored_target = cas.retrieve_symlink(hash).with_context(|| {
                        format!("Failed to read symlink {} from CAS", file.path)
                    })?;
                    if stored_target != *target {
                        anyhow::bail!(
                            "CAS symlink target mismatch for {}: expected {}, got {}",
                            file.path,
                            target,
                            stored_target
                        );
                    }
                    LiveRootContent::absent()
                }
                PayloadNodeKind::Directory
                | PayloadNodeKind::Hardlink { .. }
                | PayloadNodeKind::BlockDevice { .. }
                | PayloadNodeKind::CharacterDevice { .. }
                | PayloadNodeKind::Fifo
                | PayloadNodeKind::Socket => LiveRootContent::absent(),
            };
            Ok(LiveRootFile {
                path: file.path.clone(),
                content,
                node: file.node.clone(),
            })
        })
        .collect()
}

/// Bind hardlinks to compatible regular targets that this install preserves.
///
/// The incoming package retains its source-native claim graph. Root mutation
/// uses the existing materialized target's metadata and a deterministic
/// path-derived identity, while references participate in preflight without
/// rewriting the target inode.
pub(super) fn prepare_preserved_hardlink_references(
    conn: &rusqlite::Connection,
    directory_plan: &DirectoryInstallPlan,
    all_files: &[LiveRootFile],
    mutation_files: &mut [LiveRootFile],
) -> Result<Vec<LiveRootFile>> {
    let mutation_paths = mutation_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let incoming_by_path = all_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut references = BTreeMap::new();

    for file in mutation_files {
        let PayloadNodeKind::Hardlink { target, .. } = &file.node.source.kind else {
            continue;
        };
        let target = target.clone();
        if mutation_paths.contains(target.as_str()) {
            continue;
        }
        if !directory_plan.preserves_leaf(&target) {
            bail!(
                "hardlink {} target {} was omitted without preserved-payload authority",
                file.path,
                target
            );
        }
        let incoming_target = incoming_by_path.get(target.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "hardlink {} preserved target {} is absent from the incoming payload graph",
                file.path,
                target
            )
        })?;
        let anchor = FileEntry::find_by_path(conn, &target)?.ok_or_else(|| {
            anyhow::anyhow!(
                "hardlink {} preserved target {} has no materialized file anchor",
                file.path,
                target
            )
        })?;
        if !matches!(anchor.node.source.kind, PayloadNodeKind::Regular { .. }) {
            bail!(
                "hardlink {} preserved target {} is not a regular materialized file",
                file.path,
                target
            );
        }
        if incoming_target.content.authority() != anchor.content.as_ref() {
            bail!(
                "hardlink {} preserved target {} content differs from its materialized anchor",
                file.path,
                target
            );
        }

        let identity = format!("path:{target}");
        let mut target_node = anchor.node;
        target_node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone()),
        };
        let mut reference = (*incoming_target).clone();
        reference.node = target_node.clone();
        references.entry(target.clone()).or_insert(reference);

        target_node.source.kind = PayloadNodeKind::Hardlink { target, identity };
        file.node = target_node;
    }

    Ok(references.into_values().collect())
}

pub(super) fn run_triggers(
    conn: &rusqlite::Connection,
    root: &Path,
    changeset_id: i64,
    file_paths: &[String],
) -> Result<()> {
    let trigger_executor = conary_core::trigger::TriggerExecutor::new(conn, root);
    let triggered = trigger_executor
        .record_triggers(changeset_id, file_paths)
        .context("Failed to record package triggers")?;

    if !triggered.is_empty() {
        info!("Recorded {} trigger(s) for execution", triggered.len());
        let results = trigger_executor
            .execute_pending(changeset_id)
            .context("Failed to execute package triggers")?;
        if !results.all_succeeded() {
            anyhow::bail!(
                "{} package trigger(s) failed: {}",
                results.failed,
                results.errors.join("; ")
            );
        }
        if results.total() > 0 {
            info!(
                "Triggers: {} succeeded, {} failed, {} skipped",
                results.succeeded, results.failed, results.skipped
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};

    #[test]
    fn live_root_files_are_loaded_from_stored_cas_objects() {
        let temp = tempfile::tempdir().unwrap();
        let cas = conary_core::filesystem::CasStore::new(temp.path().join("objects")).unwrap();
        let hash = cas.store(b"from cas").unwrap();
        let files = live_root_files_from_stored_files(
            &cas,
            &[inner::ResolvedInstallFile {
                path: "/usr/bin/fixture".to_string(),
                node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755))
                    .unwrap(),
                content: Some(PayloadContentAuthority {
                    sha256: hash.clone(),
                    size: 8,
                }),
                cas_hash: Some(hash),
            }],
        )
        .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content.to_in_memory().unwrap(), b"from cas");
    }
}
