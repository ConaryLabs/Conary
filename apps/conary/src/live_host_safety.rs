// apps/conary/src/live_host_safety.rs

use anyhow::bail;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationIntent {
    Missing,
    Apply,
}

impl MutationIntent {
    pub fn from_apply_intent(apply_intent: bool) -> Self {
        if apply_intent {
            Self::Apply
        } else {
            Self::Missing
        }
    }

    pub fn is_present(self) -> bool {
        !matches!(self, Self::Missing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveMutationClass {
    AlwaysLive,
    LiveConaryState,
    SelectedRootState,
    CurrentlyLiveEvenWithRootArguments,
}

pub struct LiveMutationRequest {
    pub command_label: Cow<'static, str>,
    pub class: LiveMutationClass,
    pub dry_run: bool,
    pub intent: MutationIntent,
}

pub fn require_mutation_intent(request: &LiveMutationRequest) -> anyhow::Result<()> {
    if request.dry_run || request.intent.is_present() {
        return Ok(());
    }

    let mut message = match request.class {
        LiveMutationClass::LiveConaryState => format!(
            "command '{}' may update Conary DB or CAS metadata for this machine.",
            request.command_label
        ),
        LiveMutationClass::SelectedRootState => format!(
            "command '{}' may update Conary authority and files in the explicitly selected root.",
            request.command_label
        ),
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments => format!(
            "command '{}' may change packages, files, scriptlets, ownership, or the live Conary database.",
            request.command_label
        ),
        LiveMutationClass::AlwaysLive => format!(
            "command '{}' may change generation state, boot selection, publication debt, or recovery state.",
            request.command_label
        ),
    };

    if matches!(
        request.class,
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments
    ) {
        message.push_str(
            " Current --root or similar arguments are not sufficient isolation for this command yet.",
        );
    }

    message.push_str(" Use --dry-run when available to preview first.");
    message.push_str(" Rerun with --yes when you intend to apply this command.");
    if matches!(request.class, LiveMutationClass::AlwaysLive) {
        message.push_str(
            " For generation and recovery operations, verify the concrete generation, boot, or recovery target before applying.",
        );
    }

    bail!("{message}")
}

#[cfg(test)]
mod tests {
    use super::{LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent};
    use std::borrow::Cow;

    #[test]
    fn dry_run_bypasses_live_mutation_ack() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: true,
            intent: MutationIntent::Missing,
        };

        assert!(require_mutation_intent(&request).is_ok());
    }

    #[test]
    fn apply_intent_passes_active_host_mutation() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
            intent: MutationIntent::Apply,
        };

        assert!(require_mutation_intent(&request).is_ok());
    }

    #[test]
    fn missing_apply_intent_mentions_dry_run_and_yes_not_old_global_flag() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
            intent: MutationIntent::Missing,
        };

        let err = require_mutation_intent(&request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("conary install"));
        assert!(message.contains("--dry-run"));
        assert!(message.contains("--yes"));
        assert!(!message.contains("--allow-live-system-mutation"));
        assert!(!message.contains("early software"));
    }

    #[test]
    fn always_live_refusal_names_generation_risk() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary system generation switch"),
            class: LiveMutationClass::AlwaysLive,
            dry_run: false,
            intent: MutationIntent::Missing,
        };

        let err = require_mutation_intent(&request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("generation"));
        assert!(message.contains("boot selection"));
        assert!(message.contains("--yes"));
    }

    #[test]
    fn currently_live_root_command_mentions_root_is_not_isolation_yet() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary ccs install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
            intent: MutationIntent::Missing,
        };

        let err = require_mutation_intent(&request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("conary ccs install"));
        assert!(message.contains("--root"));
        assert!(message.contains("not sufficient isolation"));
    }

    #[test]
    fn live_conary_state_refusal_describes_db_and_cas_not_scriptlets() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary system adopt <pkg>"),
            class: LiveMutationClass::LiveConaryState,
            dry_run: false,
            intent: MutationIntent::Missing,
        };

        let err = require_mutation_intent(&request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("Conary DB"));
        assert!(message.contains("CAS"));
        assert!(message.contains("--yes"));
        assert!(!message.contains("scriptlet hooks"));
        assert!(!message.contains("remount /usr"));
    }
}
