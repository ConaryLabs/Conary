// conary-core/src/model/parser/tests.rs

//! Focused parser and model-policy tests.

use super::*;
use crate::repository::resolution_policy::SelectionMode;

fn minimal_model_toml() -> &'static str {
    r#"
[model]
version = 1
"#
}

fn model_toml_with_system(system_body: &str) -> String {
    format!(
        r#"
[model]
version = 1

[system]
{}
"#,
        system_body
    )
}

#[test]
fn test_empty_model() {
    let model = SystemModel::new();
    assert_eq!(model.config.version, MODEL_VERSION);
    assert!(model.config.install.is_empty());
}

#[test]
fn test_parse_model_string() {
    let toml = r#"
[model]
version = 1
search = ["fedora@f41:stable"]
install = ["nginx", "redis"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"
"#;
    let model = parse_model_string(toml).unwrap();
    assert_eq!(model.config.install.len(), 2);
    assert!(model.is_excluded("sendmail"));
    assert!(!model.is_excluded("nginx"));
    assert_eq!(model.get_pin("openssl"), Some("3.0.*"));
    assert_eq!(model.get_pin("nginx"), None);
}

#[test]
fn test_conflict_detection() {
    let toml = r#"
[model]
version = 1
install = ["nginx"]
exclude = ["nginx"]
"#;
    let result = parse_model_string(toml);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ModelError::ConflictingSpecs(_)
    ));
}

