// apps/conary/src/commands/install/inner.rs
//! Inner install helper for callers that own the transaction lifecycle.
//!
//! `install_inner()` performs CAS storage and the DB operations (trove insert,
//! file entries, dependencies, scriptlets) using a caller-provided DB
//! transaction and changeset. It does NOT: create/commit a DB transaction,
//! create a changeset, or call `rebuild_and_mount()`.
//! The caller handles all of those.

use anyhow::{Context, Result};
use conary_core::components::ComponentType;
use conary_core::db::models::{
    Component, DependencyEntry, FileEntry, ProvideEntry, ScriptletEntry,
};
use conary_core::dependencies::DependencyClass;
use conary_core::transaction::TransactionEngine;
use rusqlite::{OptionalExtension, Transaction};
use std::collections::HashMap;
use tracing::{info, warn};

use super::{
    ExtractionResult, InstallPhase, InstallProgress, TransactionContext,
    mark_upgraded_parent_deriveds_stale, scheme_to_string,
};

/// Result from `install_inner` -- the trove ID of the installed package.
pub struct InnerInstallResult {
    pub trove_id: i64,
}

/// Execute the install DB operations using a caller-owned DB transaction.
///
/// Stores files in CAS via the provided engine, then inserts the trove,
/// components, files, dependencies, and scriptlets into the caller-provided
/// transaction under the provided `changeset_id`.
pub fn install_inner(
    tx: &Transaction<'_>,
    engine: &mut TransactionEngine,
    changeset_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
) -> Result<InnerInstallResult> {
    let is_upgrade = ctx.old_trove_to_upgrade.is_some();

    progress.set_phase(pkg.name(), InstallPhase::Deploying);
    let mut file_hashes: Vec<(String, String, i64, i32, Option<String>)> =
        Vec::with_capacity(extraction.extracted_files.len());
    for file in &extraction.extracted_files {
        let hash = engine
            .cas()
            .store(&file.content)
            .with_context(|| format!("Failed to store {} in CAS", file.path))?;
        file_hashes.push((
            file.path.clone(),
            hash,
            file.size,
            file.mode,
            file.symlink_target.clone(),
        ));
    }

    info!(
        "Stored {} files in CAS for {}",
        file_hashes.len(),
        pkg.name()
    );

    let selection_reason = ctx.selection_reason;
    let classified = &extraction.classified;
    let language_provides = &extraction.language_provides;
    let scriptlets = pkg.scriptlets();

    let trove_id = {
        if let Some(old_trove) = ctx.old_trove_to_upgrade
            && let Some(old_id) = old_trove.id
        {
            info!("Removing old version {} before upgrade", old_trove.version);
            conary_core::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = pkg.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.version_scheme = Some(scheme_to_string(ctx.semantics.version_scheme));

        if let Some(reason) = selection_reason {
            trove.selection_reason = Some(reason.to_string());
        }

        if trove.install_source == conary_core::db::models::InstallSource::Repository {
            let repo_id: Option<i64> = tx
                .query_row(
                    "SELECT r.id FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE rp.name = ?1 AND rp.version = ?2
                       AND (?3 IS NULL OR rp.architecture IS NULL OR rp.architecture = ?3)
                     ORDER BY
                         (r.default_strategy_distro = (SELECT distro FROM distro_pin LIMIT 1)) DESC,
                         r.priority DESC, r.id ASC
                     LIMIT 1",
                    rusqlite::params![pkg.name(), pkg.version(), pkg.architecture()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(conary_core::Error::from)?;
            trove.installed_from_repository_id = repo_id;
        }

        let trove_id = trove.insert(tx)?;

        let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
        for comp_type in classified.keys() {
            let mut component = Component::from_type(trove_id, *comp_type);
            component.description = Some(format!("{} files", comp_type.as_str()));
            let comp_id = component.insert(tx)?;
            component_ids.insert(*comp_type, comp_id);
        }

        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        for (comp_type, files) in classified {
            if let Some(&comp_id) = component_ids.get(comp_type) {
                for path in files {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        }

        for (path, hash, size, mode, symlink_target) in &file_hashes {
            if hash.len() < 3 {
                warn!("Skipping file with short hash: {} (hash={})", path, hash);
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &size.to_string()],
            )?;

            let component_id = path_to_component.get(path.as_str()).copied();
            let mut file_entry = FileEntry::new(path.clone(), hash.clone(), *size, *mode, trove_id);
            file_entry.component_id = component_id;
            file_entry.symlink_target = symlink_target.clone();
            file_entry.insert(tx)?;

            let action = if is_upgrade { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), path, hash, action],
            )?;
        }

        for dep in pkg.dependencies() {
            let mut dep_entry = DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                None,
                dep.dep_type.as_str().to_string(),
                dep.version.clone(),
            );
            dep_entry.insert(tx)?;
        }

        for scriptlet in scriptlets {
            let mut entry = ScriptletEntry::with_flags(
                trove_id,
                scriptlet.phase.to_string(),
                scriptlet.interpreter.clone(),
                scriptlet.content.clone(),
                scriptlet.flags.clone(),
                match ctx.semantics.source {
                    super::PreparedSourceKind::Legacy { format } => format.as_str(),
                    super::PreparedSourceKind::Ccs => "ccs",
                },
            );
            entry.insert(tx)?;
        }

        for lang_dep in language_provides {
            let kind = match lang_dep.class {
                DependencyClass::Package => "package",
                _ => lang_dep.class.prefix(),
            };
            let mut provide = ProvideEntry::new_typed(
                trove_id,
                kind,
                lang_dep.name.clone(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            pkg.name().to_string(),
            Some(pkg.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        trove_id
    };

    if let Some(old_trove) = ctx.old_trove_to_upgrade {
        mark_upgraded_parent_deriveds_stale(
            tx,
            pkg.name(),
            Some(&old_trove.version),
            pkg.version(),
        );
    }

    Ok(InnerInstallResult { trove_id })
}
