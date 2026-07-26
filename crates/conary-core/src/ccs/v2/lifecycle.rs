// conary-core/src/ccs/v2/lifecycle.rs

//! Exact projection between native manifest lifecycle declarations and signed
//! CCS v2 lifecycle authority.

use super::schema::*;
use crate::ccs::manifest::{
    AlternativeHook, CcsManifest, DirectoryHook, GroupHook, ScriptHook,
    ScriptletCapabilityDeclaration, Service, ServiceAction, SysctlHook, SystemdHook, TmpfilesHook,
    UserHook,
};

pub(crate) fn authority_from_manifest(manifest: &CcsManifest) -> LifecycleAuthorityV2 {
    let capabilities = manifest
        .scriptlets
        .capabilities
        .iter()
        .map(|capability| LifecycleScriptCapabilityV2 {
            name: capability.name.clone(),
            paths: capability.paths.clone(),
        })
        .collect::<Vec<_>>();
    LifecycleAuthorityV2 {
        users: manifest
            .hooks
            .users
            .iter()
            .map(|hook| LifecycleUserV2 {
                name: hook.name.clone(),
                system: hook.system,
                home: hook.home.clone(),
                shell: hook.shell.clone(),
                group: hook.group.clone(),
                reversible: hook.reversible,
            })
            .collect(),
        groups: manifest
            .hooks
            .groups
            .iter()
            .map(|hook| LifecycleGroupV2 {
                name: hook.name.clone(),
                system: hook.system,
                reversible: hook.reversible,
            })
            .collect(),
        directories: manifest
            .hooks
            .directories
            .iter()
            .map(|hook| LifecycleDirectoryV2 {
                path: hook.path.clone(),
                mode: hook.mode.clone(),
                owner: hook.owner.clone(),
                group: hook.group.clone(),
                cleanup: hook.cleanup.clone(),
                reversible: hook.reversible,
            })
            .collect(),
        services: manifest
            .hooks
            .services
            .iter()
            .map(|hook| LifecycleServiceV2 {
                name: hook.name.clone(),
                action: service_action_from_manifest(&hook.action),
                reversible: hook.reversible,
            })
            .collect(),
        systemd: manifest
            .hooks
            .systemd
            .iter()
            .map(|hook| LifecycleSystemdV2 {
                unit: hook.unit.clone(),
                enable: hook.enable,
                reversible: hook.reversible,
            })
            .collect(),
        tmpfiles: manifest
            .hooks
            .tmpfiles
            .iter()
            .map(|hook| LifecycleTmpfilesV2 {
                entry_type: hook.entry_type.clone(),
                path: hook.path.clone(),
                mode: hook.mode.clone(),
                user: hook.user.clone(),
                group: hook.group.clone(),
                age: hook.age.clone(),
                argument: hook.argument.clone(),
                reversible: hook.reversible,
            })
            .collect(),
        sysctl: manifest
            .hooks
            .sysctl
            .iter()
            .map(|hook| LifecycleSysctlV2 {
                key: hook.key.clone(),
                value: hook.value.clone(),
                reversible: hook.reversible,
            })
            .collect(),
        alternatives: manifest
            .hooks
            .alternatives
            .iter()
            .map(|hook| LifecycleAlternativeV2 {
                link: hook.link.clone(),
                name: hook.name.clone(),
                path: hook.path.clone(),
                priority: hook.priority,
                reversible: hook.reversible,
            })
            .collect(),
        script_capabilities: capabilities.clone(),
        post_install: manifest
            .hooks
            .post_install
            .as_ref()
            .map(|hook| script_from_manifest(hook, &capabilities)),
        pre_remove: manifest
            .hooks
            .pre_remove
            .as_ref()
            .map(|hook| script_from_manifest(hook, &capabilities)),
        native_lifecycle: manifest.native_lifecycle.clone(),
    }
}

