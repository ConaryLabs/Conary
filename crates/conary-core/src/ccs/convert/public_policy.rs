// conary-core/src/ccs/convert/public_policy.rs

use crate::ccs::legacy_scriptlets::EffectReplacement;
use crate::ccs::legacy_scriptlets::LegacyScriptletEntry;
use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};
use std::collections::BTreeSet;

pub(crate) const FILE_CAPABILITY_PUBLIC_REVIEW_REASON: &str =
    "public-policy-file-capability-private-review";
pub(crate) const SYSCTL_PUBLIC_REVIEW_REASON: &str =
    "public-policy-sysctl-target-profile-unsupported";

const PUBLIC_READY_FILE_CAPABILITIES: &[&str] = &["cap_net_bind_service"];

pub(crate) fn file_capability_public_review_reason(
    capabilities: &[String],
) -> Option<&'static str> {
    let all_public_ready = !capabilities.is_empty()
        && capabilities
            .iter()
            .all(|capability| PUBLIC_READY_FILE_CAPABILITIES.contains(&capability.as_str()));

    (!all_public_ready).then_some(FILE_CAPABILITY_PUBLIC_REVIEW_REASON)
}

pub(crate) fn sysctl_public_review_reason(
    key: &str,
    profile: Option<&dyn TargetProfileQuery>,
) -> Option<&'static str> {
    match profile.map(|profile| profile.sysctl_status(key)) {
        Some(ProfileConstraintStatus::Accepted) => None,
        Some(ProfileConstraintStatus::Unsupported) | None => Some(SYSCTL_PUBLIC_REVIEW_REASON),
    }
}

