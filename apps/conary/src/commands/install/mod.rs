// src/commands/install/mod.rs
//! Package installation commands

mod acquire;
mod batch;
mod blocklist;
mod ccs_transaction;
mod command;
mod conversion;
mod dep_mode;
mod dep_resolution;
mod dependencies;
mod execute;
mod inner;
mod legacy_replay;
mod lifecycle;
mod options;
mod prepare;
mod resolve;
mod restore;
mod scriptlets;
mod semantics;
mod source_policy;
mod system_pm;
mod transaction;
mod validation;

pub use batch::{BatchInstaller, prepare_package_for_batch};
pub use blocklist::is_blocked as is_package_blocked;
pub use command::cmd_install;
pub use dep_mode::DepMode;
pub(crate) use dependencies::resolve_default_dep_mode_from_model;

#[allow(unused_imports)]
pub(crate) use ccs_transaction::{
    CcsTransactionInstallOptions, CcsTransactionInstallResult, install_ccs_package_transactionally,
    install_ccs_package_transactionally_with_config,
};

pub use legacy_replay::LegacyReplayOptions;
#[allow(unused_imports)]
pub(crate) use legacy_replay::{
    AcceptedLegacyBundleInstall, LegacyReplayAuditContext, LegacyReplayInstallState,
};
pub(super) use legacy_replay::{
    merge_old_upgrade_legacy_replay_state, plan_ccs_fresh_install_legacy_replay,
    plan_ccs_old_installed_upgrade_legacy_replay,
};
pub use options::InstallOptions;
pub(crate) use options::{
    RepositoryInstallProvenance, repository_install_provenance_from_package,
    verify_static_repository_ccs_package_if_needed,
};
pub use prepare::{ComponentSelection, UpgradeCheck};
pub(crate) use restore::{
    add_prepared_install_to_target_state, build_target_state_view,
    finalize_prepared_install_without_snapshot, install_prepared_inner,
    prepare_install_for_restore, run_pre_install_for_prepared,
    validate_prepared_install_dependencies,
};

use super::progress::{InstallPhase, InstallProgress};
use super::{PackageFormatType, detect_package_format};
use execute::{
    PackageExecutionPath, live_root_files_from_stored_files,
    preflight_extracted_live_root_file_ownership, prepare_install_environment_before_scriptlets,
    run_triggers,
};
use lifecycle::{
    ExtractionResult, FinalizeInstallOutput, PreScriptletState, ScriptletContext,
    extract_and_classify_files, finalize_install, finalize_install_without_snapshot,
    mark_upgraded_parent_deriveds_stale, run_pre_install_phase, show_dry_run_summary,
};
use prepare::check_upgrade_status;
use semantics::{InstallSemantics, PreparedSourceKind, scheme_to_string};
use source_policy::{build_resolution_policy, resolve_canonical_name};
use transaction::{
    InstallTransactionResult, TransactionContext, execute_install_transaction,
    execute_install_transaction_with_config,
};
