// apps/conary/src/commands/try_session/install.rs
//! Scratch install planning for try sessions.

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::db::models::TrySessionMode;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::TransactionConfig;
use std::path::{Path, PathBuf};

use crate::commands::install::{
    CcsTransactionInstallOptions, ComponentSelection, LegacyReplayOptions,
    install_ccs_package_transactionally_with_config,
};

#[derive(Debug, Clone)]
pub(super) struct TryInstallPlan {
    pub(super) install_root: PathBuf,
    pub(super) copied_db_path: PathBuf,
    pub(super) transaction_config: TransactionConfig,
    pub(super) no_scripts: bool,
}

pub(super) fn install_try_package(
    conn: &mut rusqlite::Connection,
    package: &CcsPackage,
    plan: &TryInstallPlan,
) -> Result<()> {
    let db_path_string = plan.copied_db_path.to_string_lossy().into_owned();
    let root_string = plan.install_root.to_string_lossy().into_owned();
    install_ccs_package_transactionally_with_config(
        conn,
        package,
        CcsTransactionInstallOptions {
            db_path: &db_path_string,
            root: &root_string,
            dry_run: false,
            defer_generation: true,
            no_scripts: plan.no_scripts,
            sandbox_mode: conary_core::scriptlet::SandboxMode::None,
            allow_downgrade: false,
            reinstall: false,
            selection_reason: Some("conary try"),
            component_selection: ComponentSelection::All,
            selected_manifest_components: None,
            repository_provenance: None,
            legacy_replay: LegacyReplayOptions::default(),
        },
        plan.transaction_config.clone(),
    )?;
    Ok(())
}

pub(super) fn build_try_install_plan(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_db_path: PathBuf,
    _mode: TrySessionMode,
) -> TryInstallPlan {
    TryInstallPlan {
        install_root: work_dir.join("root"),
        copied_db_path: copied_db_path.clone(),
        transaction_config: build_try_transaction_config(runtime_root, copied_db_path),
        no_scripts: true,
    }
}

pub(super) fn build_try_transaction_config(
    runtime_root: &ConaryRuntimeRoot,
    copied_db_path: PathBuf,
) -> TransactionConfig {
    TransactionConfig {
        root: runtime_root.root().to_path_buf(),
        db_path: copied_db_path,
        objects_dir: runtime_root.objects_dir(),
        generations_dir: runtime_root.generations_dir(),
        etc_state_dir: runtime_root.etc_state_dir(),
        mount_point: runtime_root.mount_dir(),
        hash_algorithm: conary_core::hash::HashAlgorithm::Sha256,
        lock_timeout_secs: TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS,
    }
}
