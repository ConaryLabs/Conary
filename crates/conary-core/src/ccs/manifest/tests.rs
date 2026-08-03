// conary-core/src/ccs/manifest/tests.rs

use super::*;

mod hooks;

#[test]
fn test_minimal_manifest() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "My application"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provides]
capabilities = ["cli-tool", "json-parsing"]

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

[[config.files]]
source = "ccs"
path = "/etc/myapp/config.toml"
noreplace = true
payload = "matched"
"#;
    let manifest = CcsManifest::parse(toml).unwrap();
    assert_eq!(manifest.package.name, "myapp");
    assert_eq!(manifest.provides.capabilities.len(), 2);
    assert!(manifest.requirements.is_empty());
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
    assert!(toml.contains("[package.platform]"));
    assert!(toml.contains("arch = \"noarch\""));
    let decoded = CcsManifest::parse(&toml).unwrap();
    assert_eq!(
        decoded
            .package
            .platform
            .as_ref()
            .and_then(|platform| platform.arch.as_deref()),
        Some(DEFAULT_CONARY_ARCHITECTURE)
    );
}

#[test]
fn package_wide_config_policy_is_not_in_the_current_manifest_schema() {
    let toml = r#"
[package]
name = "old-config-shape"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "retired config shape"

[config]
files = []
noreplace = true
"#;

    let error = CcsManifest::parse(toml).unwrap_err();

    assert!(error.to_string().contains("noreplace"), "{error}");
}

#[test]
fn manifest_validation_uses_the_declared_package_syscall_architecture() {
    let mut manifest = CcsManifest::new_minimal("targeted", "1.0.0");
    manifest.package.platform = Some(Platform {
        arch: Some("aarch64".to_string()),
        ..Default::default()
    });
    let mut capabilities = CapabilityDeclaration::default();
    capabilities.syscalls.allow.push("arch_prctl".to_string());
    manifest.capabilities = Some(capabilities);

    let error = manifest
        .validate()
        .expect_err("x86_64-only syscall must not validate for an aarch64 package");
    assert!(error.to_string().contains("aarch64 ABI"));
}

#[test]
fn flat_requires_section_is_not_in_the_current_manifest_schema() {
    let error = CcsManifest::parse(
        r#"
[package]
name = "retired-flat"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "retired flat requirement"

[requires]
packages = [{ name = "openssl", version = ">= 3" }]
"#,
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("unknown field `requires`"),
        "{error}"
    );
}

#[test]
fn native_export_rejects_unknown_target_metadata() {
    let error = CcsManifest::parse(
        r#"
[package]
name = "unknown-native-export"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "unknown native export field"

[native_export.rpm]
invented_license = "MIT"
"#,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unknown field `invented_license`"),
        "{error}"
    );
}

#[test]
fn package_release_and_kind_are_required_current_identity() {
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "hello"
version = "0.1.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "hello"
"#,
    )
    .unwrap();

    assert_eq!(manifest.package.release, "1");
    assert_eq!(
        manifest.package.kind,
        crate::ccs::v3::PackageKindTagV3::Package
    );

    let missing_release = CcsManifest::parse(
        r#"
[package]
name = "missing-release"
version = "1.0.0"
version_scheme = "conary"
kind = "package"
description = "invalid"
"#,
    )
    .unwrap_err();
    assert!(
        missing_release
            .to_string()
            .contains("missing field `release`"),
        "{missing_release}"
    );

    let missing_kind = CcsManifest::parse(
        r#"
[package]
name = "missing-kind"
version = "1.0.0"
version_scheme = "conary"
release = "1"
description = "invalid"
"#,
    )
    .unwrap_err();
    assert!(
        missing_kind.to_string().contains("missing field `kind`"),
        "{missing_kind}"
    );
}

#[test]
fn typed_requirements_are_root_authority_and_cannot_hide_under_provides() {
    let manifest = CcsManifest::parse(
        r#"
requirements = [
    { kind = "Depends", behavior = "Hard", expression = { operator = "atom", operands = { name = "openssl", capability_kind = "PackageName", version_constraint = ">=3.0.0", native_text = "openssl >=3.0.0" } }, alternatives = [{ name = "openssl", capability_kind = "PackageName", version_constraint = ">=3.0.0", native_text = "openssl >=3.0.0" }], native_text = "openssl >=3.0.0" },
]

[package]
name = "typed-requirement"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "typed requirement"
"#,
    )
    .unwrap();

    assert_eq!(manifest.requirements.len(), 1);
    assert_eq!(manifest.requirements[0].alternatives[0].name, "openssl");

    let nested_error = CcsManifest::parse(
        r#"
[package]
name = "nested-requirement"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "invalid"

[provides]
requirements = []
"#,
    )
    .unwrap_err();
    assert!(
        nested_error
            .to_string()
            .contains("unknown field `requirements`"),
        "{nested_error}"
    );
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
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[hooks.users]]
name = "daemon"
system = false
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("system user"));
}

