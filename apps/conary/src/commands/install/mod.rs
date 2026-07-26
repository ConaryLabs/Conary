// apps/conary/src/commands/install/mod.rs
//! Package installation commands

mod acquire;
mod batch;
mod ccs_removal_hooks;
mod ccs_transaction;
mod command;
pub(crate) mod config_files;
mod conversion;
mod dep_resolution;
mod dependencies;
mod execute;
mod file_capabilities;
mod inner;
mod lifecycle;
pub(crate) mod native_events;
pub(super) mod native_graph;
mod native_lifecycle;
mod options;
mod ownership_mode;
mod payload_identity;
mod prepare;
mod resolve;
mod restore;
mod rollback_snapshot;
mod semantics;
pub(crate) mod shared_directory;
mod source_policy;
mod transaction;
mod validation;

pub use batch::{BatchInstaller, prepare_package_for_batch};
pub use command::cmd_install;
pub(crate) use command::cmd_install_replatform;
pub use ownership_mode::OwnershipMode;

#[allow(unused_imports)]
pub(crate) use ccs_transaction::{
    CcsTransactionInstallOptions, CcsTransactionInstallResult, install_ccs_package_transactionally,
    install_ccs_package_transactionally_in_selected_root,
};

#[allow(unused_imports)]
pub(crate) use native_lifecycle::NativeLifecycleInstallState;
pub use options::InstallOptions;
pub(crate) use options::{
    CcsEnvelopeAuthority, RepositoryInstallProvenance, repository_install_provenance_from_package,
    verify_ccs_package_authority,
};
pub use prepare::{ComponentSelection, UpgradeCheck};
pub(crate) use restore::{
    add_prepared_install_to_target_state, build_target_state_view,
    execute_state_restore_transaction, prepare_install_for_restore,
    validate_prepared_install_dependencies,
};

use super::progress::{InstallPhase, InstallProgress};
use super::{PackageFormatType, detect_package_format};
use execute::{
    live_root_files_from_stored_files, preflight_extracted_file_ownership, run_triggers,
};
use lifecycle::{
    ExtractionResult, FinalizeInstallOutput, extract_and_classify_files, finalize_install,
    finalize_install_without_snapshot, mark_upgraded_parent_deriveds_stale,
    runtime_requirement_count, show_dry_run_summary,
};
use prepare::check_upgrade_status;
pub(crate) use semantics::InstallIntent;
use semantics::{InstallSemantics, PreparedSourceKind, build_execution_mode};
use source_policy::{
    bind_transaction_source_profile, build_resolution_policy, resolve_canonical_name,
};
use transaction::{
    InstallTransactionResult, TransactionContext, execute_install_transaction_in_selected_root,
    execute_install_transaction_in_selected_root_with_post_graph,
    preflight_generation_file_capabilities_for_install,
};
