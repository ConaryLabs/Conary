// apps/conary/src/commands/install/native_events/tests/arch.rs

//! Arch lifecycle preparation and preflight coverage.

use super::*;

fn arch_hook_bundle_with_dependency(package_name: &str, dependency: &str) -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle(package_name, "1");
    bundle.source_format = SourceFormat::Arch;
    bundle.source_family = "arch".to_string();
    bundle.source_distro = Some("arch".to_string());
    bundle.source_release = None;
    bundle.source_arch = Some("x86_64".to_string());
    bundle.version_scheme = LifecycleVersionScheme::Arch;

    let entry = &mut bundle.entries[0];
    entry.id = "arch:alpm-hook:40-cache.hook".to_string();
    entry.native_slot = "alpm-hook:40-cache.hook".to_string();
    entry.kind = NativeLifecycleEntryKind::ControlArtifact;
    entry.phase = LifecyclePath::Trigger;
    entry.lifecycle_paths = vec![LifecyclePath::Trigger.as_str().to_string()];
    entry.interpreter = "package-manager-control-artifact".to_string();
    entry.rpm_runtime = None;
    entry.arch_hook = Some(ArchHookMetadata {
        hook_path: "/usr/share/libalpm/hooks/40-cache.hook".to_string(),
        triggers: vec![ArchHookTriggerMetadata {
            operations: vec![ArchHookOperation::Install],
            trigger_type: ArchHookTriggerType::Path,
            targets: vec!["usr/share/cache/*".to_string()],
        }],
        action: ArchHookActionMetadata {
            description: Some("refresh cache".to_string()),
            when: ArchHookWhen::PostTransaction,
            exec: "/usr/bin/cache-refresh".to_string(),
            argv: vec!["/usr/bin/cache-refresh".to_string()],
            depends: vec![dependency.to_string()],
            abort_on_fail: false,
            needs_targets: true,
        },
    });
    bundle
}

#[test]
fn arch_hook_depends_accepts_incoming_virtual_provide_from_native_install_input() {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    conary_core::ccs::HostCapabilityInventory::default()
        .persist(&conn)
        .unwrap();

    let mut hook_trove = Trove::new(
        "cache-hook-owner".to_string(),
        "1".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Arch,
    );
    hook_trove.architecture = Some("x86_64".to_string());
    let hook_trove_id = hook_trove.insert(&conn).unwrap();
    let hook_bundle =
        arch_hook_bundle_with_dependency("cache-hook-owner", "cache-provider>=2:1.0-1");
    InstalledNativeLifecycleBundle::new(hook_trove_id, None, &hook_bundle)
        .unwrap()
        .insert_or_replace(&conn)
        .unwrap();

    let relation_removals = Vec::new();
    let without_provide = PreparedNativeTransaction::prepare_batch(
        &conn,
        &[NativeInstallInput {
            package_name: "cache-data",
            package_version: "1",
            package_arch: Some("x86_64"),
            version_scheme: VersionScheme::Arch,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &relation_removals,
            relation_deconfigurations: &[],
            paths: vec!["usr/share/cache/index".to_string()],
        }],
    )
    .unwrap();
    assert!(
        without_provide
            .plan
            .events_at(NativeEventStage::ArchPostTransaction)
            .next()
            .is_none(),
        "hook dependency must not be inferred from the incoming payload"
    );

    let provides = [conary_core::packages::traits::Dependency {
        name: "cache-provider".to_string(),
        version: Some("2:1.0-2".to_string()),
        dep_type: conary_core::packages::traits::DependencyType::Runtime,
        description: None,
    }];
    let with_provide = PreparedNativeTransaction::prepare_batch(
        &conn,
        &[NativeInstallInput {
            package_name: "cache-data",
            package_version: "1",
            package_arch: Some("x86_64"),
            version_scheme: VersionScheme::Arch,
            provides: &provides,
            new_bundle: None,
            old_trove: None,
            relation_removals: &relation_removals,
            relation_deconfigurations: &[],
            paths: vec!["usr/share/cache/index".to_string()],
        }],
    )
    .unwrap();
    let event = with_provide
        .plan
        .events_at(NativeEventStage::ArchPostTransaction)
        .next()
        .expect("incoming virtual Provide must satisfy exact ALPM Depends");
    assert_eq!(event.owner_package, "cache-hook-owner");
    assert_eq!(event.matched_targets, vec!["usr/share/cache/index"]);
}

#[test]
fn arch_ldconfig_preflight_uses_exact_final_path_set() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("sbin")).unwrap();
    std::fs::write(
        root.path().join("sbin/ldconfig"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'ldconfig (GNU libc) 2.40'; fi\nexit 0\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(root.path().join("sbin/ldconfig"))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(root.path().join("sbin/ldconfig"), permissions).unwrap();
    assert!(!root.path().join("etc/ld.so.conf").exists());

    let prepared = PreparedNativeTransaction {
        arch_transaction: true,
        arch_ldconfig_required_after: true,
        host_capabilities: Some(conary_core::ccs::HostCapabilityInventory {
            ldconfig: Some(
                conary_core::ccs::ExecutableInterface::probe_ldconfig_in_root(
                    root.path(),
                    Path::new("/sbin/ldconfig"),
                )
                .unwrap(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(
        prepared.arch_ldconfig_argv(root.path()).unwrap(),
        Some(vec!["/sbin/ldconfig".to_string()])
    );
}