#[test]
fn test_manifest_rejects_removed_partial_tmpfiles_shape() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[hooks.tmpfiles]]
type = "d"
path = "/run/test"
mode = "0755"
user = "root"
group = "root"
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("age"));
}

#[test]
fn test_manifest_preserves_complete_tmpfiles_entry() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[hooks.tmpfiles]]
type = "w+!-$"
path = "/proc/sys/vm/swappiness"
mode = "-"
user = "-"
group = "-"
age = "-"
argument = "10 with exact spacing"
"#;

    let manifest = CcsManifest::parse(toml).unwrap();
    let entry = &manifest.hooks.tmpfiles[0];
    assert_eq!(entry.entry_type, "w+!-$");
    assert_eq!(entry.mode, "-");
    assert_eq!(entry.user, "-");
    assert_eq!(entry.group, "-");
    assert_eq!(entry.age, "-");
    assert_eq!(entry.argument, "10 with exact spacing");
}

#[test]
fn test_manifest_rejects_denied_sysctl_keys() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[hooks.systemd]]
unit = "../evil.service"
enable = true
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("systemd"));
}

#[test]
fn test_manifest_rejects_unsafe_declarative_service_names() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[hooks.services]]
name = "../evil.service"
action = "restart"
"#;

    let err = CcsManifest::parse(toml).unwrap_err();
    assert!(err.to_string().contains("services"));
}

#[test]
fn test_manifest_accepts_supported_scriptlet_capabilities() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
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
fn test_manifest_rejects_duplicate_file_capability_authority() {
    let mut manifest = CcsManifest::new_minimal("file-capable", "1.0.0");
    manifest.file_capabilities = vec![FileCapability {
        path: "/usr/bin/server".to_string(),
        capabilities: vec![
            "cap_net_bind_service".to_string(),
            "cap_net_bind_service".to_string(),
        ],
        permitted: true,
        effective: true,
        inheritable: false,
    }];
    let duplicate_name = manifest.validate().unwrap_err();
    assert!(
        duplicate_name
            .to_string()
            .contains("duplicate Linux file capability")
    );

    manifest.file_capabilities = vec![
        FileCapability {
            path: "/usr/bin/server".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        },
        FileCapability {
            path: "/usr/bin/server".to_string(),
            capabilities: vec!["cap_net_raw".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        },
    ];
    let duplicate_path = manifest.validate().unwrap_err();
    assert!(
        duplicate_path
            .to_string()
            .contains("declared more than once")
    );
}

#[test]
fn test_manifest_accepts_inheritable_file_capabilities() {
    let toml = r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[file_capabilities]]
path = "/usr/bin/demo"
capabilities = ["cap_net_bind_service"]
permitted = true
effective = true
inheritable = true
"#;

    let manifest = CcsManifest::parse(toml).unwrap();
    let capability = &manifest.file_capabilities[0];
    assert!(capability.inheritable);
    assert_eq!(
        capability.to_setcap_spec().unwrap(),
        "cap_net_bind_service=+eip"
    );
}

#[test]
fn test_manifest_rejects_unsafe_file_capabilities() {
    for (toml, expected) in [
        (
            r#"
[package]
name = "test"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "test"

[[file_capabilities]]
path = "/usr/bin/demo"
capabilities = ["cap_net_bind_service"]
permitted = false
effective = true
"#,
            "effective file capability requires permitted",
        ),
    ] {
        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "expected {expected:?}, got {err}"
        );
    }
}

fn manifest_with_native_lifecycle_bundle(body: &str, body_sha256: &str) -> String {
    format!(
        r#"
[package]
name = "nginx"
version = "1.28.0"
version_scheme = "rpm"
release = "1"
kind = "package"
description = "nginx converted from RPM"

[native_lifecycle]
schema = "conary.native-lifecycles.v1"
schema_revision = 19
source_format = "rpm"
source_family = "fedora-rhel"
source_profile = "fedora-44"
source_release = "44"
source_arch = "x86_64"
source_package = "nginx"
source_version = "1.28.0-1.fc44"
source_checksum = "sha256:3333333333333333333333333333333333333333333333333333333333333333"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.8.0"
conversion_policy = "typed-native-lifecycle"
scriptlet_fidelity = "native-lifecycle"

[[native_lifecycle.entries]]
id = "rpm:%post"
native_slot = "%post"
kind = "executable"
phase = "post-install"
lifecycle_paths = ["install:first"]
interpreter = "/bin/sh"
interpreter_args = ["-e"]
body_sha256 = "{body_sha256}"
body = "{body}"
native_invocation = {{ args = ["1"], environment = ["RPM_INSTALL_PREFIX=/"], stdin = "none", chroot = "install-root" }}
transaction_order = {{ position = "after-payload", after = ["payload"] }}
timeout_ms = 30000
rpm_runtime = {{ program = "external", critical = false, criticality = "warning-only", raw_flags = 0 }}
"#
    )
}

