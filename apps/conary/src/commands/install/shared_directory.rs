// apps/conary/src/commands/install/shared_directory.rs
//! Exact install-time authority for package-shared directories.

use super::inner::ResolvedInstallFile;
use super::{InstallSemantics, PackageFormatType, PreparedSourceKind};
use anyhow::{Result, anyhow};
use conary_core::db::models::{
    ExistingDirectoryMaterialization, FileEntry, InstallSource, PayloadClaim, Trove,
};
use conary_core::generation::root_manifest::{RootPathDomain, classify_root_path};
use conary_core::payload::PayloadNodeKind;
use conary_core::repository::dependency_model::PackageRelationRemovalMode;
use conary_core::transaction::PackageRelationRemoval;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UpstreamDirectoryMaterializationInput {
    pub(crate) format: PackageFormatType,
    pub(crate) implementation: &'static str,
    pub(crate) revision: &'static str,
    pub(crate) path: &'static str,
    pub(crate) url: &'static str,
    pub(crate) sha256: &'static str,
    pub(crate) behavior: UpstreamDirectoryBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum UpstreamDirectoryBehavior {
    RpmInstallAppliesDirectoryMetadata,
    DpkgUnpackPreservesDirectoryOrSymlinkToDirectory,
    DpkgRemoveDeletesOnlyUnsharedDirectoryOrSymlink,
    LibalpmInstallPreservesDirectoryAndReplacesSymlink,
    EopkgTarInstallAppliesDirectoryMetadata,
}

/// Immutable upstream source inputs behind Conary's existing-directory policy.
///
/// These are production contract inputs, not test-only commentary. Updating
/// one requires re-reading the exact source and revalidating the behavior
/// selected by the typed per-path planner and claim-aware removal projection.
pub(crate) const UPSTREAM_DIRECTORY_MATERIALIZATION_INPUTS:
    &[UpstreamDirectoryMaterializationInput] = &[
    UpstreamDirectoryMaterializationInput {
        format: PackageFormatType::Rpm,
        implementation: "rpm",
        revision: "a8f0192aee1c08bd1454ed2ac6ebaf506004b55c",
        path: "lib/fsm.cc",
        url: "https://raw.githubusercontent.com/rpm-software-management/rpm/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/fsm.cc",
        sha256: "3f0fddf3f53430950ec88a0457c6341699c304db024e501a8aa14338dc90abfb",
        behavior: UpstreamDirectoryBehavior::RpmInstallAppliesDirectoryMetadata,
    },
    UpstreamDirectoryMaterializationInput {
        format: PackageFormatType::Deb,
        implementation: "dpkg",
        revision: "1.23.7",
        path: "src/main/unpack.c",
        url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/unpack.c",
        sha256: "8ca212f6901994b6265f36e34a2d83801a441639e158077242b5cf4ec1b5a9b6",
        behavior: UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory,
    },
    UpstreamDirectoryMaterializationInput {
        format: PackageFormatType::Deb,
        implementation: "dpkg",
        revision: "1.23.7",
        path: "src/main/remove.c",
        url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/remove.c",
        sha256: "72f29921c1939b988a49f4a2922803af128b038d05263a5398424636253ab824",
        behavior: UpstreamDirectoryBehavior::DpkgRemoveDeletesOnlyUnsharedDirectoryOrSymlink,
    },
    UpstreamDirectoryMaterializationInput {
        format: PackageFormatType::Arch,
        implementation: "libalpm",
        revision: "a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d",
        path: "lib/libalpm/add.c",
        url: "https://gitlab.archlinux.org/pacman/pacman/-/raw/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d/lib/libalpm/add.c",
        sha256: "c63fb24d5f7a458d1a1c0fb1f0bd9cadf8b23e461f6f9cf7598e31310f672dda",
        behavior: UpstreamDirectoryBehavior::LibalpmInstallPreservesDirectoryAndReplacesSymlink,
    },
    UpstreamDirectoryMaterializationInput {
        format: PackageFormatType::Eopkg,
        implementation: "eopkg",
        revision: "59e423ea39088cd46f1903b1094127d8fda28d73",
        path: "pisi/archive.py",
        url: "https://github.com/getsolus/eopkg/blob/59e423ea39088cd46f1903b1094127d8fda28d73/pisi/archive.py",
        sha256: "4c4dd694b8d3b0f48fb878cbb79a0fbfb04e89f40f0397589b3e40504041684c",
        behavior: UpstreamDirectoryBehavior::EopkgTarInstallAppliesDirectoryMetadata,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectoryInstallPlan {
    paths: BTreeMap<String, DirectoryPathPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DirectoryPathPlan {
    CreateOrReplace,
    PreserveCompatiblePayload,
    PreserveLeaf {
        leaf_node: conary_core::payload::ResolvedPayloadNode,
        materialization: ExistingDirectoryMaterialization,
    },
    ApplyDirectoryMetadata {
        leaf_before: conary_core::payload::ResolvedPayloadNode,
        materialization: ExistingDirectoryMaterialization,
    },
    ApplyThroughSymlink {
        leaf_node: conary_core::payload::ResolvedPayloadNode,
        resolved_target_path: String,
        target_before: conary_core::payload::ResolvedPayloadNode,
    },
}

impl DirectoryInstallPlan {
    pub(super) fn path(&self, path: &str) -> Option<&DirectoryPathPlan> {
        self.paths.get(path)
    }

    pub(super) fn preserves_leaf(&self, path: &str) -> bool {
        matches!(
            self.path(path),
            Some(
                DirectoryPathPlan::PreserveCompatiblePayload
                    | DirectoryPathPlan::PreserveLeaf { .. }
                    | DirectoryPathPlan::ApplyThroughSymlink { .. }
            )
        )
    }

    pub(super) fn leaf_before(
        &self,
        path: &str,
    ) -> Option<&conary_core::payload::ResolvedPayloadNode> {
        match self.path(path) {
            Some(DirectoryPathPlan::PreserveLeaf { leaf_node, .. }) => Some(leaf_node),
            Some(DirectoryPathPlan::ApplyDirectoryMetadata { leaf_before, .. }) => {
                Some(leaf_before)
            }
            Some(DirectoryPathPlan::ApplyThroughSymlink { leaf_node, .. }) => Some(leaf_node),
            Some(DirectoryPathPlan::PreserveCompatiblePayload) => None,
            _ => None,
        }
    }

    pub(super) fn materialization_for_path(&self, path: &str) -> ExistingDirectoryMaterialization {
        match self.path(path) {
            Some(DirectoryPathPlan::PreserveLeaf {
                materialization, ..
            }) => *materialization,
            Some(DirectoryPathPlan::ApplyThroughSymlink { .. }) => {
                ExistingDirectoryMaterialization::PreserveExistingDirectoryOrSymlinkToDirectory
            }
            Some(DirectoryPathPlan::ApplyDirectoryMetadata {
                materialization, ..
            }) => *materialization,
            _ => ExistingDirectoryMaterialization::ApplyIncoming,
        }
    }

    pub(super) fn through_symlink_metadata(
        &self,
        incoming: &ResolvedInstallFile,
    ) -> Option<(String, conary_core::payload::ResolvedPayloadNode)> {
        match self.path(&incoming.path) {
            Some(DirectoryPathPlan::ApplyThroughSymlink {
                resolved_target_path,
                ..
            }) => Some((resolved_target_path.clone(), incoming.node.clone())),
            _ => None,
        }
    }

    pub(super) fn through_symlink_root_files(
        &self,
        incoming: &[ResolvedInstallFile],
    ) -> Vec<crate::commands::LiveRootFile> {
        incoming
            .iter()
            .filter_map(|file| {
                self.through_symlink_metadata(file).map(|(path, node)| {
                    crate::commands::LiveRootFile {
                        path,
                        content: crate::commands::LiveRootContent::absent(),
                        node,
                    }
                })
            })
            .collect()
    }

    pub(super) fn reconcile_through_symlink_target(
        &self,
        tx: &rusqlite::Transaction<'_>,
        incoming: &ResolvedInstallFile,
    ) -> Result<()> {
        let Some(DirectoryPathPlan::ApplyThroughSymlink {
            resolved_target_path,
            ..
        }) = self.path(&incoming.path)
        else {
            return Ok(());
        };
        let Some(existing) = FileEntry::find_by_path(tx, resolved_target_path)? else {
            return Ok(());
        };
        if !matches!(existing.node.source.kind, PayloadNodeKind::Directory) {
            return Err(anyhow!(
                "RPM directory target {} is tracked as a non-directory",
                resolved_target_path
            ));
        }
        if directory_retainers(tx, resolved_target_path)?.is_empty() {
            return Err(anyhow!(
                "Tracked RPM directory target {} has no package claim or materialization-target authority",
                resolved_target_path
            ));
        }
        FileEntry::reconcile_directory_materialization(
            tx,
            resolved_target_path,
            &incoming.node,
            ExistingDirectoryMaterialization::ApplyIncoming,
        )?;
        Ok(())
    }

    pub(super) fn persist_through_symlink_target(
        &self,
        tx: &rusqlite::Transaction<'_>,
        incoming: &ResolvedInstallFile,
        trove_id: i64,
    ) -> Result<()> {
        let Some(DirectoryPathPlan::ApplyThroughSymlink {
            resolved_target_path,
            ..
        }) = self.path(&incoming.path)
        else {
            return Ok(());
        };
        PayloadClaim::set_materialization_target(
            tx,
            &incoming.path,
            trove_id,
            resolved_target_path,
        )?;
        Ok(())
    }
}

/// Capture the exact directory materializations that incoming package nodes
/// may mutate.
///
/// Rollback callers run this before selected-root mutation. Claims retain each
/// package's declared node, while this captures the independently materialized
/// node that must match the selected root after rollback.
pub(crate) fn capture_existing_directory_materializations(
    conn: &rusqlite::Connection,
    selected_root: &std::path::Path,
) -> Result<Vec<FileEntry>> {
    let mut captured = BTreeMap::new();
    for anchor in FileEntry::find_all_ordered(conn)? {
        let claims = directory_retainers(conn, &anchor.path)?;
        if claims.is_empty() && !matches!(anchor.node.source.kind, PayloadNodeKind::Directory) {
            continue;
        }
        let effective_path =
            conary_core::filesystem::selected_root::selected_root_effective_package_path(
                selected_root,
                &anchor.path,
            )?;
        if classify_root_path(&effective_path)? == RootPathDomain::EphemeralMountOrUser {
            continue;
        }
        if claims.is_empty() {
            return Err(anyhow!(
                "Tracked directory materialization {} has no package claim",
                anchor.path
            ));
        }
        let selected_root_node =
            conary_core::filesystem::selected_root::capture_selected_root_node(
                selected_root,
                &anchor.path,
            )?
            .ok_or_else(|| {
                anyhow!(
                    "Tracked directory materialization {} is absent from the selected root",
                    anchor.path
                )
            })?;
        if matches!(
            selected_root_node.source.kind,
            PayloadNodeKind::Symlink { .. }
        ) && conary_core::filesystem::selected_root::selected_root_symlink_targets_directory(
            selected_root,
            &anchor.path,
        )?
        .is_none()
        {
            return Err(anyhow!(
                "Tracked symlink anchor {} no longer resolves to a selected-root directory",
                anchor.path
            ));
        }
        capture_materialized_anchor(conn, &mut captured, &anchor.path, &selected_root_node)?;
    }
    Ok(captured.into_values().collect())
}

fn capture_materialized_anchor(
    conn: &rusqlite::Connection,
    captured: &mut BTreeMap<String, FileEntry>,
    path: &str,
    selected_root_node: &conary_core::payload::ResolvedPayloadNode,
) -> Result<()> {
    let Some(mut existing) = FileEntry::find_by_path(conn, path)? else {
        return Ok(());
    };
    let claims = directory_retainers(conn, path)?;
    if claims.is_empty() {
        if matches!(existing.node.source.kind, PayloadNodeKind::Directory) {
            return Err(anyhow!(
                "Tracked directory materialization {path} has no package claim"
            ));
        }
        return Ok(());
    }
    if !matches!(
        existing.node.source.kind,
        PayloadNodeKind::Directory | PayloadNodeKind::Symlink { .. }
    ) {
        return Err(anyhow!(
            "Selected-root directory materialization {path} is tracked as a non-directory"
        ));
    }
    for claim in &claims {
        if !claim
            .anchor_policy
            .accepts_kind(&selected_root_node.source.kind)
        {
            return Err(anyhow!(
                "Selected-root node {path} violates package {} directory anchor policy '{}'",
                claim.trove_id,
                claim.anchor_policy.as_str()
            ));
        }
    }
    existing.node = selected_root_node.clone();
    existing.content = None;
    if let Some(previous) = captured.insert(path.to_string(), existing.clone())
        && previous != existing
    {
        return Err(anyhow!(
            "Selected-root directory materialization {path} resolved to conflicting rollback authority"
        ));
    }
    Ok(())
}

/// Definitive ownership preflight after package identities have been resolved
/// against the selected root.
///
/// Directory-directory overlap is supported for every source format even when
/// the claims declare different metadata. RPM/author-native CCS applies the
/// incoming directory node; dpkg and libalpm preserve an existing materialized
/// directory. Other node-type overlaps still require exact existing-package or
/// typed ownership-transfer authority.
pub(super) fn preflight_resolved_file_ownership(
    conn: &rusqlite::Connection,
    selected_root: &std::path::Path,
    files: &[ResolvedInstallFile],
    package_name: &str,
    relation_removals: &[PackageRelationRemoval],
    semantics: InstallSemantics,
) -> Result<DirectoryInstallPlan> {
    let mut paths = BTreeMap::new();
    for file in files {
        let existing = FileEntry::find_by_path(conn, &file.path)?;
        let directory_path_plan = if matches!(file.node.source.kind, PayloadNodeKind::Directory) {
            Some(plan_directory_path(selected_root, &file.path, semantics)?)
        } else {
            None
        };
        if let Some(plan) = directory_path_plan.as_ref() {
            validate_selected_root_plan_against_database(conn, &file.path, plan, &file.node)?;
            paths.insert(file.path.clone(), plan.clone());
        }
        let Some(existing) = existing else {
            continue;
        };
        if matches!(file.node.source.kind, PayloadNodeKind::Directory) {
            if matches!(existing.node.source.kind, PayloadNodeKind::Directory)
                && PayloadClaim::find_by_path(conn, &file.path)?.is_empty()
            {
                return Err(anyhow!(
                    "Tracked directory materialization {} has no package claim",
                    file.path
                ));
            }
            let materialization = directory_path_plan.as_ref().map_or(
                ExistingDirectoryMaterialization::ApplyIncoming,
                |plan| {
                    match plan {
                        DirectoryPathPlan::ApplyThroughSymlink { .. } => {
                            ExistingDirectoryMaterialization::
                                PreserveExistingDirectoryOrSymlinkToDirectory
                        }
                        DirectoryPathPlan::PreserveLeaf {
                            materialization, ..
                        } => *materialization,
                        DirectoryPathPlan::CreateOrReplace
                        | DirectoryPathPlan::PreserveCompatiblePayload
                        => {
                            ExistingDirectoryMaterialization::ApplyIncoming
                        }
                        DirectoryPathPlan::ApplyDirectoryMetadata {
                            materialization, ..
                        } => *materialization,
                    }
                },
            );
            if matches!(existing.node.source.kind, PayloadNodeKind::Directory)
                || materialization.accepts_anchor_kind(&existing.node.source.kind)
            {
                continue;
            }
        }
        let sharing_mismatch = if matches!(file.node.source.kind, PayloadNodeKind::Directory) {
            None
        } else {
            let claims = PayloadClaim::find_by_path(conn, &file.path)?;
            if claims.is_empty() {
                return Err(anyhow!(
                    "Tracked payload materialization {} has no package claim",
                    file.path
                ));
            }
            let incoming_policy = semantics.payload_sharing_policy();
            let mismatch = claims.iter().find_map(|claim| {
                incoming_policy
                    .compare(
                        claim.sharing_policy,
                        &file.node.source,
                        file.content.as_ref(),
                        &claim.node.source,
                        claim.content.as_ref(),
                    )
                    .err()
                    .map(|reason| (claim.trove_id, reason))
            });
            if mismatch.is_none() {
                paths.insert(
                    file.path.clone(),
                    DirectoryPathPlan::PreserveCompatiblePayload,
                );
                continue;
            }
            mismatch
        };
        if sharing_mismatch.is_some() {
            let claimants = PayloadClaim::find_by_path(conn, &file.path)?;
            let mut every_claimant_replaced = true;
            for claimant in claimants {
                if !trove_allows_replacement(
                    conn,
                    claimant.trove_id,
                    package_name,
                    relation_removals,
                )? {
                    every_claimant_replaced = false;
                    break;
                }
            }
            if every_claimant_replaced {
                continue;
            }
        } else if owner_allows_replacement(conn, &existing, package_name, relation_removals)? {
            continue;
        }
        if let Some((claimant_id, mismatch)) = sharing_mismatch {
            let claimant = Trove::find_by_id(conn, claimant_id)?.ok_or_else(|| {
                anyhow!(
                    "Path {} has a payload claim from missing trove {}",
                    file.path,
                    claimant_id
                )
            })?;
            return Err(anyhow!(
                "Path {} from {} is incompatible with package {}: {}",
                file.path,
                package_name,
                claimant.name,
                mismatch
            ));
        }
        let owner = Trove::find_by_id(conn, existing.trove_id)?.ok_or_else(|| {
            anyhow!(
                "Path {} is already tracked by missing trove {}",
                file.path,
                existing.trove_id
            )
        })?;
        return Err(anyhow!(
            "Path {} is already tracked by package {}",
            file.path,
            owner.name
        ));
    }
    Ok(DirectoryInstallPlan { paths })
}

fn validate_selected_root_plan_against_database(
    conn: &rusqlite::Connection,
    package_path: &str,
    plan: &DirectoryPathPlan,
    incoming_node: &conary_core::payload::ResolvedPayloadNode,
) -> Result<()> {
    let leaf_node = match plan {
        DirectoryPathPlan::PreserveLeaf { leaf_node, .. }
        | DirectoryPathPlan::ApplyDirectoryMetadata {
            leaf_before: leaf_node,
            ..
        }
        | DirectoryPathPlan::ApplyThroughSymlink { leaf_node, .. } => Some(leaf_node),
        DirectoryPathPlan::CreateOrReplace | DirectoryPathPlan::PreserveCompatiblePayload => None,
    };
    if let Some(leaf_node) = leaf_node {
        for claim in PayloadClaim::find_by_path(conn, package_path)? {
            if !claim.anchor_policy.accepts_kind(&leaf_node.source.kind) {
                return Err(anyhow!(
                    "Selected-root node {package_path} violates package {} payload anchor policy '{}'",
                    claim.trove_id,
                    claim.anchor_policy.as_str()
                ));
            }
        }
    }
    let DirectoryPathPlan::ApplyThroughSymlink {
        resolved_target_path,
        ..
    } = plan
    else {
        return Ok(());
    };
    let Some(target) = FileEntry::find_by_path(conn, resolved_target_path)? else {
        return Ok(());
    };
    if !matches!(target.node.source.kind, PayloadNodeKind::Directory) {
        return Err(anyhow!(
            "RPM directory target {resolved_target_path} is tracked as a non-directory"
        ));
    }
    let target_claims = directory_retainers(conn, resolved_target_path)?;
    if target_claims.is_empty() {
        return Err(anyhow!(
            "Tracked RPM directory target {resolved_target_path} has no package claim or materialization-target authority"
        ));
    }
    for claim in target_claims {
        if !claim.anchor_policy.accepts_kind(&incoming_node.source.kind) {
            return Err(anyhow!(
                "RPM directory target {resolved_target_path} violates package {} anchor policy '{}'",
                claim.trove_id,
                claim.anchor_policy.as_str()
            ));
        }
    }
    Ok(())
}

fn plan_directory_path(
    selected_root: &std::path::Path,
    package_path: &str,
    semantics: InstallSemantics,
) -> Result<DirectoryPathPlan> {
    let Some(node) = conary_core::filesystem::selected_root::capture_selected_root_node(
        selected_root,
        package_path,
    )?
    else {
        return Ok(DirectoryPathPlan::CreateOrReplace);
    };
    let upstream_behavior = match semantics.source {
        PreparedSourceKind::NativePackage { format } => Some(upstream_install_behavior(format)),
        PreparedSourceKind::Ccs => None,
    };
    match (upstream_behavior, &node.source.kind) {
        (
            Some(
                behavior @ (UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory
                | UpstreamDirectoryBehavior::LibalpmInstallPreservesDirectoryAndReplacesSymlink),
            ),
            PayloadNodeKind::Directory,
        ) => Ok(DirectoryPathPlan::PreserveLeaf {
            leaf_node: node,
            materialization: match behavior {
                UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory => {
                    ExistingDirectoryMaterialization::
                        PreserveExistingDirectoryOrSymlinkToDirectory
                }
                UpstreamDirectoryBehavior::LibalpmInstallPreservesDirectoryAndReplacesSymlink => {
                    ExistingDirectoryMaterialization::PreserveExistingDirectory
                }
                _ => unreachable!("matched directory-preserving behavior"),
            },
        }),
        (
            behavior @ (Some(
                UpstreamDirectoryBehavior::RpmInstallAppliesDirectoryMetadata,
            ) | None),
            PayloadNodeKind::Directory,
        ) => Ok(DirectoryPathPlan::ApplyDirectoryMetadata {
            leaf_before: node,
            materialization: if behavior.is_some() {
                ExistingDirectoryMaterialization::
                    ApplyIncomingDirectoryOrSymlinkToDirectory
            } else {
                ExistingDirectoryMaterialization::ApplyIncoming
            },
        }),
        (
            Some(
                behavior @ (UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory
                | UpstreamDirectoryBehavior::RpmInstallAppliesDirectoryMetadata),
            ),
            PayloadNodeKind::Symlink { .. },
        ) => {
            let Some(target) =
                conary_core::filesystem::selected_root::selected_root_symlink_targets_directory(
                    selected_root,
                    package_path,
                )?
            else {
                return Ok(DirectoryPathPlan::CreateOrReplace);
            };
            match behavior {
                UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory => {
                    Ok(DirectoryPathPlan::PreserveLeaf {
                        leaf_node: node,
                        materialization: ExistingDirectoryMaterialization::
                            PreserveExistingDirectoryOrSymlinkToDirectory,
                    })
                }
                UpstreamDirectoryBehavior::RpmInstallAppliesDirectoryMetadata
                    if rpm_symlink_owner_may_apply(node.uid, target.node.uid) =>
                {
                    Ok(DirectoryPathPlan::ApplyThroughSymlink {
                        leaf_node: node,
                        resolved_target_path: target.package_path,
                        target_before: target.node,
                    })
                }
                _ => Ok(DirectoryPathPlan::CreateOrReplace),
            }
        }
        _ => Ok(DirectoryPathPlan::CreateOrReplace),
    }
}

fn upstream_install_behavior(format: PackageFormatType) -> UpstreamDirectoryBehavior {
    let expected = match format {
        PackageFormatType::Rpm => UpstreamDirectoryBehavior::RpmInstallAppliesDirectoryMetadata,
        PackageFormatType::Deb => {
            UpstreamDirectoryBehavior::DpkgUnpackPreservesDirectoryOrSymlinkToDirectory
        }
        PackageFormatType::Arch => {
            UpstreamDirectoryBehavior::LibalpmInstallPreservesDirectoryAndReplacesSymlink
        }
        PackageFormatType::Eopkg => {
            UpstreamDirectoryBehavior::EopkgTarInstallAppliesDirectoryMetadata
        }
    };
    UPSTREAM_DIRECTORY_MATERIALIZATION_INPUTS
        .iter()
        .find(|source| source.format == format && source.behavior == expected)
        .map(|source| source.behavior)
        .expect("native package format has no pinned directory-install behavior")
}

const fn rpm_symlink_owner_may_apply(symlink_uid: u64, target_directory_uid: u64) -> bool {
    symlink_uid == 0 || symlink_uid == target_directory_uid
}

fn directory_retainers(conn: &rusqlite::Connection, path: &str) -> Result<Vec<PayloadClaim>> {
    Ok(PayloadClaim::find_retaining_path(conn, path)?
        .into_iter()
        .filter(|claim| matches!(claim.node.source.kind, PayloadNodeKind::Directory))
        .collect())
}

pub(super) fn owner_allows_replacement(
    conn: &rusqlite::Connection,
    existing: &FileEntry,
    package_name: &str,
    relation_removals: &[PackageRelationRemoval],
) -> Result<bool> {
    trove_allows_replacement(conn, existing.trove_id, package_name, relation_removals)
}

fn trove_allows_replacement(
    conn: &rusqlite::Connection,
    trove_id: i64,
    package_name: &str,
    relation_removals: &[PackageRelationRemoval],
) -> Result<bool> {
    let owner = Trove::find_by_id(conn, trove_id)?
        .ok_or_else(|| anyhow!("Payload claimant trove {} is missing", trove_id))?;
    Ok(owner.install_source == InstallSource::CapturedRoot
        || owner.name == package_name
        || relation_removals.iter().any(|removal| {
            removal.trove_id == trove_id
                && removal.mode == PackageRelationRemovalMode::OwnershipTransfer
                && removal
                    .ownership_transfer_packages
                    .iter()
                    .any(|incoming| incoming == package_name)
        }))
}

#[cfg(test)]
#[path = "shared_directory/tests.rs"]
mod tests;
