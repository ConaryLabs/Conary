// apps/conary/src/dispatch/context.rs

use std::borrow::Cow;

use anyhow::Result;

use crate::commands;
use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent,
};

pub(super) fn require_live_mutation(
    intent: MutationIntent,
    command_label: Cow<'static, str>,
    class: LiveMutationClass,
    dry_run: bool,
) -> Result<()> {
    require_mutation_intent(&LiveMutationRequest {
        command_label,
        class,
        dry_run,
        intent,
    })
}

pub(super) fn legacy_replay_options(
    allow_legacy_replay: bool,
    allow_foreign_legacy_replay: bool,
) -> commands::LegacyReplayOptions {
    commands::LegacyReplayOptions {
        allow_legacy_replay,
        allow_foreign_legacy_replay,
    }
}
