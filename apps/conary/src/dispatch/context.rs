// apps/conary/src/dispatch/context.rs

use std::borrow::Cow;

use anyhow::Result;

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
