// apps/conary/src/commands/remove/transaction.rs

use std::path::Path;

use anyhow::Result;
use conary_core::ccs::legacy_replay::LegacyReplayPlan;
use conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle;
use conary_core::db::models::{FileEntry, ScriptletEntry, Trove};
use conary_core::scriptlet::{
    ExecutionMode, PackageFormat as ScriptletPackageFormat, ScriptletExecutor,
};
use tracing::info;

use super::legacy_replay::{
    execute_legacy_remove_replay_plan_entries, load_installed_legacy_remove_plan,
    require_legacy_replay_success,
};
use super::types::{LegacyRemoveReplayAuditContext, RemoveInnerResult, RemoveScriptletOptions};
use crate::commands::progress::{RemovePhase, RemoveProgress};
use crate::commands::{FileSnapshot, TroveSnapshot};

pub(super) struct PreparedRemove {
    pub(super) snapshot: TroveSnapshot,
    trove: Trove,
    stored_scriptlets: Vec<ScriptletEntry>,
    scriptlet_format: ScriptletPackageFormat,
    removed_count: usize,
    dirs_removed: usize,
    #[allow(dead_code)]
    planned_pre_remove: Option<LegacyReplayPlan>,
    legacy_bundle: Option<LegacyScriptletBundle>,
    legacy_pre_outcomes: Vec<conary_core::scriptlet::ScriptletOutcome>,
    legacy_audit_context: Option<LegacyRemoveReplayAuditContext>,
    planned_post_remove: Option<LegacyReplayPlan>,
}

/// Inner remove helper for callers that own the transaction lifecycle.
///
/// Performs pre-remove scriptlets and DB writes using a caller-provided DB
/// transaction and `changeset_id`. Returns the rollback snapshot plus enough
/// metadata for the caller to run post-remove handling after rebuild.
pub(crate) fn remove_inner(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    trove: &Trove,
    root: &str,
    scriptlet_options: RemoveScriptletOptions,
    progress: &RemoveProgress,
) -> Result<RemoveInnerResult> {
    let prepared = prepare_remove(tx, trove, root, scriptlet_options, progress)?;
    commit_remove_db(tx, changeset_id, prepared)
}

pub(super) fn prepare_remove(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    scriptlet_options: RemoveScriptletOptions,
    progress: &RemoveProgress,
) -> Result<PreparedRemove> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let legacy_replay = load_installed_legacy_remove_plan(conn, trove_id, scriptlet_options)?;
    let stored_scriptlets = ScriptletEntry::find_by_trove(conn, trove_id)?;
    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| ScriptletPackageFormat::parse(&s.package_format))
        .unwrap_or(ScriptletPackageFormat::Rpm);
    let legacy_pre_outcomes = execute_legacy_remove_replay_plan_entries(
        Path::new(root),
        &trove.name,
        &trove.version,
        legacy_replay.bundle.as_ref(),
        legacy_replay.planned_pre_remove.as_ref(),
        scriptlet_options.sandbox_mode,
    )?;
    require_legacy_replay_success(&legacy_pre_outcomes)?;

    // NOTE: Known limitation -- if the pre-remove scriptlet partially executes
    // and then fails, there is no automatic recovery. This is consistent with
    // RPM, dpkg, and pacman which also have no pre-remove rollback mechanism.
    if !scriptlet_options.no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PreScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        )
        .with_sandbox_mode(scriptlet_options.sandbox_mode);

        for phase in ["pre-remove", "post-remove"] {
            if let Some(scriptlet) = stored_scriptlets
                .iter()
                .find(|scriptlet| scriptlet.phase == phase)
            {
                executor.preflight_entry(scriptlet, &ExecutionMode::Remove)?;
            }
        }

        if let Some(pre) = stored_scriptlets.iter().find(|s| s.phase == "pre-remove") {
            info!("Running pre-remove scriptlet...");
            executor.execute_entry(pre, &ExecutionMode::Remove)?;
        }
    }

    let breaking_now =
        conary_core::resolver::solve_removal(conn, std::slice::from_ref(&trove.name))?;
    if !breaking_now.is_empty() {
        return Err(conary_core::Error::IoError(format!(
            "Concurrent change: '{}' now required by: {}",
            trove.name,
            breaking_now.join(", ")
        ))
        .into());
    }

    let (directories, regular_files): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|f| f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000);

    Ok(PreparedRemove {
        snapshot: TroveSnapshot {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
            description: trove.description.clone(),
            install_source: trove.install_source.as_str().to_string(),
            installed_from_repository_id: trove.installed_from_repository_id,
            files: files
                .iter()
                .map(|f| FileSnapshot {
                    path: f.path.clone(),
                    sha256_hash: f.sha256_hash.clone(),
                    size: f.size,
                    permissions: f.permissions,
                    symlink_target: f.symlink_target.clone(),
                })
                .collect(),
        },
        trove: trove.clone(),
        stored_scriptlets,
        scriptlet_format,
        removed_count: regular_files.len(),
        dirs_removed: directories.len(),
        planned_pre_remove: legacy_replay.planned_pre_remove,
        legacy_bundle: legacy_replay.bundle,
        legacy_pre_outcomes,
        legacy_audit_context: legacy_replay.audit_context,
        planned_post_remove: legacy_replay.planned_post_remove,
    })
}

