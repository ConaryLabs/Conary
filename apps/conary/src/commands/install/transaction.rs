// src/commands/install/transaction.rs

use super::{
    AcceptedLegacyBundleInstall, ExtractionResult, InstallProgress, InstallSemantics,
    LegacyReplayOptions, PackageExecutionPath, RepositoryInstallProvenance, inner,
    live_root_files_from_stored_files,
};
use anyhow::{Context, Result};
use conary_core::db::models::{Changeset, ChangesetStatus, ProvideEntry};
use conary_core::dependencies::DependencyClass;
use conary_core::packages::PackageFormat;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::path::{Path, PathBuf};
use tracing::info;

/// Context for the transaction execution phase.
pub(super) struct TransactionContext<'a> {
    pub(super) db_path: &'a str,
    pub(super) root: &'a str,
    pub(super) semantics: InstallSemantics,
    pub(super) selection_reason: Option<&'a str>,
    pub(super) old_trove_to_upgrade: Option<&'a conary_core::db::models::Trove>,
    pub(super) ccs_manifest_provides: Option<&'a conary_core::ccs::manifest::Provides>,
    pub(super) ccs_capabilities: Option<&'a conary_core::capability::CapabilityDeclaration>,
    pub(super) execution_path: PackageExecutionPath,
    pub(super) defer_generation: bool,
    pub(super) repository_provenance: Option<RepositoryInstallProvenance>,
    pub(super) legacy_replay: LegacyReplayOptions,
    #[allow(dead_code)]
    pub(super) accepted_legacy_bundle: Option<&'a AcceptedLegacyBundleInstall>,
}

/// Result from a successful transaction execution.
pub(super) struct InstallTransactionResult {
    pub(super) changeset_id: i64,
}

/// Execute the main install transaction: filesystem changes + DB commit.
pub(super) fn execute_install_transaction(
    conn: &mut rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
) -> Result<InstallTransactionResult> {
    execute_install_transaction_inner(conn, pkg, extraction, ctx, progress, None)
}

pub(super) fn execute_install_transaction_with_config(
    conn: &mut rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
    transaction_config: TransactionConfig,
) -> Result<InstallTransactionResult> {
    execute_install_transaction_inner(
        conn,
        pkg,
        extraction,
        ctx,
        progress,
        Some(transaction_config),
    )
}