pub(crate) fn entry_public_policy_review_reasons(
    entry: &LegacyScriptletEntry,
    profile: Option<&dyn TargetProfileQuery>,
) -> Vec<String> {
    let mut reasons = BTreeSet::new();

    for effect in &entry.effects {
        if effect.adapter_id.as_deref() == Some("file-capability/v1")
            && effect.kind == "file-capability"
        {
            let capabilities = effect
                .extra
                .get("capabilities")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();

            if let Some(reason) = file_capability_public_review_reason(&capabilities) {
                reasons.insert(reason.to_string());
            }
        }

        // kind "sysctl-setting" is defined by SysctlAdapter::classify in
        // crates/conary-core/src/ccs/convert/adapters.rs.
        if effect.adapter_id.as_deref() == Some("sysctl/v1")
            && effect.kind == "sysctl-setting"
            && effect.replacement == EffectReplacement::Complete
        {
            let key = effect
                .extra
                .get("key")
                .and_then(toml::Value::as_str)
                .or(effect.path.as_deref());
            let review_reason = key.map_or(Some(SYSCTL_PUBLIC_REVIEW_REASON), |key| {
                sysctl_public_review_reason(key, profile)
            });
            if let Some(reason) = review_reason {
                reasons.insert(reason.to_string());
            }
        }
    }

    reasons.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::legacy_scriptlets::{
        EffectConfidence, EffectReplacement, EffectSource, LifecyclePath, NativeInvocation,
        ScriptletDecision, ScriptletEffect, TransactionOrder,
    };
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};
    use std::collections::BTreeMap;

    fn caps(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    struct SysctlProfile {
        accepted: &'static str,
    }

    impl TargetProfileQuery for SysctlProfile {
        fn sysctl_status(&self, key: &str) -> ProfileConstraintStatus {
            if key == self.accepted {
                ProfileConstraintStatus::Accepted
            } else {
                ProfileConstraintStatus::Unsupported
            }
        }

        fn service_status(&self, _service: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn user_status(&self, _user: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn group_status(&self, _group: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn directory_status(&self, _directory: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }
    }

    fn sysctl_entry_with_replacement(
        key: &str,
        replacement: EffectReplacement,
    ) -> LegacyScriptletEntry {
        let mut extra = BTreeMap::new();
        extra.insert("key".to_string(), toml::Value::String(key.to_string()));

        LegacyScriptletEntry {
            id: "scriptlet:0:post-install".to_string(),
            native_slot: "%post".to_string(),
            phase: LifecyclePath::PostInstall,
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: crate::hash::sha256_prefixed(format!("sysctl:{key}").as_bytes()),
            body: format!("sysctl -w {key}=1\n"),
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                ..TransactionOrder::default()
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Replaced,
            reason_code: "helper-complete-sysctl".to_string(),
            human_reason: None,
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            effects: vec![ScriptletEffect {
                kind: "sysctl-setting".to_string(),
                source: EffectSource::ShellAst,
                confidence: EffectConfidence::Declared,
                replacement,
                adapter_id: Some("sysctl/v1".to_string()),
                adapter_digest: None,
                command: Some("sysctl".to_string()),
                args: vec!["-w".to_string(), format!("{key}=1")],
                path: None,
                reason_code: Some("helper-complete-sysctl".to_string()),
                extra,
            }],
            unknown_command_evidence: Vec::new(),
            blocked_classes: Vec::new(),
            boot_security_intents: Vec::new(),
            security_policy_intents: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn sysctl_entry(key: &str) -> LegacyScriptletEntry {
        sysctl_entry_with_replacement(key, EffectReplacement::Complete)
    }

    #[test]
    fn cap_net_bind_service_is_public_ready_by_default() {
        assert_eq!(
            file_capability_public_review_reason(&caps(&["cap_net_bind_service"])),
            None
        );
    }

    #[test]
    fn high_risk_known_capabilities_require_private_review() {
        for capability in [
            "cap_sys_admin",
            "cap_sys_module",
            "cap_sys_rawio",
            "cap_sys_boot",
            "cap_sys_ptrace",
            "cap_bpf",
            "cap_net_admin",
            "cap_setpcap",
            "cap_setfcap",
        ] {
            assert_eq!(
                file_capability_public_review_reason(&caps(&[capability])),
                Some(FILE_CAPABILITY_PUBLIC_REVIEW_REASON),
                "{capability}"
            );
        }
    }

    #[test]
    fn mixed_public_and_private_capabilities_require_private_review() {
        assert_eq!(
            file_capability_public_review_reason(&caps(
                &["cap_net_bind_service", "cap_sys_admin",]
            )),
            Some(FILE_CAPABILITY_PUBLIC_REVIEW_REASON)
        );
    }

    #[test]
    fn sysctl_public_review_reason_respects_target_profile_support() {
        let profile = SysctlProfile {
            accepted: "kernel.example",
        };

        assert_eq!(
            sysctl_public_review_reason("kernel.example", Some(&profile)),
            None
        );
        assert_eq!(
            sysctl_public_review_reason("net.ipv4.ip_forward", Some(&profile)),
            Some(SYSCTL_PUBLIC_REVIEW_REASON)
        );
        assert_eq!(
            sysctl_public_review_reason("kernel.example", None),
            Some(SYSCTL_PUBLIC_REVIEW_REASON)
        );
    }

    #[test]
    fn sysctl_public_review_reason_treats_whitespace_padded_keys_as_private_review() {
        let profile = SysctlProfile {
            accepted: "kernel.example",
        };

        assert_eq!(
            sysctl_public_review_reason(" kernel.example ", Some(&profile)),
            Some(SYSCTL_PUBLIC_REVIEW_REASON)
        );
    }

    #[test]
    fn sysctl_entries_require_review_when_target_profile_does_not_accept_them() {
        let profile = SysctlProfile {
            accepted: "kernel.example",
        };

        assert_eq!(
            entry_public_policy_review_reasons(&sysctl_entry("kernel.example"), Some(&profile)),
            Vec::<String>::new()
        );
        assert_eq!(
            entry_public_policy_review_reasons(
                &sysctl_entry("net.ipv4.ip_forward"),
                Some(&profile)
            ),
            vec![SYSCTL_PUBLIC_REVIEW_REASON.to_string()]
        );
        assert_eq!(
            entry_public_policy_review_reasons(&sysctl_entry("kernel.example"), None),
            vec![SYSCTL_PUBLIC_REVIEW_REASON.to_string()]
        );
    }

    #[test]
    fn partial_sysctl_effects_do_not_count_as_public_policy_evidence() {
        let profile = SysctlProfile {
            accepted: "kernel.example",
        };

        assert_eq!(
            entry_public_policy_review_reasons(
                &sysctl_entry_with_replacement("net.ipv4.ip_forward", EffectReplacement::Partial),
                Some(&profile)
            ),
            Vec::<String>::new()
        );
    }
}
