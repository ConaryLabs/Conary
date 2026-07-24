// conary-core/src/ccs/manifest/tests.rs

use super::*;

#[test]
fn test_minimal_manifest() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "A test package"
"#;
    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.package.name, "test");
    assert_eq!(manifest.package.version, "1.0.0");
}

#[test]
fn test_full_manifest() {
    let toml = r#"
[package]
name = "myapp"
version = "1.2.3"
description = "My application"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provides]
capabilities = ["cli-tool", "json-parsing"]

[requires]
capabilities = [
    "glibc",
    { name = "tls", version = ">=1.2" },
]
packages = [
    { name = "openssl", version = ">=3.0" },
]

[components]
default = ["runtime", "lib"]

[components.files]
"/usr/bin/helper" = "lib"

[[hooks.users]]
name = "myapp"
system = true
home = "/var/lib/myapp"

[[hooks.directories]]
path = "/var/lib/myapp"
mode = "0750"
owner = "myapp"

[[hooks.systemd]]
unit = "myapp.service"
enable = false

[config]
files = ["/etc/myapp/config.toml"]
"#;
    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.package.name, "myapp");
    assert_eq!(manifest.provides.capabilities.len(), 2);
    assert_eq!(manifest.requires.capabilities.len(), 2);
    assert_eq!(manifest.hooks.users.len(), 1);
    assert_eq!(manifest.hooks.users[0].name, "myapp");
    assert!(manifest.hooks.users[0].system);
}

#[test]
fn test_generate_minimal() {
    let manifest = CcsManifest::new_minimal("test", "0.1.0");
    let toml = manifest.to_toml().unwrap();
    assert!(toml.contains("name = \"test\""));
    assert!(toml.contains("version = \"0.1.0\""));
}

#[test]
fn parses_v2_authoring_identity_fields_without_guessing_release() {
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "hello"
version = "0.1.0"
release = "1"
kind = "package"
description = "hello"
"#,
    )
    .unwrap();

    assert_eq!(manifest.package.release.as_deref(), Some("1"));
    assert_eq!(
        manifest.package.kind,
        Some(crate::ccs::v2::PackageKindTagV2::Package)
    );

    let legacy = CcsManifest::parse(
        r#"
[package]
name = "legacy"
version = "1.0.0-1"
description = "legacy"
"#,
    )
    .unwrap();

    assert_eq!(legacy.package.release, None);
    assert_eq!(legacy.package.kind, None);
}

#[test]
fn manifest_provenance_serializes_m1a_origin_and_hardening() {
    let provenance = ManifestProvenance {
        origin_class: Some("native-built".to_string()),
        hardening_level: Some("sandboxed".to_string()),
        hermetic_evidence: None,
        ..Default::default()
    };
    let toml = toml::to_string(&provenance).unwrap();
    assert!(toml.contains("origin_class"));
    assert!(toml.contains("hardening_level"));
}

#[test]
fn test_redirects_section_parsing() {
    let toml = r#"
[package]
name = "nginx"
version = "1.24.0"
description = "High-performance HTTP server"

[[redirects.renames]]
old_name = "nginx-mainline"
message = "Consolidated with mainline"

[[redirects.obsoletes]]
package = "nginx-legacy"
version = "<1.20"
message = "Legacy branch no longer supported"
"#;
    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.redirects.renames.len(), 1);
    assert_eq!(manifest.redirects.renames[0].old_name, "nginx-mainline");

    assert_eq!(manifest.redirects.obsoletes.len(), 1);
    assert_eq!(manifest.redirects.obsoletes[0].package, "nginx-legacy");
    assert_eq!(
        manifest.redirects.obsoletes[0].version,
        Some("<1.20".to_string())
    );
}

#[test]
fn test_redirects_merge_split() {
    let toml = r#"
[package]
name = "foo-combined"
version = "2.0.0"
description = "Combined package"

[[redirects.merges]]
package = "foo-core"
message = "Merged foo-core into main package"

[[redirects.merges]]
package = "foo-extras"
message = "Merged foo-extras into main package"

[[redirects.splits]]
from_package = "monolithic-foo"
component = "core"
"#;
    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.redirects.merges.len(), 2);
    assert_eq!(manifest.redirects.splits.len(), 1);
    assert_eq!(manifest.redirects.splits[0].from_package, "monolithic-foo");
    assert_eq!(
        manifest.redirects.splits[0].component,
        Some("core".to_string())
    );
}

#[test]
fn test_redirects_is_empty() {
    let redirects = Redirects::default();
    assert!(redirects.is_empty());
    assert_eq!(redirects.len(), 0);

    let toml = r#"
[package]
name = "simple"
version = "1.0.0"
description = "No redirects"
"#;
    let manifest = CcsManifest::parse(toml).unwrap();
    assert!(manifest.redirects.is_empty());
}