fn execute_install_transaction_inner(
    conn: &mut rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
    transaction_config_override: Option<TransactionConfig>,
) -> Result<InstallTransactionResult> {
    let _legacy_replay = ctx.legacy_replay;
    if ctx.execution_path == PackageExecutionPath::MutableLiveRoot {
        inner::preflight_live_root_file_ownership(
            conn,
            extraction
                .extracted_files
                .iter()
                .map(|file| file.path.as_str()),
            pkg.name(),
        )?;
    }

    let db_path_buf = PathBuf::from(ctx.db_path);
    let skip_recovery = transaction_config_override.is_some();
    let tx_config = transaction_config_override
        .unwrap_or_else(|| TransactionConfig::from_paths(PathBuf::from(ctx.root), db_path_buf));
    let mut engine =
        TransactionEngine::new(tx_config).context("Failed to create transaction engine")?;

    if !skip_recovery {
        engine
            .recover(conn)
            .context("Failed to recover incomplete transactions")?;
    }

    let tx_description = if let Some(old_trove) = ctx.old_trove_to_upgrade {
        format!(
            "Upgrade {} from {} to {}",
            pkg.name(),
            old_trove.version,
            pkg.version()
        )
    } else {
        format!("Install {}-{}", pkg.name(), pkg.version())
    };
    engine.begin().context("Failed to begin transaction")?;

    if ctx.execution_path == PackageExecutionPath::MutableLiveRoot {
        let result = (|| -> Result<InstallTransactionResult> {
            let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(
                PathBuf::from(ctx.db_path),
            );
            crate::commands::live_root::recover_pending_journals_with_changesets(
                runtime_root.root(),
                Path::new(ctx.root),
                conn,
            )?;

            let tx_uuid = uuid::Uuid::new_v4().to_string();
            let mut changeset = Changeset::with_tx_uuid(tx_description.clone(), tx_uuid.clone());
            let stored_files = inner::store_install_files_in_cas(&engine, extraction)?;
            let live_files = live_root_files_from_stored_files(engine.cas(), &stored_files)?;
            let mut live_tx = crate::commands::LiveRootTransaction::begin(
                runtime_root.root(),
                Path::new(ctx.root),
                tx_uuid,
                tx_description.clone(),
            )?;
            live_tx.apply_install_files(&live_files)?;

            let tx = conn.unchecked_transaction()?;
            let db_result = (|| -> Result<i64> {
                let changeset_id = changeset.insert(&tx)?;
                let inner_result = inner::install_inner_with_stored_files(
                    &tx,
                    changeset_id,
                    pkg,
                    extraction,
                    ctx,
                    &stored_files,
                )?;
                if let Some(provides) = ctx.ccs_manifest_provides {
                    persist_ccs_manifest_provides(
                        &tx,
                        inner_result.trove_id,
                        pkg.name(),
                        provides,
                    )?;
                }
                if let Some(capabilities) = ctx.ccs_capabilities {
                    conary_core::capability::store_capabilities(
                        &tx,
                        inner_result.trove_id,
                        capabilities,
                    )?;
                }
                changeset.update_status(&tx, ChangesetStatus::Applied)?;
                Ok(changeset_id)
            })();
            let changeset_id = match db_result {
                Ok(changeset_id) => changeset_id,
                Err(error) => {
                    live_tx.rollback()?;
                    return Err(error);
                }
            };
            if let Err(error) = tx.commit() {
                if let Err(rollback_error) = live_tx.rollback() {
                    return Err(error)
                        .context(format!("Failed to rollback live root: {rollback_error}"));
                }
                return Err(error.into());
            }
            live_tx.commit()?;

            Ok(InstallTransactionResult { changeset_id })
        })();
        engine.release_lock();
        return result;
    }

    // Capture /etc snapshot BEFORE the DB transaction so the three-way merge
    // can distinguish pre- from post-install state.
    let prev_etc = crate::commands::composefs_ops::collect_etc_files(conn)?;

    let mut changeset = Changeset::new(tx_description.clone());
    let tx = conn.unchecked_transaction()?;
    let changeset_id = changeset.insert(&tx)?;

    let inner_result = match inner::install_inner(
        &tx,
        &mut engine,
        changeset_id,
        pkg,
        extraction,
        ctx,
        progress,
    ) {
        Ok(result) => result,
        Err(e) => {
            engine.release_lock();
            return Err(e);
        }
    };
    if let Some(provides) = ctx.ccs_manifest_provides {
        persist_ccs_manifest_provides(&tx, inner_result.trove_id, pkg.name(), provides)?;
    }
    if let Some(capabilities) = ctx.ccs_capabilities {
        conary_core::capability::store_capabilities(&tx, inner_result.trove_id, capabilities)?;
    }

    changeset.update_status(&tx, ChangesetStatus::Applied)?;
    if ctx.defer_generation && ctx.execution_path == PackageExecutionPath::GenerationAware {
        let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(ctx.db_path);
        conary_core::db::models::GenerationPublication::create_pending(
            &tx,
            Some(changeset_id),
            changeset.tx_uuid.as_deref(),
            ctx.db_path,
            &runtime_root.root().display().to_string(),
            &tx_description,
        )?;
        crate::commands::append_deferred_follow_up_metadata(
            &tx,
            changeset_id,
            crate::commands::publication_deferred_follow_up(
                "generation publication was deferred by caller request".to_string(),
            ),
        )?;
    }

    tx.commit()?;
    info!(
        "DB commit successful: changeset={}, trove={}",
        changeset_id, inner_result.trove_id
    );

    if ctx.defer_generation && ctx.execution_path == PackageExecutionPath::GenerationAware {
        engine.release_lock();
        return Ok(InstallTransactionResult { changeset_id });
    }

    let post_commit_result = (|| -> Result<()> {
        let outcome = crate::commands::generation::publication::publish_current_db_state(
            conn,
            crate::commands::generation::publication::PublicationRequest {
                db_path: ctx.db_path,
                summary: &tx_description,
                trigger_changeset_id: Some(changeset_id),
                tx_uuid: changeset.tx_uuid.as_deref(),
                prev_etc_snapshot: Some(prev_etc),
            },
        )?;
        if outcome.needs_publication {
            crate::commands::append_deferred_follow_up_metadata(
                conn,
                changeset_id,
                crate::commands::publication_deferred_follow_up(
                    "generation publication is pending".to_string(),
                ),
            )?;
            crate::commands::generation::publication::warn_if_publication_pending(
                changeset_id,
                &outcome,
            );
        }
        Ok(())
    })();
    engine.release_lock();
    post_commit_result?;

    Ok(InstallTransactionResult { changeset_id })
}

