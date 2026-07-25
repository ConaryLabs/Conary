// conary-core/src/ccs/manifest/tests/hooks.rs

use super::super::*;

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
fn omitted_reversible_fields_use_current_hook_semantics() {
    let toml = r#"
[package]
name = "hook-defaults"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
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
version_scheme = "conary"
release = "1"
kind = "package"
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
