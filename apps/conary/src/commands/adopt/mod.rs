// src/commands/adopt/mod.rs

//! Commands for adopting existing system packages into Conary tracking
//!
//! This module provides the ability to import packages already installed
//! by the system package manager (RPM, dpkg, pacman) into Conary's tracking database.

pub(crate) mod cas_capture;
mod checkpoint;
mod conflicts;
mod convert;
mod hooks;
mod native_handoff;
mod outcome;
mod packages;
mod refresh;
mod status;
mod system;
mod unadopt;

// Re-export all public commands
pub use conflicts::cmd_conflicts;
pub use convert::cmd_adopt_convert;
pub use hooks::cmd_sync_hook_install;
pub use native_handoff::{
    NativeHandoffOptions, NativeHandoffOutcome, NativeHandoffSummary, cmd_native_handoff,
};
pub use packages::cmd_adopt;
pub use refresh::cmd_adopt_refresh;
pub use status::cmd_adopt_status;
pub use system::FileInfoTuple;
pub use system::cmd_adopt_system;
pub use unadopt::{UnadoptOptions, cmd_unadopt};

#[cfg(test)]
mod db_checkpoint_tests {
    use super::checkpoint::write_db_checkpoint;
    use conary_core::db::backup::CheckpointReason;

    #[test]
    fn write_db_checkpoint_records_reason_next_to_runtime_root() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let record = write_db_checkpoint(&db_path, CheckpointReason::PreMutation).unwrap();

        assert_eq!(record.manifest.reason, CheckpointReason::PreMutation);
        assert!(record.backup_path.starts_with(temp.path().join("backups")));
    }
}