pub(crate) fn apply_authority_to_manifest(
    authority: &LifecycleAuthorityV2,
    manifest: &mut CcsManifest,
) -> Result<(), String> {
    validate_script_capability_projection(authority)?;
    manifest.hooks.users = authority
        .users
        .iter()
        .map(|hook| UserHook {
            name: hook.name.clone(),
            system: hook.system,
            home: hook.home.clone(),
            shell: hook.shell.clone(),
            group: hook.group.clone(),
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.groups = authority
        .groups
        .iter()
        .map(|hook| GroupHook {
            name: hook.name.clone(),
            system: hook.system,
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.directories = authority
        .directories
        .iter()
        .map(|hook| DirectoryHook {
            path: hook.path.clone(),
            mode: hook.mode.clone(),
            owner: hook.owner.clone(),
            group: hook.group.clone(),
            cleanup: hook.cleanup.clone(),
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.services = authority
        .services
        .iter()
        .map(|hook| Service {
            name: hook.name.clone(),
            action: service_action_to_manifest(hook.action),
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.systemd = authority
        .systemd
        .iter()
        .map(|hook| SystemdHook {
            unit: hook.unit.clone(),
            enable: hook.enable,
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.tmpfiles = authority
        .tmpfiles
        .iter()
        .map(|hook| TmpfilesHook {
            entry_type: hook.entry_type.clone(),
            path: hook.path.clone(),
            mode: hook.mode.clone(),
            user: hook.user.clone(),
            group: hook.group.clone(),
            age: hook.age.clone(),
            argument: hook.argument.clone(),
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.sysctl = authority
        .sysctl
        .iter()
        .map(|hook| SysctlHook {
            key: hook.key.clone(),
            value: hook.value.clone(),
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.alternatives = authority
        .alternatives
        .iter()
        .map(|hook| AlternativeHook {
            link: hook.link.clone(),
            name: hook.name.clone(),
            path: hook.path.clone(),
            priority: hook.priority,
            reversible: hook.reversible,
        })
        .collect();
    manifest.hooks.post_install = authority.post_install.as_ref().map(script_to_manifest);
    manifest.hooks.pre_remove = authority.pre_remove.as_ref().map(script_to_manifest);
    manifest.native_lifecycle = authority.native_lifecycle.clone();
    manifest.scriptlets.capabilities = authority
        .script_capabilities
        .iter()
        .map(|capability| ScriptletCapabilityDeclaration {
            name: capability.name.clone(),
            paths: capability.paths.clone(),
        })
        .collect();
    Ok(())
}

fn service_action_from_manifest(action: &ServiceAction) -> LifecycleServiceActionV2 {
    match action {
        ServiceAction::Enable => LifecycleServiceActionV2::Enable,
        ServiceAction::Disable => LifecycleServiceActionV2::Disable,
        ServiceAction::Start => LifecycleServiceActionV2::Start,
        ServiceAction::Stop => LifecycleServiceActionV2::Stop,
        ServiceAction::Reload => LifecycleServiceActionV2::Reload,
        ServiceAction::Restart => LifecycleServiceActionV2::Restart,
        ServiceAction::TryRestart => LifecycleServiceActionV2::TryRestart,
        ServiceAction::ReloadOrRestart => LifecycleServiceActionV2::ReloadOrRestart,
        ServiceAction::ReloadOrTryRestart => LifecycleServiceActionV2::ReloadOrTryRestart,
    }
}

fn service_action_to_manifest(action: LifecycleServiceActionV2) -> ServiceAction {
    match action {
        LifecycleServiceActionV2::Enable => ServiceAction::Enable,
        LifecycleServiceActionV2::Disable => ServiceAction::Disable,
        LifecycleServiceActionV2::Start => ServiceAction::Start,
        LifecycleServiceActionV2::Stop => ServiceAction::Stop,
        LifecycleServiceActionV2::Reload => ServiceAction::Reload,
        LifecycleServiceActionV2::Restart => ServiceAction::Restart,
        LifecycleServiceActionV2::TryRestart => ServiceAction::TryRestart,
        LifecycleServiceActionV2::ReloadOrRestart => ServiceAction::ReloadOrRestart,
        LifecycleServiceActionV2::ReloadOrTryRestart => ServiceAction::ReloadOrTryRestart,
    }
}

fn script_from_manifest(
    hook: &ScriptHook,
    capabilities: &[LifecycleScriptCapabilityV2],
) -> LifecycleScriptV2 {
    LifecycleScriptV2 {
        interpreter: "/bin/sh".to_string(),
        body: hook.script.clone(),
        capabilities: capabilities.to_vec(),
        reversible: hook.reversible,
        execution: LifecycleScriptExecutionV2::SandboxedTargetRoot,
    }
}

fn script_to_manifest(hook: &LifecycleScriptV2) -> ScriptHook {
    ScriptHook {
        script: hook.body.clone(),
        reversible: hook.reversible,
    }
}

fn validate_script_capability_projection(authority: &LifecycleAuthorityV2) -> Result<(), String> {
    for (phase, script) in [
        ("post-install", authority.post_install.as_ref()),
        ("pre-remove", authority.pre_remove.as_ref()),
    ] {
        if let Some(script) = script
            && script.capabilities != authority.script_capabilities
        {
            return Err(format!(
                "{phase} capabilities differ from the exact package-scoped hook capability set"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_lifecycle_round_trips_without_losing_execution_fields() {
        let mut manifest = CcsManifest::new_minimal("lifecycle", "1.0.0");
        manifest.hooks.users.push(UserHook {
            name: "example".to_string(),
            system: true,
            home: Some("/var/lib/example".to_string()),
            shell: Some("/usr/sbin/nologin".to_string()),
            group: Some("example".to_string()),
            reversible: Some(true),
        });
        manifest.hooks.groups.push(GroupHook {
            name: "example".to_string(),
            system: true,
            reversible: Some(true),
        });
        manifest.hooks.directories.push(DirectoryHook {
            path: "/var/lib/example".to_string(),
            mode: "0750".to_string(),
            owner: "example".to_string(),
            group: "example".to_string(),
            cleanup: Some("30d".to_string()),
            reversible: Some(true),
        });
        manifest.hooks.services.push(Service {
            name: "example".to_string(),
            action: ServiceAction::Restart,
            reversible: Some(false),
        });
        manifest.hooks.systemd.push(SystemdHook {
            unit: "example.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
        manifest.hooks.tmpfiles.push(TmpfilesHook {
            entry_type: "f+!$".to_string(),
            path: "/run/example".to_string(),
            mode: "~:0640".to_string(),
            user: ":example".to_string(),
            group: "example".to_string(),
            age: "am:30d".to_string(),
            argument: "exact payload with spaces".to_string(),
            reversible: Some(true),
        });
        manifest.hooks.sysctl.push(SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "1".to_string(),
            reversible: Some(false),
        });
        manifest.hooks.alternatives.push(AlternativeHook {
            link: "/usr/bin/example".to_string(),
            name: "example".to_string(),
            path: "/usr/bin/example-v2".to_string(),
            priority: 80,
            reversible: Some(true),
        });
        manifest
            .scriptlets
            .capabilities
            .push(ScriptletCapabilityDeclaration {
                name: "systemd-service-registration".to_string(),
                paths: vec!["/etc/systemd/system".to_string()],
            });
        manifest.hooks.post_install = Some(ScriptHook {
            script: "printf installed".to_string(),
            reversible: Some(false),
        });
        manifest.hooks.pre_remove = Some(ScriptHook {
            script: "printf removing".to_string(),
            reversible: Some(true),
        });

        let authority = authority_from_manifest(&manifest);
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&authority, &mut encoded).unwrap();
        let decoded: LifecycleAuthorityV2 = ciborium::from_reader(encoded.as_slice()).unwrap();
        let mut execution_manifest = CcsManifest::new_minimal("lifecycle", "1.0.0");
        apply_authority_to_manifest(&decoded, &mut execution_manifest).unwrap();

        assert_eq!(authority_from_manifest(&execution_manifest), authority);
        assert_eq!(decoded.tmpfiles[0].mode, "~:0640");
        assert_eq!(decoded.tmpfiles[0].age, "am:30d");
        assert_eq!(decoded.tmpfiles[0].argument, "exact payload with spaces");
        assert_eq!(
            decoded.post_install.as_ref().unwrap().interpreter,
            "/bin/sh"
        );
        assert_eq!(
            decoded.post_install.as_ref().unwrap().execution,
            LifecycleScriptExecutionV2::SandboxedTargetRoot
        );
    }
}
