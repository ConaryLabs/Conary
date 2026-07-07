// conary-core/src/ccs/convert/security_policy.rs

use crate::ccs::legacy_scriptlets::ScriptletEffect;
use crate::ccs::security_policy::{
    SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyFallback, SecurityPolicyIntent,
    SecurityPolicyPayloadEvidence, SecurityPolicyProvider, SecurityPolicyReconciliation,
    SecurityPolicyReconciliationState, SecurityPolicyRequirements, SecurityPolicyScope,
    SecurityPolicySource,
};
use std::collections::BTreeMap;

pub(super) fn policy_intents_from_effects(
    entry_id: &str,
    effects: &[ScriptletEffect],
) -> Vec<SecurityPolicyIntent> {
    effects
        .iter()
        .filter_map(|effect| policy_intent_from_effect(entry_id, effect))
        .collect()
}

fn policy_intent_from_effect(
    entry_id: &str,
    effect: &ScriptletEffect,
) -> Option<SecurityPolicyIntent> {
    if effect.adapter_id.as_deref() != Some("selinux-policy/v1") {
        return None;
    }

    let operation = effect
        .extra
        .get("selinux_operation")
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| operation_from_kind(&effect.kind))
        .to_string();

    let scope = scope_from_selinux_effect(effect, &operation);
    let payload_paths = payload_paths(effect);
    let mut desired_state = BTreeMap::new();
    for key in [
        "action",
        "selinux_type",
        "expression",
        "payload_root",
        "boolean",
        "value",
        "persistent",
        "module_path",
        "module_format",
        "recursive",
    ] {
        if let Some(value) = effect.extra.get(key) {
            desired_state.insert(key.to_string(), value.clone());
        }
    }

    Some(SecurityPolicyIntent {
        schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
        id: format!("{entry_id}:{}", effect.kind),
        source: SecurityPolicySource {
            source_format: None,
            source_distro: None,
            entry_id: Some(entry_id.to_string()),
            command: effect.command.clone(),
            argv: effect.args.clone(),
            adapter_id: effect.adapter_id.clone(),
        },
        provider: SecurityPolicyProvider::Selinux,
        operation,
        scope,
        desired_state,
        requirements: SecurityPolicyRequirements {
            required_on_active_provider: false,
            provider_mode: None,
            tools: effect.command.iter().cloned().collect(),
            modules: Vec::new(),
        },
        fallback: SecurityPolicyFallback::Dormant,
        payload_evidence: SecurityPolicyPayloadEvidence {
            payload_backed: effect
                .extra
                .get("payload_backed")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
            paths: payload_paths,
            digest: effect.adapter_digest.clone(),
        },
        reconciliation: SecurityPolicyReconciliation {
            state: SecurityPolicyReconciliationState::Pending,
            reason: Some("metadata-only-provider-planning-deferred".to_string()),
            target_provider: None,
        },
        extra: BTreeMap::new(),
    })
}

fn operation_from_kind(kind: &str) -> &str {
    match kind {
        "selinux-label-refresh" => "label-refresh",
        "selinux-file-context" => "file-context",
        "selinux-boolean" => "boolean-set",
        "selinux-policy-module" => "module-install",
        _ => kind,
    }
}

