// apps/conary/src/live_host_safety.rs

use anyhow::bail;
use std::borrow::Cow;

pub enum LiveMutationClass {
    AlwaysLive,
    CurrentlyLiveEvenWithRootArguments,
}

pub struct LiveMutationRequest {
    pub command_label: Cow<'static, str>,
    pub class: LiveMutationClass,
    pub dry_run: bool,
}

pub fn require_live_system_mutation_ack(
    allow_live_system_mutation: bool,
    request: &LiveMutationRequest,
) -> anyhow::Result<()> {
    if request.dry_run || allow_live_system_mutation {
        return Ok(());
    }

    let mut message = format!(
        "command '{}' may mutate the active host. Conary is still early software, and this command can perform generation rebuild or activation work, remount /usr, rewrite the live /etc overlay, execute scriptlet hooks, or change package ownership during takeover or rollback.",
        request.command_label
    );

    if matches!(
        request.class,
        LiveMutationClass::CurrentlyLiveEvenWithRootArguments
    ) {
        message.push_str(
            " Current --root or similar arguments are not sufficient isolation for this command yet.",
        );
    }

    message.push_str(
        " Rerun with --allow-live-system-mutation only if you intend to modify the real machine.",
    );

    bail!("{message}")
}

#[cfg(test)]
mod tests {
    use super::{LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack};
    use std::borrow::Cow;

    #[test]
    fn dry_run_bypasses_live_mutation_ack() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: true,
        };

        assert!(require_live_system_mutation_ack(false, &request).is_ok());
    }

    #[test]
    fn missing_ack_mentions_early_software_rationale() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        };

        let err = require_live_system_mutation_ack(false, &request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("--allow-live-system-mutation"));
        assert!(message.contains("early software"));
    }

    #[test]
    fn allow_live_mutation_ack_passes() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        };

        assert!(require_live_system_mutation_ack(true, &request).is_ok());
    }

    #[test]
    fn refusal_lists_live_host_risks() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary system generation switch"),
            class: LiveMutationClass::AlwaysLive,
            dry_run: false,
        };

        let err = require_live_system_mutation_ack(false, &request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("conary system generation switch"));
        assert!(message.contains("mutate the active host"));
        assert!(message.contains("/usr"));
        assert!(message.contains("/etc"));
        assert!(message.contains("scriptlet"));
    }

    #[test]
    fn currently_live_root_command_mentions_root_is_not_isolation_yet() {
        let request = LiveMutationRequest {
            command_label: Cow::Borrowed("conary ccs install"),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run: false,
        };

        let err = require_live_system_mutation_ack(false, &request).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("conary ccs install"));
        assert!(message.contains("--root"));
        assert!(message.contains("not sufficient isolation"));
    }
}
