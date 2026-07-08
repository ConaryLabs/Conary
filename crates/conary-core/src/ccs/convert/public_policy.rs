// conary-core/src/ccs/convert/public_policy.rs

use crate::ccs::legacy_scriptlets::LegacyScriptletEntry;
use std::collections::BTreeSet;

pub(crate) const FILE_CAPABILITY_PUBLIC_REVIEW_REASON: &str =
    "public-policy-file-capability-private-review";

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

pub(crate) fn entry_public_policy_review_reasons(entry: &LegacyScriptletEntry) -> Vec<String> {
    let mut reasons = BTreeSet::new();

    for effect in &entry.effects {
        if effect.adapter_id.as_deref() != Some("file-capability/v1")
            || effect.kind != "file-capability"
        {
            continue;
        }

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

    reasons.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
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
}