fn scope_from_selinux_effect(effect: &ScriptletEffect, operation: &str) -> SecurityPolicyScope {
    match effect.kind.as_str() {
        "selinux-boolean" => SecurityPolicyScope {
            kind: "boolean".to_string(),
            name: effect
                .extra
                .get("boolean")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            paths: Vec::new(),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        "selinux-policy-module" => SecurityPolicyScope {
            kind: "policy-module".to_string(),
            name: effect.path.clone(),
            paths: effect.path.iter().cloned().collect(),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        _ if operation.contains("file-context") => SecurityPolicyScope {
            kind: "file-context".to_string(),
            name: effect.path.clone(),
            paths: payload_paths(effect),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        _ => SecurityPolicyScope {
            kind: "path".to_string(),
            name: effect.path.clone(),
            paths: payload_paths(effect),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
    }
}

fn payload_paths(effect: &ScriptletEffect) -> Vec<String> {
    if let Some(values) = effect.extra.get("paths").and_then(toml::Value::as_array) {
        return values
            .iter()
            .filter_map(toml::Value::as_str)
            .map(str::to_string)
            .collect();
    }
    effect.path.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::policy_intents_from_effects;
    use crate::ccs::legacy_scriptlets::{
        EffectConfidence, EffectReplacement, EffectSource, ScriptletEffect,
    };
    use std::collections::BTreeMap;

    fn selinux_effect(kind: &str, command: &str) -> ScriptletEffect {
        ScriptletEffect {
            kind: kind.to_string(),
            source: EffectSource::StaticSignal,
            confidence: EffectConfidence::Inferred,
            replacement: EffectReplacement::Complete,
            adapter_id: Some("selinux-policy/v1".to_string()),
            adapter_digest: Some(crate::hash::sha256_prefixed(b"selinux-policy/v1")),
            command: Some(command.to_string()),
            args: Vec::new(),
            path: None,
            reason_code: Some("helper-complete-selinux-policy".to_string()),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn non_lsm_effects_do_not_project_policy_intent() {
        let effect = ScriptletEffect {
            kind: "dynamic-linker-cache".to_string(),
            source: EffectSource::StaticSignal,
            confidence: EffectConfidence::Inferred,
            replacement: EffectReplacement::Complete,
            adapter_id: Some("ldconfig/v2".to_string()),
            adapter_digest: Some(crate::hash::sha256_prefixed(b"ldconfig/v2")),
            command: Some("ldconfig".to_string()),
            args: Vec::new(),
            path: None,
            reason_code: Some("helper-complete-ldconfig".to_string()),
            extra: BTreeMap::new(),
        };
        let intents = policy_intents_from_effects("scriptlet:0:post-install", &[effect]);

        assert!(intents.is_empty());
    }

    #[test]
    fn selinux_file_context_effects_project_generic_policy_intent() {
        let mut effect = selinux_effect("selinux-file-context", "semanage");
        effect.args = vec![
            "fcontext".to_string(),
            "-a".to_string(),
            "-t".to_string(),
            "demo_exec_t".to_string(),
            "/usr/bin/demo".to_string(),
        ];
        effect.path = Some("/usr/bin/demo".to_string());
        effect.extra.insert(
            "selinux_operation".to_string(),
            toml::Value::String("file-context-add".to_string()),
        );
        effect
            .extra
            .insert("action".to_string(), toml::Value::String("add".to_string()));
        effect.extra.insert(
            "selinux_type".to_string(),
            toml::Value::String("demo_exec_t".to_string()),
        );
        effect.extra.insert(
            "expression".to_string(),
            toml::Value::String("/usr/bin/demo".to_string()),
        );
        effect.extra.insert(
            "payload_root".to_string(),
            toml::Value::String("/usr".to_string()),
        );
        effect
            .extra
            .insert("payload_backed".to_string(), toml::Value::Boolean(true));
        effect.extra.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String("/usr/bin/demo".to_string())]),
        );

        let intents = policy_intents_from_effects("scriptlet:0:post-install", &[effect]);

        assert_eq!(intents.len(), 1);
        let intent = &intents[0];
        assert_eq!(intent.provider.as_str(), "selinux");
        assert_eq!(intent.operation, "file-context-add");
        assert_eq!(intent.scope.kind, "file-context");
        assert_eq!(intent.scope.name.as_deref(), Some("/usr/bin/demo"));
        assert_eq!(intent.scope.paths, vec!["/usr/bin/demo"]);
        assert_eq!(
            intent
                .desired_state
                .get("selinux_type")
                .and_then(toml::Value::as_str),
            Some("demo_exec_t")
        );
        assert_eq!(intent.payload_evidence.payload_backed, true);
        assert_eq!(intent.payload_evidence.paths, vec!["/usr/bin/demo"]);
        assert_eq!(intent.fallback.as_str(), "dormant");
        assert_eq!(intent.reconciliation.state.as_str(), "pending");
    }

    #[test]
    fn selinux_boolean_effects_project_generic_policy_intent() {
        let mut effect = selinux_effect("selinux-boolean", "setsebool");
        effect.args = vec![
            "-P".to_string(),
            "demo_can_network".to_string(),
            "on".to_string(),
        ];
        effect.path = Some("demo_can_network".to_string());
        effect.extra.insert(
            "selinux_operation".to_string(),
            toml::Value::String("boolean-set".to_string()),
        );
        effect.extra.insert(
            "boolean".to_string(),
            toml::Value::String("demo_can_network".to_string()),
        );
        effect
            .extra
            .insert("value".to_string(), toml::Value::String("on".to_string()));
        effect
            .extra
            .insert("persistent".to_string(), toml::Value::Boolean(true));

        let intents = policy_intents_from_effects("scriptlet:0:post-install", &[effect]);

        assert_eq!(intents.len(), 1);
        let intent = &intents[0];
        assert_eq!(intent.provider.as_str(), "selinux");
        assert_eq!(intent.operation, "boolean-set");
        assert_eq!(intent.scope.kind, "boolean");
        assert_eq!(intent.scope.name.as_deref(), Some("demo_can_network"));
        assert!(intent.scope.paths.is_empty());
        assert_eq!(
            intent
                .desired_state
                .get("value")
                .and_then(toml::Value::as_str),
            Some("on")
        );
        assert_eq!(intent.payload_evidence.payload_backed, false);
        assert_eq!(intent.fallback.as_str(), "dormant");
        assert_eq!(intent.reconciliation.state.as_str(), "pending");
    }

    #[test]
    fn selinux_policy_module_effects_project_generic_policy_intent() {
        let mut effect = selinux_effect("selinux-policy-module", "semodule");
        effect.args = vec![
            "-i".to_string(),
            "/usr/share/selinux/packages/demo.pp".to_string(),
        ];
        effect.path = Some("/usr/share/selinux/packages/demo.pp".to_string());
        effect.extra.insert(
            "selinux_operation".to_string(),
            toml::Value::String("module-install".to_string()),
        );
        effect.extra.insert(
            "module_path".to_string(),
            toml::Value::String("/usr/share/selinux/packages/demo.pp".to_string()),
        );
        effect.extra.insert(
            "module_format".to_string(),
            toml::Value::String("pp".to_string()),
        );
        effect
            .extra
            .insert("payload_backed".to_string(), toml::Value::Boolean(true));
        effect.extra.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String(
                "/usr/share/selinux/packages/demo.pp".to_string(),
            )]),
        );

        let intents = policy_intents_from_effects("scriptlet:0:post-install", &[effect]);

        assert_eq!(intents.len(), 1);
        let intent = &intents[0];
        assert_eq!(intent.provider.as_str(), "selinux");
        assert_eq!(intent.operation, "module-install");
        assert_eq!(intent.scope.kind, "policy-module");
        assert_eq!(
            intent.scope.name.as_deref(),
            Some("/usr/share/selinux/packages/demo.pp")
        );
        assert_eq!(
            intent.scope.paths,
            vec!["/usr/share/selinux/packages/demo.pp"]
        );
        assert_eq!(
            intent
                .desired_state
                .get("module_format")
                .and_then(toml::Value::as_str),
            Some("pp")
        );
        assert_eq!(intent.payload_evidence.payload_backed, true);
        assert_eq!(
            intent.payload_evidence.paths,
            vec!["/usr/share/selinux/packages/demo.pp"]
        );
        assert_eq!(intent.fallback.as_str(), "dormant");
        assert_eq!(intent.reconciliation.state.as_str(), "pending");
    }
}
