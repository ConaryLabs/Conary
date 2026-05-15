// apps/conary/src/commands/install/inner.rs
//! Inner install helper for callers that own the transaction lifecycle.
//!
//! `install_inner()` performs CAS storage and the DB operations (trove insert,
//! file entries, dependencies, scriptlets) using a caller-provided DB
//! transaction and changeset. It does NOT: create/commit a DB transaction,
//! create a changeset, or call `rebuild_and_mount()`.
//! The caller handles all of those.

use anyhow::{Context, Result, anyhow};
use conary_core::components::ComponentType;
use conary_core::db::models::{
    Component, DependencyEntry, FileEntry, ProvideEntry, ScriptletEntry, Trove,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoredInstallFile {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mode: i32,
    pub symlink_target: Option<String>,
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
    progress.set_phase(pkg.name(), InstallPhase::Deploying);
    let stored_files = store_install_files_in_cas(engine, extraction)?;
    info!(
        "Stored {} files in CAS for {}",
        stored_files.len(),
        pkg.name()
    );
    install_inner_with_stored_files(tx, changeset_id, pkg, extraction, ctx, &stored_files)
}

pub(super) fn store_install_files_in_cas(
    engine: &TransactionEngine,
    extraction: &ExtractionResult,
) -> Result<Vec<StoredInstallFile>> {
    let mut stored_files: Vec<StoredInstallFile> =
        Vec::with_capacity(extraction.extracted_files.len());
    for file in &extraction.extracted_files {
        let hash = if let Some(target) = file.symlink_target.as_deref() {
            engine
                .cas()
                .store_symlink(target)
                .with_context(|| format!("Failed to store symlink {} in CAS", file.path))?
        } else {
            engine
                .cas()
                .store(&file.content)
                .with_context(|| format!("Failed to store {} in CAS", file.path))?
        };
        stored_files.push(StoredInstallFile {
            path: file.path.clone(),
            hash,
            size: file.size,
            mode: file.mode,
            symlink_target: file.symlink_target.clone(),
        });
    }

    Ok(stored_files)
}

pub(super) fn install_inner_with_stored_files(
    tx: &Transaction<'_>,
    changeset_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    stored_files: &[StoredInstallFile],
) -> Result<InnerInstallResult> {
    let is_upgrade = ctx.old_trove_to_upgrade.is_some();

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

        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        if let (Some(component_names), Some(component_names_by_path)) = (
            extraction.installed_component_names.as_ref(),
            extraction.component_names_by_path.as_ref(),
        ) {
            let mut component_ids: HashMap<&str, i64> = HashMap::new();
            for component_name in component_names {
                let mut component = Component::new(trove_id, component_name.clone());
                component.description = Some(format!("{component_name} files"));
                let comp_id = component.insert(tx)?;
                component_ids.insert(component_name.as_str(), comp_id);
            }
            for (path, component_name) in component_names_by_path {
                if let Some(&comp_id) = component_ids.get(component_name.as_str()) {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        } else {
            let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
            for comp_type in classified.keys() {
                let mut component = Component::from_type(trove_id, *comp_type);
                component.description = Some(format!("{} files", comp_type.as_str()));
                let comp_id = component.insert(tx)?;
                component_ids.insert(*comp_type, comp_id);
            }

            for (comp_type, files) in classified {
                if let Some(&comp_id) = component_ids.get(comp_type) {
                    for path in files {
                        path_to_component.insert(path.as_str(), comp_id);
                    }
                }
            }
        }

        for file in stored_files {
            let path = &file.path;
            let hash = &file.hash;
            if hash.len() < 3 {
                warn!("Skipping file with short hash: {} (hash={})", path, hash);
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
            )?;

            let component_id = path_to_component.get(path.as_str()).copied();
            let mut file_entry =
                FileEntry::new(path.clone(), hash.clone(), file.size, file.mode, trove_id);
            file_entry.component_id = component_id;
            file_entry.symlink_target = file.symlink_target.clone();
            insert_file_entry_claiming_live_root_overlap(tx, &mut file_entry, pkg.name())?;

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

        if let Some(script) = extraction.ccs_pre_remove_script.as_deref() {
            let mut entry = ScriptletEntry::new(
                trove_id,
                "pre-remove".to_string(),
                "/bin/sh".to_string(),
                script.to_string(),
                "ccs",
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

fn insert_file_entry_claiming_live_root_overlap(
    tx: &Transaction<'_>,
    file_entry: &mut FileEntry,
    package_name: &str,
) -> Result<i64> {
    const LIVE_ROOT_PACKAGE_NAME: &str = "conary-live-root";

    let Some(existing) = FileEntry::find_by_path(tx, &file_entry.path)? else {
        return Ok(file_entry.insert(tx)?);
    };

    let owner = Trove::find_by_id(tx, existing.trove_id)?.ok_or_else(|| {
        anyhow!(
            "Path {} is already tracked by missing trove {}",
            file_entry.path,
            existing.trove_id
        )
    })?;

    if owner.name == LIVE_ROOT_PACKAGE_NAME || owner.name == package_name {
        info!(
            "Claiming {} from tracked package {} for {}",
            file_entry.path, owner.name, package_name
        );
        return Ok(file_entry.insert_or_replace(tx)?);
    }

    Err(anyhow!(
        "Path {} is already tracked by package {}",
        file_entry.path,
        owner.name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::install::{ExtractionResult, InstallSemantics, TransactionContext};
    use conary_core::db::models::{Changeset, FileEntry, Trove, TroveType};
    use conary_core::packages::traits::{
        Dependency, ExtractedFile, PackageFile, PackageFormat, Scriptlet,
    };
    use conary_core::transaction::{TransactionConfig, TransactionEngine};
    use std::collections::HashMap;

    struct FakePackage {
        name: String,
        version: String,
        files: Vec<PackageFile>,
        extracted_files: Vec<ExtractedFile>,
        dependencies: Vec<Dependency>,
        scriptlets: Vec<Scriptlet>,
    }

    impl FakePackage {
        fn with_file(name: &str, path: &str, content: &[u8]) -> Self {
            let size = content.len() as i64;
            Self {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                files: vec![PackageFile {
                    path: path.to_string(),
                    size,
                    mode: 0o100644,
                    sha256: None,
                    symlink_target: None,
                }],
                extracted_files: vec![ExtractedFile {
                    path: path.to_string(),
                    content: content.to_vec(),
                    size,
                    mode: 0o100644,
                    sha256: None,
                    symlink_target: None,
                }],
                dependencies: Vec::new(),
                scriptlets: Vec::new(),
            }
        }
    }

    impl PackageFormat for FakePackage {
        fn parse(_path: &str) -> conary_core::Result<Self> {
            unimplemented!("test package is constructed directly")
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn version(&self) -> &str {
            &self.version
        }

        fn architecture(&self) -> Option<&str> {
            Some("x86_64")
        }

        fn description(&self) -> Option<&str> {
            None
        }

        fn files(&self) -> &[PackageFile] {
            &self.files
        }

        fn dependencies(&self) -> &[Dependency] {
            &self.dependencies
        }

        fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> {
            Ok(self.extracted_files.clone())
        }

        fn scriptlets(&self) -> &[Scriptlet] {
            &self.scriptlets
        }

        fn to_trove(&self) -> Trove {
            Trove::new(self.name.clone(), self.version.clone(), TroveType::Package)
        }
    }

    #[test]
    fn install_inner_replaces_live_root_owned_overlapping_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();

        let mut live_root = Trove::new(
            "conary-live-root".to_string(),
            "2026.05.14".to_string(),
            TroveType::Package,
        );
        let live_root_id = live_root.insert(&conn).unwrap();
        let mut live_file = FileEntry::new(
            "/boot/grub2/grub.cfg".to_string(),
            "old-live-root-hash".to_string(),
            4,
            0o100644,
            live_root_id,
        );
        live_file.insert(&conn).unwrap();

        let package = FakePackage::with_file("grub2", "/boot/grub2/grub.cfg", b"new-grub");
        let extraction = ExtractionResult {
            extracted_files: package.extracted_files.clone(),
            classified: HashMap::from([(
                conary_core::components::ComponentType::Runtime,
                vec!["/boot/grub2/grub.cfg".to_string()],
            )]),
            component_names_by_path: None,
            installed_component_names: None,
            ccs_pre_remove_script: None,
            installed_component_types: vec![conary_core::components::ComponentType::Runtime],
            skipped_components: Vec::new(),
            language_provides: Vec::new(),
        };
        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();
        let ctx = TransactionContext {
            db_path: &db_path_string,
            root: &root_string,
            semantics: InstallSemantics::ccs(),
            selection_reason: None,
            old_trove_to_upgrade: None,
            ccs_manifest_provides: None,
            ccs_capabilities: None,
            defer_generation: true,
        };
        let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
        let mut engine = TransactionEngine::new(tx_config).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let changeset_id = Changeset::new("Install grub2-1.0.0".to_string())
            .insert(&tx)
            .unwrap();

        install_inner(
            &tx,
            &mut engine,
            changeset_id,
            &package,
            &extraction,
            &ctx,
            &InstallProgress::single("Installing"),
        )
        .unwrap();
        tx.commit().unwrap();

        let owner = FileEntry::find_by_path(&conn, "/boot/grub2/grub.cfg")
            .unwrap()
            .and_then(|file| Trove::find_by_id(&conn, file.trove_id).unwrap())
            .unwrap();
        assert_eq!(owner.name, "grub2");
    }

    #[test]
    fn store_install_files_in_cas_preserves_symlink_targets() {
        let temp = tempfile::tempdir().unwrap();
        let config = TransactionConfig::new(temp.path());
        let engine = TransactionEngine::new(config).unwrap();
        let package = FakePackage {
            name: "fixture".to_string(),
            version: "1.0.0".to_string(),
            files: vec![],
            extracted_files: vec![ExtractedFile {
                path: "/usr/bin/fixture-link".to_string(),
                content: Vec::new(),
                size: 7,
                mode: 0o120777,
                sha256: None,
                symlink_target: Some("fixture".to_string()),
            }],
            dependencies: Vec::new(),
            scriptlets: Vec::new(),
        };
        let extraction = ExtractionResult {
            extracted_files: package.extracted_files.clone(),
            classified: HashMap::from([(
                conary_core::components::ComponentType::Runtime,
                vec!["/usr/bin/fixture-link".to_string()],
            )]),
            component_names_by_path: None,
            installed_component_names: None,
            ccs_pre_remove_script: None,
            installed_component_types: vec![conary_core::components::ComponentType::Runtime],
            skipped_components: Vec::new(),
            language_provides: Vec::new(),
        };

        let stored = store_install_files_in_cas(&engine, &extraction).unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].path, "/usr/bin/fixture-link");
        assert_eq!(stored[0].symlink_target.as_deref(), Some("fixture"));
        assert!(!stored[0].hash.is_empty());
    }
}
