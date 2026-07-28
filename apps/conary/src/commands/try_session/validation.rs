// apps/conary/src/commands/try_session/validation.rs
//! Try-session package and manifest policy.

use anyhow::{Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::manifest::{CcsManifest, HookExecutionRoot};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TryExecutionRoot {
    Namespace,
    Generation,
    Host,
}

impl TryExecutionRoot {
    fn hook_execution_root(self) -> HookExecutionRoot {
        match self {
            Self::Namespace => HookExecutionRoot::TryRoot,
            Self::Generation => HookExecutionRoot::GenerationRoot,
            Self::Host => HookExecutionRoot::HostRoot,
        }
    }
}

#[allow(dead_code)]
pub(super) fn validate_try_package_policy(
    package: &CcsPackage,
    execution_root: TryExecutionRoot,
    activated: bool,
) -> Result<()> {
    validate_try_manifest_policy(package.manifest(), execution_root, activated)
}

#[allow(dead_code)]
fn validate_try_manifest_policy(
    manifest: &CcsManifest,
    execution_root: TryExecutionRoot,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;

    if hooks.has_script_hooks() {
        bail!("{}", script_hook_policy_error(activated));
    }

    if manifest.native_lifecycle.is_some() {
        bail!(
            "try sessions have no typed native-lifecycle filesystem-delta contract; \
             install the converted package through the normal transaction path"
        );
    }

    if hooks.has_service_hooks() {
        if activated {
            bail!(
                "service lifecycle is not generation-scoped in activated M1b try sessions; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!(
            "service lifecycle is not generation-scoped in M1b try sessions; \
            hooks.services cannot run during try"
        );
    }

    validate_m1b_try_declarative_hook_support(manifest, activated)?;

    if matches!(execution_root, TryExecutionRoot::Host) && hooks.has_declarative_hooks() {
        if activated {
            bail!(
                "try hooks cannot execute against the host root; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!("try hooks cannot execute against the host root");
    }

    if hooks.has_irreversible_hooks_for_try_root(execution_root.hook_execution_root()) {
        bail!("try package contains irreversible hooks for the planned execution root");
    }

    Ok(())
}

fn validate_m1b_try_declarative_hook_support(
    manifest: &CcsManifest,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;
    if !hooks.systemd.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.systemd", activated)
        );
    }
    if !hooks.tmpfiles.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.tmpfiles", activated)
        );
    }
    if !hooks.sysctl.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.sysctl", activated)
        );
    }
    if !hooks.alternatives.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.alternatives", activated)
        );
    }
    Ok(())
}

fn unsupported_declarative_hook_error(hook_class: &str, activated: bool) -> String {
    if activated {
        format!(
            "{hook_class} are not supported in activated M1b try sessions; \
             generation-scoped effect verification for this hook class is M2 work"
        )
    } else {
        format!(
            "{hook_class} are not supported in M1b try sessions; \
             promotable try-root effect verification for this hook class is M2 work"
        )
    }
}