#[test]
fn test_manifest_rejects_non_system_user_hooks() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.users]]
name = "daemon"
system = false
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("system user"));
}

#[test]
fn test_manifest_rejects_unsafe_tmpfiles_entries() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.tmpfiles]]
entry_type = "L"
path = "../etc/shadow"
mode = "0755"
owner = "root"
group = "root"
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(
        err.to_string().contains("tmpfiles")
            || err.to_string().contains("path")
            || err.to_string().contains("entry")
    );
}

#[test]
fn test_manifest_rejects_denied_sysctl_keys() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.sysctl]]
key = "kernel.modules_disabled"
value = "0"
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("sysctl"));
}

#[test]
fn test_manifest_rejects_unsafe_systemd_unit_names() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.systemd]]
unit = "../evil.service"
enable = true
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("systemd"));
}

#[test]
fn test_manifest_accepts_supported_scriptlet_capabilities() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[scriptlets.capabilities]]
name = "systemd-service-registration"
paths = ["/etc/systemd/system"]

[[scriptlets.capabilities]]
name = "tmpfiles-registration"
paths = ["/usr/lib/tmpfiles.d", "/etc/tmpfiles.d"]
"#;

    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.scriptlets.capabilities.len(), 2);
    assert!(manifest.scriptlets.has_capability_declarations());
}

#[test]
fn test_manifest_rejects_unknown_scriptlet_capability() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[scriptlets.capabilities]]
name = "pam-live-edit"
paths = ["/etc/pam.d"]
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(
            err.to_string().contains(
                "unknown scriptlet capability 'pam-live-edit'; declare a supported capability or run in a VM until enforcement exists"
            ),
            "unexpected error: {err}"
        );
}

#[test]
fn test_manifest_accepts_supported_file_capabilities() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[file_capabilities]]
path = "/usr/bin/demo"
capabilities = ["cap_net_bind_service"]
permitted = true
effective = true
inheritable = false
"#;

    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.file_capabilities.len(), 1);
    assert_eq!(manifest.file_capabilities[0].path, "/usr/bin/demo");
    assert_eq!(
        manifest.file_capabilities[0].capabilities,
        vec!["cap_net_bind_service".to_string()]
    );
    assert!(manifest.file_capabilities[0].permitted);
    assert!(manifest.file_capabilities[0].effective);
    assert!(!manifest.file_capabilities[0].inheritable);

    let encoded = manifest.to_toml().expect("serialize manifest");
    assert!(encoded.contains("[[file_capabilities]]"));
    let decoded = CcsManifest::parse(&encoded).expect("parse serialized manifest");
    assert_eq!(decoded.file_capabilities[0].path, "/usr/bin/demo");
}

#[test]
fn test_manifest_rejects_unsafe_file_capabilities() {
    for (toml, expected) in [
        (
            r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[file_capabilities]]
path = "usr/bin/demo"
capabilities = ["cap_net_bind_service"]
"#,
            "relative path not allowed in file_capabilities",
        ),
        (
            r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[file_capabilities]]
path = "/usr/bin/demo"
capabilities = ["cap_not_real"]
"#,
            "unknown Linux file capability",
        ),
        (
            r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[file_capabilities]]
path = "/usr/bin/demo"
capabilities = ["cap_net_bind_service"]
permitted = false
effective = true
"#,
            "effective file capability requires permitted",
        ),
        (
            r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[file_capabilities]]
path = "/usr/bin/demo"
capabilities = ["cap_net_bind_service"]
inheritable = true
"#,
            "inheritable file capabilities are not supported yet",
        ),
    ] {
        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "expected {expected:?}, got {err}"
        );
    }
}