fn persist_ccs_manifest_provides(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    package_name: &str,
    provides: &conary_core::ccs::manifest::Provides,
) -> Result<()> {
    for capability in &provides.capabilities {
        if capability == package_name {
            continue;
        }
        let mut provide = ProvideEntry::new(trove_id, capability.clone(), None);
        provide.insert_or_ignore(tx)?;
    }

    for soname in &provides.sonames {
        insert_ccs_manifest_typed_provide(tx, trove_id, DependencyClass::Soname.prefix(), soname)?;
    }

    for binary in &provides.binaries {
        insert_ccs_manifest_typed_provide(tx, trove_id, DependencyClass::Binary.prefix(), binary)?;
    }

    for module in &provides.pkgconfig {
        insert_ccs_manifest_typed_provide(
            tx,
            trove_id,
            DependencyClass::PkgConfig.prefix(),
            module,
        )?;
    }

    Ok(())
}

fn insert_ccs_manifest_typed_provide(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    kind: &str,
    capability: &str,
) -> Result<()> {
    let mut provide = ProvideEntry::new_typed(trove_id, kind, capability.to_string(), None);
    provide.insert_or_ignore(tx)?;

    tx.execute(
        "UPDATE provides
         SET kind = ?3
         WHERE trove_id = ?1
           AND capability = ?2
           AND kind = 'package'",
        rusqlite::params![trove_id, capability, kind],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::PackageFormatType;

    #[test]
    fn no_generation_install_transaction_materializes_live_root_file() {
        use conary_core::db::models::{Changeset, ChangesetStatus, FileEntry, Trove, TroveType};
        use conary_core::packages::traits::{
            Dependency, ExtractedFile, PackageFile, PackageFormat, Scriptlet,
        };
        use std::collections::HashMap;

        struct FakePackage;

        impl PackageFormat for FakePackage {
            fn parse(_path: &str) -> conary_core::Result<Self> {
                unreachable!("test constructs package directly")
            }

            fn name(&self) -> &str {
                "fixture"
            }

            fn version(&self) -> &str {
                "1.0.0"
            }

            fn architecture(&self) -> Option<&str> {
                Some("x86_64")
            }

            fn description(&self) -> Option<&str> {
                None
            }

            fn files(&self) -> &[PackageFile] {
                &[]
            }

            fn dependencies(&self) -> &[Dependency] {
                &[]
            }

            fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> {
                Ok(vec![])
            }

            fn scriptlets(&self) -> &[Scriptlet] {
                &[]
            }

            fn to_trove(&self) -> Trove {
                Trove::new(
                    "fixture".to_string(),
                    "1.0.0".to_string(),
                    TroveType::Package,
                )
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let extraction = ExtractionResult {
            extracted_files: vec![ExtractedFile {
                path: "/usr/bin/fixture".to_string(),
                content: b"fixture".to_vec(),
                size: 7,
                mode: 0o100755,
                sha256: None,
                symlink_target: None,
            }],
            classified: HashMap::from([(
                conary_core::components::ComponentType::Runtime,
                vec!["/usr/bin/fixture".to_string()],
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
            semantics: InstallSemantics::legacy(PackageFormatType::Rpm),
            selection_reason: None,
            old_trove_to_upgrade: None,
            ccs_manifest_provides: None,
            ccs_capabilities: None,
            execution_path: PackageExecutionPath::MutableLiveRoot,
            defer_generation: false,
            repository_provenance: None,
            legacy_replay: LegacyReplayOptions::default(),
            accepted_legacy_bundle: None,
        };

        assert!(ctx.accepted_legacy_bundle.is_none());

        let result = execute_install_transaction(
            &mut conn,
            &FakePackage,
            &extraction,
            &ctx,
            &InstallProgress::single("Installing"),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
            "fixture"
        );
        assert!(
            FileEntry::find_by_path(&conn, "/usr/bin/fixture")
                .unwrap()
                .is_some()
        );
        let changeset = Changeset::find_by_id(&conn, result.changeset_id)
            .unwrap()
            .unwrap();
        assert_eq!(changeset.status, ChangesetStatus::Applied);
        let journal_dir = temp.path().join("live-root-journals");
        assert!(!journal_dir.exists() || std::fs::read_dir(&journal_dir).unwrap().next().is_none());
    }

    #[test]
    fn no_generation_install_conflict_preflight_preserves_live_root_file() {
        use conary_core::db::models::{FileEntry, Trove, TroveType};
        use conary_core::packages::traits::{
            Dependency, ExtractedFile, PackageFile, PackageFormat, Scriptlet,
        };
        use std::collections::HashMap;
        use std::os::unix::fs::PermissionsExt;

        struct FakePackage;

        impl PackageFormat for FakePackage {
            fn parse(_path: &str) -> conary_core::Result<Self> {
                unreachable!("test constructs package directly")
            }

            fn name(&self) -> &str {
                "fixture"
            }

            fn version(&self) -> &str {
                "1.0.0"
            }

            fn architecture(&self) -> Option<&str> {
                Some("x86_64")
            }

            fn description(&self) -> Option<&str> {
                None
            }

            fn files(&self) -> &[PackageFile] {
                &[]
            }

            fn dependencies(&self) -> &[Dependency] {
                &[]
            }

            fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> {
                Ok(vec![])
            }

            fn scriptlets(&self) -> &[Scriptlet] {
                &[]
            }

            fn to_trove(&self) -> Trove {
                Trove::new(
                    "fixture".to_string(),
                    "1.0.0".to_string(),
                    TroveType::Package,
                )
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        let live_file = root.join("usr/bin/fixture");
        std::fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        std::fs::write(&live_file, "owned elsewhere").unwrap();
        let mut perms = std::fs::metadata(&live_file).unwrap().permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&live_file, perms).unwrap();

        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let mut other_trove = Trove::new(
            "other-owner".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let other_trove_id = other_trove.insert(&conn).unwrap();
        let mut existing = FileEntry::new(
            "/usr/bin/fixture".to_string(),
            "other-hash".to_string(),
            15,
            0o100755,
            other_trove_id,
        );
        existing.insert(&conn).unwrap();
        let mut runtime_perms = std::fs::metadata(temp.path()).unwrap().permissions();
        runtime_perms.set_mode(0o555);
        std::fs::set_permissions(temp.path(), runtime_perms).unwrap();

        let extraction = ExtractionResult {
            extracted_files: vec![ExtractedFile {
                path: "/usr/bin/fixture".to_string(),
                content: b"replacement".to_vec(),
                size: 11,
                mode: 0o100755,
                sha256: None,
                symlink_target: None,
            }],
            classified: HashMap::from([(
                conary_core::components::ComponentType::Runtime,
                vec!["/usr/bin/fixture".to_string()],
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
            semantics: InstallSemantics::legacy(PackageFormatType::Rpm),
            selection_reason: None,
            old_trove_to_upgrade: None,
            ccs_manifest_provides: None,
            ccs_capabilities: None,
            execution_path: PackageExecutionPath::MutableLiveRoot,
            defer_generation: false,
            repository_provenance: None,
            legacy_replay: LegacyReplayOptions::default(),
            accepted_legacy_bundle: None,
        };

        let error = match execute_install_transaction(
            &mut conn,
            &FakePackage,
            &extraction,
            &ctx,
            &InstallProgress::single("Installing"),
        ) {
            Ok(_) => panic!("conflicting install unexpectedly succeeded"),
            Err(error) => error,
        };

        let mut runtime_perms = std::fs::metadata(temp.path()).unwrap().permissions();
        runtime_perms.set_mode(0o755);
        std::fs::set_permissions(temp.path(), runtime_perms).unwrap();
        let mut perms = std::fs::metadata(&live_file).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&live_file, perms).unwrap();
        assert!(
            error
                .to_string()
                .contains("Path /usr/bin/fixture is already tracked by package other-owner"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
            "owned elsewhere"
        );
    }
}