#[test]
fn test_derived_package() {
    let toml = r#"
[model]
version = 1
install = ["nginx-custom"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"
patches = ["custom.patch"]

[derive.override_files]
"/etc/nginx/nginx.conf" = "files/nginx.conf"
"#;
    let model = parse_model_string(toml).unwrap();
    assert_eq!(model.derive.len(), 1);
    assert_eq!(model.derive[0].name, "nginx-custom");
    assert_eq!(model.derive[0].from, "nginx");
    assert_eq!(model.derive[0].patches.len(), 1);
}

#[test]
fn test_to_toml_roundtrip() {
    let mut model = SystemModel::new();
    model.config.search = vec!["fedora@f41:stable".to_string()];
    model.config.install = vec!["nginx".to_string(), "redis".to_string()];
    model.pin.insert("openssl".to_string(), "3.0.*".to_string());

    let toml = model.to_toml().unwrap();
    assert!(toml.contains("[system]"));
    assert!(toml.contains("profile = \"balanced/latest-anywhere\""));
    let parsed = parse_model_string(&toml).unwrap();

    assert_eq!(parsed.config.install, model.config.install);
    assert_eq!(parsed.pin, model.pin);
}

#[test]
fn test_parse_include_section() {
    let toml = r#"
[model]
version = 1
install = ["custom-app"]

[include]
models = ["group-base-server@myrepo:stable", "group-security@corp:production"]
on_conflict = "local"
trusted_keys = ["d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"]
"#;
    let model = parse_model_string(toml).unwrap();
    assert_eq!(model.include.models.len(), 2);
    assert_eq!(model.include.models[0], "group-base-server@myrepo:stable");
    assert_eq!(model.include.on_conflict, ConflictStrategy::Local);
    assert!(model.include.trusted_keys.is_empty());
}

#[test]
fn test_parse_include_error_strategy() {
    let toml = r#"
[model]
version = 1
install = ["custom-app"]

[include]
models = ["group-base@myrepo:stable"]
on_conflict = "error"
trusted_keys = ["d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"]
"#;
    let model = parse_model_string(toml).unwrap();
    assert_eq!(model.include.on_conflict, ConflictStrategy::Error);
}

#[test]
fn test_parse_remote_include_requires_trusted_key() {
    let toml = r#"
[model]
version = 1

[include]
models = ["group-base@myrepo:stable"]
"#;

    let error = parse_model_string(toml).unwrap_err().to_string();
    assert!(error.contains("require at least one trusted"), "{error}");
}

#[test]
fn test_parse_remote_include_rejects_malformed_trusted_key() {
    let toml = r#"
[model]
version = 1

[include]
models = ["group-base@myrepo:stable"]
trusted_keys = ["not-an-ed25519-key"]
"#;

    let error = parse_model_string(toml).unwrap_err().to_string();
    assert!(error.contains("Invalid trusted Ed25519"), "{error}");
}

#[test]
fn test_parse_remote_include_rejects_duplicate_trusted_key() {
    let toml = r#"
[model]
version = 1

[include]
models = ["group-base@myrepo:stable"]
trusted_keys = [
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
]
"#;

    let error = parse_model_string(toml).unwrap_err().to_string();
    assert!(error.contains("Duplicate trusted Ed25519"), "{error}");
}

#[test]
fn test_parse_include_rejects_removed_signature_bypass() {
    let toml = r#"
[model]
version = 1

[include]
models = ["group-base@myrepo:stable"]
require_signatures = false
"#;
    let error = parse_model_string(toml).unwrap_err().to_string();
    assert!(error.contains("require_signatures"), "{error}");
}

#[test]
fn test_has_includes() {
    let mut model = SystemModel::new();
    assert!(!model.has_includes());

    model
        .include
        .models
        .push("group-base@repo:stable".to_string());
    assert!(model.has_includes());
}

#[test]
fn test_automation_defaults() {
    let model = SystemModel::new();
    // Default mode is Suggest (safest)
    assert_eq!(model.automation.mode, AutomationMode::Suggest);
    // AI assist is disabled by default
    assert!(!model.automation.ai_assist.enabled);
    // Major upgrades require approval by default
    assert!(model.automation.major_upgrades.require_approval);
}

#[test]
fn test_parse_automation_config() {
    let toml = r#"
[model]
version = 1
install = ["nginx"]

[automation]
mode = "suggest"
check_interval = "1h"
notify = ["admin@example.com"]

[automation.security]
mode = "auto"
within = "12h"
severities = ["critical", "high", "medium"]
reboot = "never"

[automation.orphans]
mode = "suggest"
after = "14d"
keep = ["libfoo"]

[automation.updates]
mode = "disabled"
frequency = "daily"
window = "02:00-04:00"
exclude = ["kernel"]

[automation.major_upgrades]
require_approval = true
allow_auto = ["nodejs"]

[automation.repair]
integrity_check = true
check_interval = "12h"
auto_restore = true

[[automation.repair.rollback_triggers]]
name = "nginx-health"
command = "curl -f localhost/health"
timeout = "10s"
failure_window = "3m"
auto_rollback = true

[automation.ai_assist]
enabled = true
mode = "assisted"
intent_resolution = true
scriptlet_translation = false
natural_language = true
confidence_threshold = 0.85
require_human_approval = ["security", "removal"]
"#;
    let model = parse_model_string(toml).unwrap();

    // Global settings
    assert_eq!(model.automation.mode, AutomationMode::Suggest);
    assert_eq!(model.automation.check_interval, "1h");
    assert_eq!(model.automation.notify, vec!["admin@example.com"]);

    // Security
    assert_eq!(model.automation.security.mode, Some(AutomationMode::Auto));
    assert_eq!(model.automation.security.within, "12h");
    assert_eq!(model.automation.security.severities.len(), 3);
    assert_eq!(model.automation.security.reboot, "never");

    // Orphans
    assert_eq!(model.automation.orphans.mode, Some(AutomationMode::Suggest));
    assert_eq!(model.automation.orphans.after, "14d");
    assert_eq!(model.automation.orphans.keep, vec!["libfoo"]);

    // Updates
    assert_eq!(
        model.automation.updates.mode,
        Some(AutomationMode::Disabled)
    );
    assert_eq!(model.automation.updates.frequency, "daily");
    assert_eq!(
        model.automation.updates.window,
        Some("02:00-04:00".to_string())
    );
    assert_eq!(model.automation.updates.exclude, vec!["kernel"]);

    // Major upgrades
    assert!(model.automation.major_upgrades.require_approval);
    assert_eq!(model.automation.major_upgrades.allow_auto, vec!["nodejs"]);

    // Repair
    assert!(model.automation.repair.integrity_check);
    assert!(model.automation.repair.auto_restore);
    assert_eq!(model.automation.repair.rollback_triggers.len(), 1);
    let trigger = &model.automation.repair.rollback_triggers[0];
    assert_eq!(trigger.name, "nginx-health");
    assert!(trigger.auto_rollback);

    // AI assist
    assert!(model.automation.ai_assist.enabled);
    assert_eq!(model.automation.ai_assist.mode, AiAssistMode::Assisted);
    assert!(model.automation.ai_assist.intent_resolution);
    assert!(!model.automation.ai_assist.scriptlet_translation);
    assert!(model.automation.ai_assist.natural_language);
    assert!((model.automation.ai_assist.confidence_threshold - 0.85).abs() < 0.001);
}

#[test]
fn test_effective_automation_mode() {
    let toml = r#"
[model]
version = 1
install = ["nginx"]

[automation]
mode = "suggest"

[automation.security]
mode = "auto"
"#;
    let model = parse_model_string(toml).unwrap();

    // Security has explicit override
    assert_eq!(
        model.effective_mode(AutomationCategory::Security),
        AutomationMode::Auto
    );
    // Orphans inherits global
    assert_eq!(
        model.effective_mode(AutomationCategory::Orphans),
        AutomationMode::Suggest
    );
    // Updates inherits global
    assert_eq!(
        model.effective_mode(AutomationCategory::Updates),
        AutomationMode::Suggest
    );
}

#[test]
fn test_ai_assist_feature_checks() {
    let mut model = SystemModel::new();

    // AI assist disabled by default
    assert!(!model.ai_assist_enabled(AiFeature::IntentResolution));
    assert!(!model.ai_assist_enabled(AiFeature::NaturalLanguage));

    // Enable AI assist
    model.automation.ai_assist.enabled = true;
    model.automation.ai_assist.intent_resolution = true;

    // Now intent resolution is enabled
    assert!(model.ai_assist_enabled(AiFeature::IntentResolution));
    // But scriptlet translation is still disabled
    assert!(!model.ai_assist_enabled(AiFeature::ScriptletTranslation));
}

#[test]
fn test_automation_mode_serialization() {
    let mut model = SystemModel::new();
    model.automation.mode = AutomationMode::Auto;
    model.automation.security.mode = Some(AutomationMode::Suggest);

    let toml = model.to_toml().unwrap();
    let parsed = parse_model_string(&toml).unwrap();

    assert_eq!(parsed.automation.mode, AutomationMode::Auto);
    assert_eq!(
        parsed.automation.security.mode,
        Some(AutomationMode::Suggest)
    );
}

#[test]
fn removed_flat_source_pin_fields_are_rejected() {
    let input = r#"
[model]
version = 1

[system]
distro = "ubuntu-noble"
mixing = "guarded"
"#;
    let error = toml::from_str::<SystemModel>(input)
        .expect_err("removed flat source-pin fields must fail")
        .to_string();
    assert!(error.contains("unknown field `distro`"), "{error}");
}

#[test]
fn test_parse_source_policy_profile_and_pin() {
    let input = r#"
[model]
version = 1

[system]
profile = "balanced/latest-anywhere"
allowed_distros = ["fedora-44", "arch"]

[system.pin]
distro = "arch"
strength = "hard"
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(
        model.system.profile.as_deref(),
        Some("balanced/latest-anywhere")
    );
    assert_eq!(
        model.system.allowed_distros,
        vec!["fedora-44".to_string(), "arch".to_string()]
    );
    let pin = model.system.effective_pin().expect("expected source pin");
    assert_eq!(pin.distro, "arch");
    assert_eq!(pin.strength.as_deref(), Some("hard"));
}

#[test]
fn test_parse_package_overrides() {
    let input = r#"
[model]
version = 1

[overrides]
mesa = { from = "fedora-41" }
nvidia-driver = { from = "rpmfusion-41", reason = "closed source drivers" }
kernel = { from = "fedora-44", scope = "family", reason = "prefer fedora kernels" }
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.overrides.len(), 3);
    assert_eq!(model.overrides["mesa"].from, "fedora-41");
    assert_eq!(
        model.overrides["nvidia-driver"].reason.as_deref(),
        Some("closed source drivers")
    );
    assert_eq!(model.overrides["kernel"].scope.as_deref(), Some("family"));
    assert_eq!(
        model.overrides["kernel"].reason.as_deref(),
        Some("prefer fedora kernels")
    );
}

