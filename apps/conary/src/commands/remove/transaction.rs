// apps/conary/src/commands/remove/transaction.rs

use super::types::{RemoveInnerResult, RemoveLifecycleOptions};
use super::{execute_preflighted_ccs_remove_hook, preflight_ccs_remove_hook};
use crate::commands::progress::{RemovePhase, RemoveProgress};
use crate::commands::{
    CcsRemoveHookSnapshot, FileSnapshot, NativeLifecycleSnapshot, TroveSnapshot,
};
use anyhow::Result;
use conary_core::db::models::{
    ConfigFile, ConfigSource, FileEntry, InstalledNativeLifecycleBundle, Trove,
};

pub(super) struct PreparedRemove {
    pub(super) snapshot: TroveSnapshot,
    trove: Trove,
    removed_count: usize,
    dirs_removed: usize,
}

pub(super) fn prepare_remove(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    lifecycle_options: RemoveLifecycleOptions,
    progress: &RemoveProgress,
) -> Result<PreparedRemove> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let ccs_remove_hook =
        preflight_ccs_remove_hook(conn, trove, root, lifecycle_options.sandbox_mode)?;
    let version_scheme = trove.version_scheme;
    let native_lifecycle = InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id)?
        .map(|installed| {
            installed.bundle()?;
            Ok::<_, anyhow::Error>(NativeLifecycleSnapshot {
                bundle_toml: installed.bundle_toml,
                lifecycle_state: installed.lifecycle_state.as_str().to_string(),
                pending_triggers: installed.pending_triggers,
                awaited_packages: installed.awaited_packages,
            })
        })
        .transpose()?;
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

    // NOTE: If the CCS pre-remove hook partially executes and then fails, there
    // is no automatic recovery of the hook's external side effects.
    if let Some(hook) = ccs_remove_hook.as_ref() {
        progress.set_phase(RemovePhase::PreScript);
        execute_preflighted_ccs_remove_hook(trove, hook, root, lifecycle_options.sandbox_mode)?;
    }

    let (directories, regular_files): (Vec<_>, Vec<_>) = files.iter().partition(|file| {
        matches!(
            file.node.source.kind,
            conary_core::payload::PayloadNodeKind::Directory
        )
    });

    Ok(PreparedRemove {
        snapshot: TroveSnapshot {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
            description: trove.description.clone(),
            install_source: trove.install_source.as_str().to_string(),
            source_distro: trove.source_distro.clone(),
            version_scheme,
            native_lifecycle,
            ccs_remove_hook: ccs_remove_hook.as_ref().map(|hook| CcsRemoveHookSnapshot {
                script: hook.script.clone(),
                reversible: hook.reversible,
            }),
            installed_from_repository_id: trove.installed_from_repository_id,
            files: files
                .iter()
                .map(|f| FileSnapshot {
                    path: f.path.clone(),
                    node: f.node.clone(),
                    content: f.content.clone(),
                })
                .collect(),
        },
        trove: trove.clone(),
        removed_count: regular_files.len(),
        dirs_removed: directories.len(),
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
        let use_hash = if let Some(content) = file.content.as_ref() {
            let hash_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_contents WHERE sha256_hash = ?1)",
                [&content.sha256],
                |row| row.get(0),
            )?;
            if hash_exists {
                Some(content.sha256.as_str())
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

    for config in ConfigFile::find_by_trove(tx, trove_id)? {
        if config.source != ConfigSource::Deb {
            ConfigFile::delete(
                tx,
                config
                    .id
                    .ok_or_else(|| anyhow::anyhow!("tracked config has no database identity"))?,
            )?;
        }
    }
    Trove::delete(tx, trove_id)?;

    Ok(RemoveInnerResult {
        snapshot: prepared.snapshot,
        trove: prepared.trove,
        removed_count: prepared.removed_count,
        dirs_removed: prepared.dirs_removed,
    })
}