fn script_hook_policy_error(activated: bool) -> &'static str {
    if activated {
        "script hooks are not supported in activated M1b try sessions; \
         host-root lifecycle helper is M2 work"
    } else {
        "script hooks are not supported in M1b try sessions; \
         scripts cannot run against the host root"
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use conary_core::ccs::builder::write_signed_current_ccs_package;
    use conary_core::ccs::manifest::{
        AlternativeHook, CcsManifest, DirectoryHook, ScriptHook, Service, ServiceAction,
        SysctlHook, SystemdHook, TmpfilesHook,
    };
    use conary_core::ccs::{
        BuildResult, CcsPackage, ComponentData, FileEntry, SigningKeyPair, TrustPolicy,
    };
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};

    fn validate_manifest(
        manifest: &CcsManifest,
        execution_root: TryExecutionRoot,
        activated: bool,
    ) -> anyhow::Result<()> {
        validate_try_manifest_policy(manifest, execution_root, activated)
    }

    fn assert_policy_error_contains(
        manifest: &CcsManifest,
        execution_root: TryExecutionRoot,
        activated: bool,
        expected: &str,
    ) {
        let err = validate_manifest(manifest, execution_root, activated)
            .expect_err("policy should reject package");
        let message = err.to_string();
        assert!(
            message.contains(expected),
            "expected error to contain {expected:?}, got {message:?}"
        );
    }

    fn minimal_package(manifest: CcsManifest) -> anyhow::Result<CcsPackage> {
        let temp_dir = tempfile::tempdir()?;
        let package_path = temp_dir.path().join("try-policy.ccs");
        let content = b"try package".to_vec();
        let hash = conary_core::hash::sha256(&content);
        let files = vec![FileEntry {
            path: "/usr/bin/try-policy".to_string(),
            node: PayloadNode::regular(0o755),
            content: Some(PayloadContentAuthority {
                sha256: hash.clone(),
                size: content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        }];
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: content.len() as u64,
                },
            )]),
            files: files.clone(),
            payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
                &files,
                HashMap::from([(hash, content)]),
            )
            .unwrap(),
            total_size: 11,
            chunked: false,
            chunk_stats: None,
        };
        let signer = SigningKeyPair::generate().with_key_id("try-policy-test");
        write_signed_current_ccs_package(&result, &package_path, &signer, false)?;
        let verified = conary_core::ccs::verify::verify_package(
            &package_path,
            &TrustPolicy::strict(vec![signer.public_key_base64()]),
        )?;
        CcsPackage::from_verified_archive(&package_path.to_string_lossy(), &verified)
            .map_err(|error| anyhow::anyhow!(error))
    }

    fn manifest_with_post_install_script() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("script-post", "1.0.0");
        manifest.hooks.post_install = Some(ScriptHook {
            script: "echo post-install".to_string(),
            reversible: None,
        });
        manifest
    }

    fn manifest_with_pre_remove_script() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("script-pre", "1.0.0");
        manifest.hooks.pre_remove = Some(ScriptHook {
            script: "echo pre-remove".to_string(),
            reversible: None,
        });
        manifest
    }

    fn manifest_with_declarative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("declarative", "1.0.0");
        manifest.hooks.directories.push(DirectoryHook {
            path: "/var/lib/declarative".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: None,
        });
        manifest
    }

    fn manifest_with_systemd_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("systemd-hook", "1.0.0");
        manifest.hooks.systemd.push(SystemdHook {
            unit: "try-systemd.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_tmpfiles_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("tmpfiles-hook", "1.0.0");
        manifest.hooks.tmpfiles.push(TmpfilesHook {
            entry_type: "d".to_string(),
            path: "/var/lib/try-tmpfiles".to_string(),
            mode: "0755".to_string(),
            user: "root".to_string(),
            group: "root".to_string(),
            age: "-".to_string(),
            argument: "-".to_string(),
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_sysctl_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("sysctl-hook", "1.0.0");
        manifest.hooks.sysctl.push(SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "0".to_string(),
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_alternative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("alternative-hook", "1.0.0");
        manifest.hooks.alternatives.push(AlternativeHook {
            link: "/usr/bin/try-editor".to_string(),
            name: "try-editor".to_string(),
            path: "/usr/bin/try-editor".to_string(),
            priority: 50,
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_service_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("service-hook", "1.0.0");
        manifest.hooks.services.push(Service {
            name: "service-hook.service".to_string(),
            action: ServiceAction::Restart,
            reversible: None,
        });
        manifest
    }

    fn manifest_with_native_lifecycle_bundle() -> CcsManifest {
        let body = "ldconfig";
        let body_sha256 = conary_core::hash::sha256_prefixed(body.as_bytes());
        let toml = format!(
            r#"
[package]
name = "native-lifecycles"
version = "1.0.0"
version_scheme = "rpm"
release = "1"
kind = "package"
description = "native lifecycles"

[native_lifecycle]
schema = "conary.native-lifecycles.v1"
schema_revision = 19
source_format = "rpm"
source_family = "fedora-rhel"
source_profile = "fedora-44"
source_release = "44"
source_arch = "x86_64"
source_package = "native-lifecycles"
source_version = "1.0.0-1.fc44"
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
        );

        CcsManifest::parse(&toml).expect("parse native lifecycle fixture")
    }

    #[test]
    fn package_with_no_hooks_is_allowed() -> anyhow::Result<()> {
        let manifest = CcsManifest::new_minimal("no-hooks", "1.0.0");
        validate_manifest(&manifest, TryExecutionRoot::Namespace, false)?;
        validate_manifest(&manifest, TryExecutionRoot::Generation, false)?;
        validate_manifest(&manifest, TryExecutionRoot::Host, true)?;

        let package = minimal_package(manifest)?;
        validate_try_package_policy(&package, TryExecutionRoot::Namespace, false)
    }

    #[test]
    fn declarative_hooks_are_allowed_only_for_try_or_generation_roots() {
        let manifest = manifest_with_declarative_hook();

        validate_manifest(&manifest, TryExecutionRoot::Namespace, false)
            .expect("namespace-root declarative hooks should be allowed");
        validate_manifest(&manifest, TryExecutionRoot::Generation, false)
            .expect("generation-root declarative hooks should be allowed");
        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Host,
            false,
            "try hooks cannot execute against the host root",
        );
    }

    #[test]
    fn post_install_script_hooks_are_rejected_by_default() {
        let manifest = manifest_with_post_install_script();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            "scripts cannot run against the host root",
        );
    }

    #[test]
    fn pre_remove_script_hooks_are_rejected_by_default() {
        let manifest = manifest_with_pre_remove_script();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            "scripts cannot run against the host root",
        );
    }

    #[test]
    fn native_lifecycle_bundles_fail_before_lossy_try_publication() {
        let manifest = manifest_with_native_lifecycle_bundle();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            "no typed native-lifecycle filesystem-delta contract",
        );
        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Generation,
            true,
            "no typed native-lifecycle filesystem-delta contract",
        );
    }

    #[test]
    fn service_hooks_are_rejected_in_m1b() {
        let manifest = manifest_with_service_hook();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            "service lifecycle is not generation-scoped",
        );
    }

    #[test]
    fn unsupported_declarative_hook_classes_are_rejected_in_m1b_try_policy() {
        for (manifest, expected) in [
            (manifest_with_systemd_hook(), "hooks.systemd"),
            (manifest_with_tmpfiles_hook(), "hooks.tmpfiles"),
            (manifest_with_sysctl_hook(), "hooks.sysctl"),
            (manifest_with_alternative_hook(), "hooks.alternatives"),
        ] {
            assert_policy_error_contains(&manifest, TryExecutionRoot::Namespace, false, expected);
            assert_policy_error_contains(&manifest, TryExecutionRoot::Generation, true, "M2");
        }
    }

    #[test]
    fn package_round_trip_preserves_service_hooks_for_policy() -> anyhow::Result<()> {
        let package = minimal_package(manifest_with_service_hook())?;

        let err = validate_try_package_policy(&package, TryExecutionRoot::Namespace, false)
            .expect_err("package service hook should be rejected after round trip");

        assert!(
            err.to_string()
                .contains("service lifecycle is not generation-scoped"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn package_round_trip_preserves_declarative_reversibility_for_policy() -> anyhow::Result<()> {
        let mut manifest = manifest_with_declarative_hook();
        manifest.hooks.directories[0].reversible = Some(false);
        let package = minimal_package(manifest)?;

        for (execution_root, activated) in [
            (TryExecutionRoot::Namespace, false),
            (TryExecutionRoot::Generation, true),
        ] {
            let err = validate_try_package_policy(&package, execution_root, activated)
                .expect_err("irreversible declarative hook should always be rejected");
            assert!(
                err.to_string()
                    .contains("try package contains irreversible hooks"),
                "unexpected error: {err}"
            );
        }
        Ok(())
    }

    #[test]
    fn script_and_service_hooks_remain_rejected() {
        assert_policy_error_contains(
            &manifest_with_post_install_script(),
            TryExecutionRoot::Namespace,
            false,
            "scripts cannot run against the host root",
        );
        assert_policy_error_contains(
            &manifest_with_pre_remove_script(),
            TryExecutionRoot::Namespace,
            true,
            "host-root lifecycle helper is M2 work",
        );
        assert_policy_error_contains(
            &manifest_with_service_hook(),
            TryExecutionRoot::Generation,
            false,
            "service lifecycle is not generation-scoped",
        );
        assert_policy_error_contains(
            &manifest_with_service_hook(),
            TryExecutionRoot::Generation,
            true,
            "host-root lifecycle helper is M2 work",
        );
    }
}