#[test]
fn manifest_toml_round_trips_native_lifecycle_bundle() {
    let body = "ldconfig";
    let body_sha256 = crate::hash::sha256_prefixed(body.as_bytes());
    let toml = manifest_with_native_lifecycle_bundle(body, &body_sha256);

    let manifest = CcsManifest::parse(&toml).expect("parse manifest");
    let bundle = manifest
        .native_lifecycle
        .as_ref()
        .expect("native lifecycle bundle");

    assert_eq!(bundle.source_package, "nginx");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].id, "rpm:%post");

    let encoded = manifest.to_toml().expect("serialize manifest");
    assert!(encoded.contains("[native_lifecycle]"));
    let decoded = CcsManifest::parse(&encoded).expect("parse serialized manifest");
    assert_eq!(
        decoded
            .native_lifecycle
            .as_ref()
            .expect("native lifecycle bundle")
            .entries[0]
            .body,
        "ldconfig"
    );
}

#[test]
fn manifest_rejects_removed_security_policy_intent_authority() {
    let toml = r#"
[package]
name = "demo-policy"
version = "1.0.0"
version_scheme = "rpm"
release = "1"
kind = "package"
description = "policy fixture"

[native_lifecycle]
schema = "conary.native-lifecycles.v1"
schema_revision = 19
source_format = "rpm"
source_family = "rpm"
source_profile = "fedora-44"
source_release = "44"
source_arch = "x86_64"
source_package = "demo-policy"
source_version = "1.0.0-1.fc44"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.10.1"
conversion_policy = "passive-scriptlet-bundle-goal4"
scriptlet_fidelity = "native-free"
security_policy_intents = []
"#;

    let error = CcsManifest::parse(toml)
        .expect_err("removed metadata-only policy intent authority must fail closed");
    assert!(error.to_string().contains("security_policy_intents"));
}

#[test]
fn manifest_validation_rejects_invalid_native_lifecycle_bundle() {
    let body_sha256 = crate::hash::sha256_prefixed(b"ldconfig");
    let toml = manifest_with_native_lifecycle_bundle("ldconfig && echo tampered", &body_sha256);

    let err = CcsManifest::parse(&toml).expect_err("tampered bundle must fail");

    assert!(err.to_string().contains("native lifecycle bundle"));
    assert!(err.to_string().contains("body_sha256 mismatch"));
}

#[test]
fn manifest_rejects_unknown_hook_keys() {
    let toml = r#"
[package]
name = "future-hook"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
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
fn typed_package_relations_round_trip_through_manifest_toml() {
    let mut manifest = CcsManifest::new_minimal("replacement", "2.0-1");
    manifest.package.version_scheme = crate::repository::versioning::VersionScheme::Debian;
    manifest.package.debian_multi_arch =
        Some(crate::repository::dependency_model::DebianMultiArch::No);
    manifest.relations.push(
        crate::repository::package_relation::parse_native_relation(
            crate::repository::dependency_model::RepositoryRequirementKind::Replace,
            crate::repository::versioning::VersionScheme::Debian,
            "old-package (<< 2.0)",
        )
        .unwrap(),
    );

    let encoded = manifest.to_toml().unwrap();
    let decoded = CcsManifest::parse(&encoded).unwrap();

    assert_eq!(decoded.relations, manifest.relations);
    assert_eq!(
        decoded.package.version_scheme,
        crate::repository::versioning::VersionScheme::Debian
    );
}

#[test]
fn typed_positive_requirements_round_trip_through_manifest_toml() {
    let mut manifest = CcsManifest::new_minimal("rich-consumer", "1.0-1");
    manifest.package.version_scheme = crate::repository::versioning::VersionScheme::Rpm;
    manifest.requirements.push(
        crate::repository::requirement::parse_native_requirement(
            crate::repository::dependency_model::RepositoryRequirementKind::Depends,
            crate::repository::versioning::VersionScheme::Rpm,
            "((feature-a with feature-b) if engine else fallback)",
        )
        .unwrap(),
    );

    let encoded = manifest.to_toml().unwrap();
    let decoded = CcsManifest::parse(&encoded).unwrap();

    assert_eq!(decoded.requirements, manifest.requirements);
    assert_eq!(
        decoded.package.version_scheme,
        crate::repository::versioning::VersionScheme::Rpm
    );
}