#[test]
fn test_default_source_policy_has_no_pin() {
    let input = r#"
[model]
version = 1
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(
        model.system.profile.as_deref(),
        Some("balanced/latest-anywhere")
    );
    assert!(model.system.allowed_distros.is_empty());
    assert!(model.system.effective_pin().is_none());
    assert!(model.overrides.is_empty());
}

#[test]
fn test_convergence_intent_defaults_to_cas_backed() {
    let input = r#"
[model]
version = 1
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.system.convergence, ConvergenceIntent::CasBacked);
}

#[test]
fn test_parse_convergence_cas_backed() {
    let input = r#"
[model]
version = 1

[system]
convergence = "cas-backed"
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.system.convergence, ConvergenceIntent::CasBacked);
}

#[test]
fn test_parse_convergence_full_ownership() {
    let input = r#"
[model]
version = 1

[system]
convergence = "full-ownership"
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.system.convergence, ConvergenceIntent::FullOwnership);
}

#[test]
fn test_parse_convergence_with_pin_and_profile() {
    let input = r#"
[model]
version = 1

[system]
profile = "balanced/latest-anywhere"
convergence = "full-ownership"
allowed_distros = ["arch", "fedora-44"]

[system.pin]
distro = "arch"
strength = "hard"
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.system.convergence, ConvergenceIntent::FullOwnership);
    assert_eq!(
        model.system.profile.as_deref(),
        Some("balanced/latest-anywhere")
    );
    let pin = model.system.effective_pin().unwrap();
    assert_eq!(pin.distro, "arch");
    assert_eq!(pin.strength.as_deref(), Some("hard"));
}

