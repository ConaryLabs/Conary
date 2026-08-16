// apps/conary/src/commands/try_session/install.rs
//! Scratch install planning for try sessions.

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::db::models::TrySessionMode;
use conary_core::generation::root_manifest::CapturedSelectedRoot;
use conary_core::packages::PackageFormat;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::TransactionConfig;
use std::path::{Path, PathBuf};

use crate::commands::install::{
    CcsTransactionInstallOptions, install_ccs_package_transactionally_in_selected_root,
};

#[derive(Debug, Clone)]
pub(super) struct TryInstallPlan {
    pub(super) install_root: PathBuf,
    pub(super) copied_db_path: PathBuf,
    pub(super) runtime_root: ConaryRuntimeRoot,
}

pub(super) fn install_try_package(
    conn: &mut rusqlite::Connection,
    package: &CcsPackage,
    plan: &TryInstallPlan,
) -> Result<CapturedSelectedRoot> {
    let db_path_string = plan.copied_db_path.to_string_lossy().into_owned();
    let selected_session_dir = plan
        .install_root
        .parent()
        .expect("try selected root has a session directory")
        .to_path_buf();
    let mut selected =
        crate::commands::generation::selected_root::SelectedRootSession::begin_for_try(
            conn,
            &plan.runtime_root,
            selected_session_dir,
            format!("Try {}-{}", package.name(), package.version()),
        )?;
    let root_string = selected.selected_root().to_string_lossy().into_owned();
    install_ccs_package_transactionally_in_selected_root(
        conn,
        package,
        CcsTransactionInstallOptions {
            db_path: &db_path_string,
            root: &root_string,
            dry_run: false,
            defer_generation: true,
            quiet: true,
            sandbox_mode: conary_core::scriptlet::SandboxMode::Always,
            allow_downgrade: false,
            intent: crate::commands::install::InstallIntent::PackageChange,
            reinstall: false,
            selection_reason: Some("conary try"),
            selected_manifest_components: None,
            repository_provenance: None,
            requested_source_identity: None,
        },
        &mut selected,
    )?;
    let (_, captured) = selected.capture_preserving_root(&plan.runtime_root)?;
    Ok(captured)
}

pub(super) fn build_try_install_plan(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_db_path: PathBuf,
    _mode: TrySessionMode,
) -> TryInstallPlan {
    TryInstallPlan {
        install_root: work_dir.join("selected-root-session/root"),
        copied_db_path,
        runtime_root: runtime_root.clone(),
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