pub(super) fn commit_remove_db(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    prepared: PreparedRemove,
) -> Result<RemoveInnerResult> {
    let trove_id = prepared
        .trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    for file in &prepared.snapshot.files {
        let use_hash = if file.sha256_hash.len() == 64
            && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            let hash_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_contents WHERE sha256_hash = ?1)",
                [&file.sha256_hash],
                |row| row.get(0),
            )?;
            if hash_exists {
                Some(file.sha256_hash.as_str())
            } else {
                None
            }
        } else {
            None
        };

        match use_hash {
            Some(hash) => {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                    [&changeset_id.to_string(), &file.path, hash, "delete"],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, NULL, ?3)",
                    [&changeset_id.to_string(), &file.path, "delete"],
                )?;
            }
        }
    }

    Trove::delete(tx, trove_id)?;

    Ok(RemoveInnerResult {
        changeset_id,
        snapshot: prepared.snapshot,
        trove: prepared.trove,
        stored_scriptlets: prepared.stored_scriptlets,
        scriptlet_format: prepared.scriptlet_format,
        removed_count: prepared.removed_count,
        dirs_removed: prepared.dirs_removed,
        planned_pre_remove: prepared.planned_pre_remove,
        legacy_bundle: prepared.legacy_bundle,
        legacy_pre_outcomes: prepared.legacy_pre_outcomes,
        legacy_audit_context: prepared.legacy_audit_context,
        planned_post_remove: prepared.planned_post_remove,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::remove_snapshot;
    use super::*;
    use conary_core::ccs::legacy_replay::{
        LegacyReplayCompatibilityDecision, LegacyReplayPlan, PlannedLegacyEntry,
    };
    use conary_core::ccs::legacy_scriptlets::LifecyclePath;
    use conary_core::db::models::{InstallSource, Trove, TroveType};
    use conary_core::scriptlet::{SandboxMode, ScriptletOutcome};
    use tempfile::TempDir;

    #[test]
    fn commit_remove_db_carries_planned_post_remove_after_trove_delete() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.id = Some(trove.insert(&conn).unwrap());
        let planned_post_remove = LegacyReplayPlan {
            target_id: "rpm/fedora/44/x86_64".to_string(),
            source_target_id: "rpm/fedora/44/x86_64".to_string(),
            bundle_evidence_digest: Some(conary_core::hash::sha256_prefixed(b"bundle-evidence")),
            lifecycle_entries: vec![PlannedLegacyEntry {
                entry_id: "rpm:%postun".to_string(),
                native_slot: "%postun".to_string(),
                phase: LifecyclePath::PostRemove,
                timeout_ms: 30_000,
            }],
            sandbox_floor: SandboxMode::None,
            ccs_hooks_allowed: true,
            raw_replay_required: true,
            compatibility_decision: accepted_compatibility_decision(),
        };
        let prepared = PreparedRemove {
            snapshot: remove_snapshot(Vec::new()),
            trove,
            stored_scriptlets: Vec::new(),
            scriptlet_format: ScriptletPackageFormat::Rpm,
            removed_count: 0,
            dirs_removed: 0,
            planned_pre_remove: None,
            legacy_bundle: None,
            legacy_pre_outcomes: Vec::<ScriptletOutcome>::new(),
            legacy_audit_context: None,
            planned_post_remove: Some(planned_post_remove.clone()),
        };
        let tx = conn.unchecked_transaction().unwrap();

        let result = commit_remove_db(&tx, 1, prepared).unwrap();

        assert_eq!(result.planned_post_remove, Some(planned_post_remove));
    }

    fn accepted_compatibility_decision() -> LegacyReplayCompatibilityDecision {
        LegacyReplayCompatibilityDecision {
            decision: "accepted".to_string(),
            reason_code: "compatibility-source-native".to_string(),
            matrix_entry_id: None,
            matrix_digest: None,
            preflight_checks: Vec::new(),
            override_required: false,
            override_used: false,
        }
    }
}