#[test]
fn test_convergence_intent_roundtrip_via_toml() {
    let mut model = SystemModel::new();
    model.system.convergence = ConvergenceIntent::CasBacked;
    model.system.pin = Some(SourcePinConfig {
        distro: "arch".to_string(),
        strength: Some("hard".to_string()),
    });

    let toml = model.to_toml().unwrap();
    let parsed = parse_model_string(&toml).unwrap();

    assert_eq!(parsed.system.convergence, ConvergenceIntent::CasBacked);
    let pin = parsed.system.effective_pin().unwrap();
    assert_eq!(pin.distro, "arch");
    assert_eq!(pin.strength.as_deref(), Some("hard"));
}

#[test]
fn test_convergence_intent_target_install_source_mapping() {
    assert_eq!(
        ConvergenceIntent::TrackOnly.target_install_source(),
        "adopted-track"
    );
    assert_eq!(
        ConvergenceIntent::CasBacked.target_install_source(),
        "adopted-full"
    );
    assert_eq!(
        ConvergenceIntent::FullOwnership.target_install_source(),
        "taken"
    );
}

#[test]
fn test_convergence_intent_display_names() {
    assert_eq!(ConvergenceIntent::TrackOnly.display_name(), "track-only");
    assert_eq!(ConvergenceIntent::CasBacked.display_name(), "cas-backed");
    assert_eq!(
        ConvergenceIntent::FullOwnership.display_name(),
        "full-ownership"
    );
}

#[test]
fn test_override_scope_exact_wins_over_family_and_class() {
    let input = r#"
[model]
version = 1

[overrides]
kernel = { from = "fedora-44", scope = "family", reason = "prefer fedora kernels" }
kernel-core = { from = "arch", reason = "exact match override" }
libs = { from = "ubuntu-noble", scope = "class", reason = "prefer ubuntu libs" }
"#;
    let model: SystemModel = toml::from_str(input).unwrap();

    // Exact match wins even though family and class are available
    let result = model.resolve_override("kernel-core", Some("kernel"), Some("libs"));
    assert!(result.is_some());
    let (key, config) = result.unwrap();
    assert_eq!(key, "kernel-core");
    assert_eq!(config.from, "arch");
}