fn manifest_with_legacy_scriptlet_bundle(body: &str, body_sha256: &str) -> String {
    format!(
        r#"
[package]
name = "nginx"
version = "1.28.0"
description = "nginx converted from RPM"

[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 2
source_format = "rpm"
source_family = "fedora-rhel"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "nginx"
source_version = "1.28.0-1.fc44"
source_checksum = "sha256:3333333333333333333333333333333333333333333333333333333333333333"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.8.0"
conversion_policy = "safe-or-legacy"
target_compatibility = "source-native"
allowed_targets = ["rpm/fedora/44/x86_64"]
foreign_replay_policy = "deny"
publication_policy = "public-if-no-blocked"
publication_status = "private-review"
scriptlet_fidelity = "legacy-replay"

[legacy_scriptlets.decision_counts]
legacy = 1

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
lifecycle_paths = ["install:first"]
interpreter = "/bin/sh"
interpreter_args = ["-e"]
body_sha256 = "{body_sha256}"
body = "{body}"
native_invocation = {{ args = ["1"], environment = ["RPM_INSTALL_PREFIX=/"], stdin = "none", chroot = "install-root" }}
transaction_order = {{ position = "after-payload", after = ["payload"] }}
timeout_ms = 30000
decision = "legacy"
reason_code = "protected-replay-required"

[[legacy_scriptlets.entries.effects]]
kind = "ldconfig"
source = "shell-ast"
confidence = "declared"
replacement = "complete"
"#
    )
}

#[test]
fn manifest_toml_round_trips_legacy_scriptlet_bundle() {
    let body = "ldconfig";
    let body_sha256 = crate::hash::sha256_prefixed(body.as_bytes());
    let toml = manifest_with_legacy_scriptlet_bundle(body, &body_sha256);

    let manifest = CcsManifest::parse(&toml).expect("parse manifest");
    let bundle = manifest
        .legacy_scriptlets
        .as_ref()
        .expect("legacy scriptlet bundle");

    assert_eq!(bundle.source_package, "nginx");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].id, "rpm:%post");

    let encoded = manifest.to_toml().expect("serialize manifest");
    assert!(encoded.contains("[legacy_scriptlets]"));
    let decoded = CcsManifest::parse(&encoded).expect("parse serialized manifest");
    assert_eq!(
        decoded
            .legacy_scriptlets
            .as_ref()
            .expect("legacy bundle")
            .entries[0]
            .effects[0]
            .kind,
        "ldconfig"
    );
}

#[test]
fn manifest_toml_round_trips_generic_security_policy_intent() {
    let body_sha256 = crate::hash::sha256_prefixed(b"restorecon /usr/share/demo\n");
    let toml = format!(
        r#"
[package]
name = "demo-policy"
version = "1.0.0"
description = "policy fixture"

[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 2
source_format = "rpm"
source_family = "rpm"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "demo-policy"
source_version = "1.0.0-1.fc44"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.10.1"
conversion_policy = "passive-scriptlet-bundle-goal4"
target_compatibility = "conary-portable"
foreign_replay_policy = "deny"
publication_policy = "public-if-no-blocked"
publication_status = "public"
scriptlet_fidelity = "fully-replaced"

[legacy_scriptlets.decision_counts]
replaced = 1

[[legacy_scriptlets.security_policy_intents]]
schema = "conary.security-policy-intent.v1"
id = "rpm:%post:selinux-label-refresh"
provider = "selinux"
operation = "label-refresh"
fallback = "dormant"

[legacy_scriptlets.security_policy_intents.source]
source_format = "rpm"
source_distro = "fedora"
entry_id = "rpm:%post"
command = "restorecon"
argv = ["-R", "/usr/share/demo"]
adapter_id = "selinux-policy/v1"

[legacy_scriptlets.security_policy_intents.scope]
kind = "path"
paths = ["/usr/share/demo"]

[legacy_scriptlets.security_policy_intents.desired_state]
recursive = true

[legacy_scriptlets.security_policy_intents.requirements]
required_on_active_provider = false
tools = ["restorecon"]

[legacy_scriptlets.security_policy_intents.payload_evidence]
payload_backed = true
paths = ["/usr/share/demo"]

[legacy_scriptlets.security_policy_intents.reconciliation]
state = "pending"

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
lifecycle_paths = ["post-install"]
interpreter = "/bin/sh"
body_sha256 = "{body_sha256}"
body = "restorecon /usr/share/demo\n"
native_invocation = {{ args = [], environment = [] }}
transaction_order = {{ position = "after-payload" }}
timeout_ms = 30000
decision = "replaced"
reason_code = "helper-complete-selinux-policy"

[[legacy_scriptlets.entries.security_policy_intents]]
schema = "conary.security-policy-intent.v1"
id = "rpm:%post:selinux-label-refresh"
provider = "selinux"
operation = "label-refresh"
fallback = "dormant"

[legacy_scriptlets.entries.security_policy_intents.source]
source_format = "rpm"
source_distro = "fedora"
entry_id = "rpm:%post"
command = "restorecon"
argv = ["-R", "/usr/share/demo"]
adapter_id = "selinux-policy/v1"

[legacy_scriptlets.entries.security_policy_intents.scope]
kind = "path"
paths = ["/usr/share/demo"]

[legacy_scriptlets.entries.security_policy_intents.desired_state]
recursive = true

[legacy_scriptlets.entries.security_policy_intents.requirements]
required_on_active_provider = false
tools = ["restorecon"]

[legacy_scriptlets.entries.security_policy_intents.payload_evidence]
payload_backed = true
paths = ["/usr/share/demo"]

[legacy_scriptlets.entries.security_policy_intents.reconciliation]
state = "pending"
"#
    );

    let manifest = CcsManifest::parse(&toml).expect("parse manifest");
    let bundle = manifest.legacy_scriptlets.as_ref().unwrap();

    assert_eq!(bundle.security_policy_intents.len(), 1);
    assert_eq!(
        bundle.security_policy_intents[0].provider.as_str(),
        "selinux"
    );
    assert_eq!(
        bundle.security_policy_intents[0].fallback.as_str(),
        "dormant"
    );
    assert_eq!(bundle.entries[0].security_policy_intents.len(), 1);

    let encoded = manifest.to_toml().expect("serialize manifest");
    assert!(encoded.contains("[[legacy_scriptlets.security_policy_intents]]"));
    assert!(encoded.contains("[[legacy_scriptlets.entries.security_policy_intents]]"));

    let decoded = CcsManifest::parse(&encoded).expect("parse serialized manifest");
    let decoded_bundle = decoded.legacy_scriptlets.as_ref().unwrap();
    assert_eq!(
        decoded_bundle.security_policy_intents[0]
            .reconciliation
            .state
            .as_str(),
        "pending"
    );
}

#[test]
fn manifest_validation_rejects_invalid_legacy_scriptlet_bundle() {
    let body_sha256 = crate::hash::sha256_prefixed(b"ldconfig");
    let toml = manifest_with_legacy_scriptlet_bundle("ldconfig && echo tampered", &body_sha256);

    let err = CcsManifest::parse(&toml).expect_err("tampered bundle must fail");

    assert!(err.to_string().contains("legacy scriptlet bundle"));
    assert!(err.to_string().contains("body_sha256 mismatch"));
}

#[test]
fn manifest_rejects_unknown_hook_keys() {
    let toml = r#"
[package]
name = "future-hook"
version = "1.0.0"
description = "future hook"

[[hooks.some_new_hook]]
name = "must-not-be-dropped"
"#;

    let err = CcsManifest::parse(toml).expect_err("unknown hook must be rejected");
    assert!(
        err.to_string().contains("some_new_hook") || err.to_string().contains("unknown field"),
        "unexpected error: {err}"
    );
}

#[test]
fn hooks_classify_script_service_and_declarative_entries() {
    let mut hooks = Hooks::default();
    assert!(!hooks.has_script_hooks());
    assert!(!hooks.has_service_hooks());
    assert!(!hooks.has_declarative_hooks());
    assert!(!hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::HostRoot));

    hooks.directories.push(DirectoryHook {
        path: "/var/lib/conary-test".to_string(),
        mode: "0755".to_string(),
        owner: "root".to_string(),
        group: "root".to_string(),
        cleanup: None,
        reversible: None,
    });
    assert!(hooks.has_declarative_hooks());
    assert!(!hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot));
    assert!(!hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::GenerationRoot));
    assert!(hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::HostRoot));

    hooks.services.push(Service {
        name: "conary-test.service".to_string(),
        action: ServiceAction::Restart,
        reversible: None,
    });
    assert!(hooks.has_service_hooks());
    assert!(hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot));

    hooks.post_install = Some(ScriptHook {
        script: "echo post-install".to_string(),
        reversible: None,
    });
    assert!(hooks.has_script_hooks());
    assert!(hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::GenerationRoot));
}

