// apps/conary/src/commands/system/rollback_restore/materialized_directories.rs

//! Exact restoration of pre-mutation materialized shared-directory authority.

use crate::commands::installed_authority_snapshot::{
    MaterializedDirectorySnapshot, StableTroveIdentitySnapshot,
};
use conary_core::Error;
use conary_core::db::models::{Component, DirectoryClaim, FileEntry, Trove};
use rusqlite::Transaction;

pub(in crate::commands::system) fn restore_materialized_directories(
    tx: &Transaction<'_>,
    snapshots: &[MaterializedDirectorySnapshot],
) -> conary_core::Result<()> {
    for snapshot in snapshots {
        snapshot
            .validate()
            .map_err(|error| Error::InitError(error.to_string()))?;
        let trove = resolve_stable_trove(tx, &snapshot.anchor_trove)?;
        let trove_id = trove.id.ok_or_else(|| {
            Error::MissingId(format!(
                "rollback materialized directory anchor '{}-{}' has no database identity",
                trove.name, trove.version
            ))
        })?;
        let component_id = snapshot
            .anchor_component
            .as_deref()
            .map(|component| {
                let component =
                    Component::find_by_trove_and_name(tx, trove_id, component)?.ok_or_else(|| {
                        Error::InitError(format!(
                            "rollback materialized directory '{}' anchor component '{}' is missing",
                            snapshot.path, component
                        ))
                    })?;
                component.id.ok_or_else(|| {
                    Error::MissingId(format!(
                        "rollback materialized directory '{}' anchor component has no database identity",
                        snapshot.path
                    ))
                })
            })
            .transpose()?;
        let claims = DirectoryClaim::find_retaining_path(tx, &snapshot.path)?;
        if claims.is_empty() {
            return Err(Error::ConflictError(format!(
                "rollback materialized directory '{}' has no directory-claim authority",
                snapshot.path
            )));
        }
        for claim in &claims {
            if !claim
                .anchor_policy
                .accepts(&snapshot.materialized_node.source.kind)
            {
                return Err(Error::ConflictError(format!(
                    "rollback materialized directory '{}' is incompatible with claim policy '{}' for trove {}",
                    snapshot.path,
                    claim.anchor_policy.as_str(),
                    claim.trove_id
                )));
            }
        }
        FileEntry::restore_materialized_directory_anchor(
            tx,
            &snapshot.path,
            &snapshot.materialized_node,
            snapshot.anchor_policy,
            trove_id,
            component_id,
            &snapshot.installed_at,
        )?;
    }
    Ok(())
}

fn resolve_stable_trove(
    tx: &Transaction<'_>,
    identity: &StableTroveIdentitySnapshot,
) -> conary_core::Result<Trove> {
    let mut candidates = Trove::find_by_name(tx, &identity.name)?
        .into_iter()
        .filter(|trove| {
            trove.version == identity.version
                && trove.trove_type == identity.trove_type
                && trove.architecture == identity.architecture
                && trove.installed_at.as_deref() == Some(identity.installed_at.as_str())
                && trove.version_scheme == identity.version_scheme
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err(Error::ConflictError(format!(
            "rollback materialized directory anchor identity '{}-{}' resolved to {} installed troves",
            identity.name,
            identity.version,
            candidates.len()
        )));
    }
    Ok(candidates.pop().expect("one stable trove candidate"))
}
