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
    allow_irreversible: bool,
    activated: bool,
) -> Result<()> {
    validate_try_manifest_policy(
        package.manifest(),
        execution_root,
        allow_irreversible,
        activated,
    )
}

#[allow(dead_code)]
fn validate_try_manifest_policy(
    manifest: &CcsManifest,
    execution_root: TryExecutionRoot,
    allow_irreversible: bool,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;

    if hooks.has_script_hooks() {
        bail!("{}", script_hook_policy_error(activated));
    }

    if manifest.legacy_scriptlets.is_some() {
        if activated {
            bail!(
                "legacy scriptlet bundles are not supported in activated M1b try sessions; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!(
            "legacy scriptlet bundles are not supported in M1b try sessions; \
             replay against try roots requires a reviewed lifecycle helper"
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

    if hooks.has_irreversible_hooks_for_try_root(execution_root.hook_execution_root())
        && !allow_irreversible
    {
        bail!(
            "try package contains irreversible hooks for the planned execution root; \
             pass --allow-irreversible only after review"
        );
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
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::{
        AlternativeHook, CcsManifest, DirectoryHook, ScriptHook, Service, ServiceAction,
        SysctlHook, SystemdHook, TmpfilesHook,
    };
    use conary_core::ccs::{BuildResult, CcsPackage, ComponentData, FileEntry, FileType};
    use conary_core::packages::traits::PackageFormat;

    fn validate_manifest(
        manifest: &CcsManifest,
        execution_root: TryExecutionRoot,
        allow_irreversible: bool,
        activated: bool,
    ) -> anyhow::Result<()> {
        validate_try_manifest_policy(manifest, execution_root, allow_irreversible, activated)
    }

    fn assert_policy_error_contains(
        manifest: &CcsManifest,
        execution_root: TryExecutionRoot,
        allow_irreversible: bool,
        activated: bool,
        expected: &str,
    ) {
        let err = validate_manifest(manifest, execution_root, allow_irreversible, activated)
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
            hash: hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
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
            files,
            blobs: HashMap::from([(hash, content)]),
            total_size: 11,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path)?;
        <CcsPackage as PackageFormat>::parse(&package_path.to_string_lossy())
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
            owner: "root".to_string(),
            group: "root".to_string(),
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_sysctl_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("sysctl-hook", "1.0.0");
        manifest.hooks.sysctl.push(SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "0".to_string(),
            only_if_lower: false,
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_alternative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("alternative-hook", "1.0.0");
        manifest.hooks.alternatives.push(AlternativeHook {
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

    fn manifest_with_legacy_scriptlet_bundle() -> CcsManifest {
        let body = "ldconfig";
        let body_sha256 = conary_core::hash::sha256_prefixed(body.as_bytes());
        let toml = format!(
            r#"
[package]
name = "legacy-scriptlets"
version = "1.0.0"
description = "legacy scriptlets"

[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
source_format = "rpm"
source_family = "fedora-rhel"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "legacy-scriptlets"
source_version = "1.0.0-1.fc44"
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
source = "static-signal"
confidence = "declared"
replacement = "complete"
"#
        );

        CcsManifest::parse(&toml).expect("parse legacy scriptlet fixture")
    }

    #[test]
    fn package_with_no_hooks_is_allowed() -> anyhow::Result<()> {
        let manifest = CcsManifest::new_minimal("no-hooks", "1.0.0");
        validate_manifest(&manifest, TryExecutionRoot::Namespace, false, false)?;
        validate_manifest(&manifest, TryExecutionRoot::Generation, false, false)?;
        validate_manifest(&manifest, TryExecutionRoot::Host, false, true)?;

        let package = minimal_package(manifest)?;
        validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)
    }

    #[test]
    fn declarative_hooks_are_allowed_only_for_try_or_generation_roots() {
        let manifest = manifest_with_declarative_hook();

        validate_manifest(&manifest, TryExecutionRoot::Namespace, false, false)
            .expect("namespace-root declarative hooks should be allowed");
        validate_manifest(&manifest, TryExecutionRoot::Generation, false, false)
            .expect("generation-root declarative hooks should be allowed");
        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Host,
            false,
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
            false,
            "scripts cannot run against the host root",
        );
    }

    #[test]
    fn legacy_scriptlet_bundles_are_rejected_by_default() {
        let manifest = manifest_with_legacy_scriptlet_bundle();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            false,
            "legacy scriptlet bundles are not supported in M1b try sessions",
        );
    }

    #[test]
    fn service_hooks_are_rejected_in_m1b() {
        let manifest = manifest_with_service_hook();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
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
            assert_policy_error_contains(
                &manifest,
                TryExecutionRoot::Namespace,
                true,
                false,
                expected,
            );
            assert_policy_error_contains(&manifest, TryExecutionRoot::Generation, true, true, "M2");
        }
    }

    #[test]
    fn package_round_trip_preserves_service_hooks_for_policy() -> anyhow::Result<()> {
        let package = minimal_package(manifest_with_service_hook())?;

        let err = validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)
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

        let err = validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)
            .expect_err("irreversible declarative hook should be rejected after round trip");
        assert!(
            err.to_string()
                .contains("try package contains irreversible hooks"),
            "unexpected error: {err}"
        );

        validate_try_package_policy(&package, TryExecutionRoot::Namespace, true, false)?;
        validate_try_package_policy(&package, TryExecutionRoot::Generation, true, false)
    }

    #[test]
    fn allow_irreversible_does_not_permit_scripts_legacy_or_services() {
        assert_policy_error_contains(
            &manifest_with_post_install_script(),
            TryExecutionRoot::Namespace,
            true,
            false,
            "scripts cannot run against the host root",
        );
        assert_policy_error_contains(
            &manifest_with_pre_remove_script(),
            TryExecutionRoot::Namespace,
            true,
            true,
            "host-root lifecycle helper is M2 work",
        );
        assert_policy_error_contains(
            &manifest_with_legacy_scriptlet_bundle(),
            TryExecutionRoot::Namespace,
            true,
            false,
            "legacy scriptlet bundles are not supported in M1b try sessions",
        );
        assert_policy_error_contains(
            &manifest_with_legacy_scriptlet_bundle(),
            TryExecutionRoot::Namespace,
            true,
            true,
            "host-root lifecycle helper is M2 work",
        );
        assert_policy_error_contains(
            &manifest_with_service_hook(),
            TryExecutionRoot::Generation,
            true,
            false,
            "service lifecycle is not generation-scoped",
        );
        assert_policy_error_contains(
            &manifest_with_service_hook(),
            TryExecutionRoot::Generation,
            true,
            true,
            "host-root lifecycle helper is M2 work",
        );
    }
}