#[test]
fn omitted_reversible_fields_keep_wire_compatibility_and_m1b_defaults() {
    let toml = r#"
[package]
name = "hook-defaults"
version = "1.0.0"
description = "hook defaults"

[[hooks.users]]
name = "hookuser"
system = true

[[hooks.services]]
name = "hook-defaults.service"
action = "restart"

[hooks.post_install]
script = "echo post-install"
"#;

    let manifest = CcsManifest::parse(toml).expect("parse manifest without reversible fields");

    assert_eq!(manifest.hooks.users[0].reversible, None);
    assert_eq!(manifest.hooks.services[0].reversible, None);
    assert_eq!(
        manifest
            .hooks
            .post_install
            .as_ref()
            .expect("post-install hook")
            .reversible,
        None
    );
    assert!(
        manifest
            .hooks
            .has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot)
    );

    let encoded = manifest.to_toml().expect("serialize manifest");
    assert!(!encoded.contains("reversible"));

    let declarative_only = CcsManifest::parse(
        r#"
[package]
name = "declarative-defaults"
version = "1.0.0"
description = "declarative defaults"

[[hooks.groups]]
name = "hookgroup"
system = true
"#,
    )
    .expect("parse declarative manifest");

    assert!(
        !declarative_only
            .hooks
            .has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot)
    );
    assert!(
        !declarative_only
            .hooks
            .has_irreversible_hooks_for_try_root(HookExecutionRoot::GenerationRoot)
    );
    assert!(
        declarative_only
            .hooks
            .has_irreversible_hooks_for_try_root(HookExecutionRoot::HostRoot)
    );
}
