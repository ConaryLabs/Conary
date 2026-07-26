// apps/conary/src/commands/remove/types.rs

use conary_core::db::models::Trove;
use conary_core::scriptlet::SandboxMode;

use crate::commands::TroveSnapshot;

pub(crate) struct RemoveInnerResult {
    pub(crate) snapshot: TroveSnapshot,
    pub(super) trove: Trove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoveLifecycleOptions {
    pub(super) sandbox_mode: SandboxMode,
    pub(super) purge_config_files: bool,
}

impl RemoveLifecycleOptions {
    pub(crate) fn new(sandbox_mode: SandboxMode) -> Self {
        Self {
            sandbox_mode,
            purge_config_files: false,
        }
    }

    pub(crate) fn with_purge_config_files(mut self, purge: bool) -> Self {
        self.purge_config_files = purge;
        self
    }
}