#[test]
fn test_override_scope_family_wins_over_class() {
    let input = r#"
[model]
version = 1

[overrides]
kernel = { from = "fedora-44", scope = "family", reason = "prefer fedora kernels" }
libs = { from = "ubuntu-noble", scope = "class", reason = "prefer ubuntu libs" }
"#;
    let model: SystemModel = toml::from_str(input).unwrap();

    // No exact match for "kernel-headers", family "kernel" should win over class "libs"
    let result = model.resolve_override("kernel-headers", Some("kernel"), Some("libs"));
    assert!(result.is_some());
    let (key, config) = result.unwrap();
    assert_eq!(key, "kernel");
    assert_eq!(config.from, "fedora-44");
}

#[test]
fn test_override_scope_class_fallback() {
    let input = r#"
[model]
version = 1

[overrides]
libs = { from = "ubuntu-noble", scope = "class", reason = "prefer ubuntu libs" }
"#;
    let model: SystemModel = toml::from_str(input).unwrap();

    // No exact or family match, class should match
    let result = model.resolve_override("libssl", None, Some("libs"));
    assert!(result.is_some());
    let (key, config) = result.unwrap();
    assert_eq!(key, "libs");
    assert_eq!(config.from, "ubuntu-noble");
}

#[test]
fn test_override_scope_no_match_returns_none() {
    let input = r#"
[model]
version = 1

[overrides]
mesa = { from = "fedora-41" }
"#;
    let model: SystemModel = toml::from_str(input).unwrap();

    let result = model.resolve_override("vim", None, None);
    assert!(result.is_none());
}

#[test]
fn test_source_policy_default_is_unconfigured() {
    let config = SystemConfig::default();
    assert!(!config.is_source_policy_configured());
}

#[test]
fn source_policy_default_profile_maps_to_latest_selection_mode() {
    let config = SystemConfig::default();
    assert_eq!(
        config.effective_selection_mode(),
        Some(SelectionMode::Latest)
    );
}

#[test]
fn source_policy_explicit_selection_mode_overrides_profile_mapping() {
    let config = SystemConfig {
        profile: Some("balanced/latest-anywhere".to_string()),
        selection_mode: Some("policy".to_string()),
        ..Default::default()
    };
    assert_eq!(
        config.effective_selection_mode(),
        Some(SelectionMode::Policy)
    );
}

#[test]
fn source_policy_non_default_profile_counts_as_configuration() {
    let config = SystemConfig {
        profile: Some("conservative/policy-first".to_string()),
        ..Default::default()
    };
    assert!(config.is_source_policy_configured());
}

#[test]
fn source_policy_implicit_default_profile_is_not_counted_as_explicit_configuration() {
    let model = parse_model_string(minimal_model_toml()).unwrap();
    assert!(!model.system.is_source_policy_configured());
}

#[test]
fn source_policy_explicit_default_profile_counts_as_configuration() {
    let model = parse_model_string(&model_toml_with_system(
        "profile = \"balanced/latest-anywhere\"",
    ))
    .unwrap();
    assert!(model.system.is_source_policy_configured());
}

#[test]
fn source_policy_unknown_profile_is_rejected() {
    let model = parse_model_string(&model_toml_with_system("profile = \"mystery/not-real\""));
    assert!(model.is_err());
}

#[test]
fn test_source_policy_with_distro_pin_is_configured() {
    let config = SystemConfig {
        pin: Some(SourcePinConfig {
            distro: "arch".to_string(),
            strength: Some("hard".to_string()),
        }),
        ..SystemConfig::default()
    };
    assert!(config.is_source_policy_configured());
}

#[test]
fn test_source_policy_with_track_only_convergence_is_configured() {
    let config = SystemConfig {
        convergence: ConvergenceIntent::TrackOnly,
        ..SystemConfig::default()
    };
    assert!(config.is_source_policy_configured());
}

#[test]
fn test_source_policy_with_allowed_distros_is_configured() {
    let config = SystemConfig {
        allowed_distros: vec!["fedora-44".to_string()],
        ..SystemConfig::default()
    };
    assert!(config.is_source_policy_configured());
}
