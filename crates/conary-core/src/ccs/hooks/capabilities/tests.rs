// crates/conary-core/src/ccs/hooks/capabilities/tests.rs

use super::*;
use crate::ccs::manifest::{Service, ServiceAction};
use crate::ccs::native_lifecycle::SourceFormat;
use crate::test_support::{HostToolFixture, link_host_tool};
use std::fs;

#[cfg(unix)]
fn fake_interface(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    let fixture = match name {
        "systemctl" | "systemd-sysusers" | "systemd-tmpfiles" => HostToolFixture::Systemd,
        "rc-service" => HostToolFixture::OpenRc,
        "sysctl" => HostToolFixture::Sysctl406,
        "ldconfig" => HostToolFixture::Ldconfig,
        other => panic!("unknown fake interface {other}"),
    };
    link_host_tool(&path, fixture);
    path
}

#[cfg(unix)]
#[test]
fn discovery_records_exact_interfaces_without_a_distro_selector() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "systemctl",
        "systemd-sysusers",
        "systemd-tmpfiles",
        "sysctl",
        "ldconfig",
    ] {
        fake_interface(root.path(), name);
    }

    let inventory = HostCapabilityInventory::discover_in_paths([root.path().to_path_buf()]);

    assert!(inventory.systemd.is_some());
    assert!(inventory.sysusers.is_some());
    assert!(inventory.tmpfiles.is_some());
    assert!(inventory.sysctl.is_some());
    assert!(inventory.ldconfig.is_some());
    let encoded = serde_json::to_string(&inventory).unwrap();
    assert!(!encoded.contains("distro"));
    assert!(!encoded.contains("profile"));
    assert!(encoded.contains("\"tmpfiles\":{\"command\":{\"executable\":"));
}

#[cfg(unix)]
#[test]
fn active_manager_facts_select_openrc_and_prefer_running_systemd() {
    let root = tempfile::tempdir().unwrap();
    fake_interface(root.path(), "rc-service");

    let openrc = HostCapabilityInventory::discover_in_paths_with_runtime(
        [root.path().to_path_buf()],
        None,
        false,
        true,
    );
    assert_eq!(openrc.init_system, InitSystemCapability::OpenRc);
    assert!(openrc.openrc.is_some());
    openrc.validate().unwrap();

    fake_interface(root.path(), "systemctl");
    let systemd = HostCapabilityInventory::discover_in_paths_with_runtime(
        [root.path().to_path_buf()],
        None,
        true,
        true,
    );
    assert_eq!(systemd.init_system, InitSystemCapability::Systemd);
    assert!(systemd.openrc.is_some());
    systemd.validate().unwrap();
}

#[cfg(unix)]
#[test]
fn sysusers_path_requires_the_exact_persisted_interface() {
    let missing = HostCapabilityInventory::default()
        .sysusers_interface()
        .unwrap_err();
    assert!(matches!(
        missing,
        HostCapabilityPreflightError::MissingCapability {
            requirement: HostCapabilityRequirement::Sysusers,
            ..
        }
    ));

    let root = tempfile::tempdir().unwrap();
    let executable = fake_interface(root.path(), "systemd-sysusers");
    let inventory = HostCapabilityInventory {
        sysusers: Some(ExecutableInterface::probe_sysusers(executable.clone()).unwrap()),
        ..HostCapabilityInventory::default()
    };
    assert_eq!(
        inventory.sysusers_interface().unwrap().executable,
        executable
    );

    fs::remove_file(&executable).unwrap();
    link_host_tool(&executable, HostToolFixture::ExitSuccess);
    let drift = inventory.sysusers_interface().unwrap_err();
    assert!(matches!(
        drift,
        HostCapabilityPreflightError::InterfaceDrift {
            requirement: HostCapabilityRequirement::Sysusers,
            ..
        }
    ));
}

#[test]
fn inventory_persists_opaque_target_immutable_backing_security() {
    let capability = ImmutableBackingSecurity {
        mechanism: ImmutableBackingSecurityMechanism::Selinux,
        xattr_value: b"system_u:object_r:usr_t:s0\0".to_vec(),
    };
    let inventory =
        HostCapabilityInventory::discover_in_paths_with_security([], Some(capability.clone()));
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();

    inventory.persist(&conn).unwrap();
    let loaded = HostCapabilityInventory::load_required(&conn).unwrap();

    assert_eq!(loaded.immutable_backing_security, Some(capability));
    assert_eq!(
        loaded.schema_version,
        HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION
    );
}

#[test]
fn removed_flat_tmpfiles_interface_shape_is_not_accepted() {
    let removed_shape = r#"{
            "schema_version": 1,
            "init_system": "unsupported",
            "tmpfiles": {"executable": "/usr/bin/systemd-tmpfiles"}
        }"#;

    assert!(serde_json::from_str::<HostCapabilityInventory>(removed_shape).is_err());
}

#[cfg(unix)]
#[test]
fn descriptor_contract_must_match_its_inventory_field() {
    let root = tempfile::tempdir().unwrap();
    let tmpfiles = fake_interface(root.path(), "systemd-tmpfiles");
    let inventory = HostCapabilityInventory {
        sysctl: Some(ExecutableInterface::probe_tmpfiles(tmpfiles).unwrap()),
        ..HostCapabilityInventory::default()
    };

    assert!(matches!(
        inventory.validate(),
        Err(HostCapabilityInventoryError::WrongContract {
            interface: "sysctl",
            found: HostExecutableContract::SystemdTmpfiles,
            expected: HostExecutableContract::ProcpsSysctl,
        })
    ));
}

#[cfg(unix)]
#[test]
fn init_system_and_systemd_operations_are_one_exact_shape() {
    let root = tempfile::tempdir().unwrap();
    let systemctl = fake_interface(root.path(), "systemctl");
    let inventory = HostCapabilityInventory {
        init_system: InitSystemCapability::Systemd,
        systemd: Some(SystemdInterface::probe(systemctl, false).unwrap()),
        ..HostCapabilityInventory::default()
    };

    assert!(matches!(
        inventory.validate(),
        Err(HostCapabilityInventoryError::InvalidSystemdShape)
    ));
}

#[cfg(unix)]
#[test]
fn source_format_and_host_capability_profiles_are_orthogonal_axes() {
    let root = tempfile::tempdir().unwrap();
    let systemctl = fake_interface(root.path(), "systemctl");
    let systemd_profile = HostCapabilityInventory {
        init_system: InitSystemCapability::Systemd,
        systemd: Some(SystemdInterface::probe(systemctl, false).unwrap()),
        ..HostCapabilityInventory::default()
    };
    let mut hooks = Hooks::default();
    hooks.services.push(Service {
        name: "portable.service".to_string(),
        action: ServiceAction::Enable,
        reversible: Some(true),
    });

    let profiles = [
        ("systemd", systemd_profile, true),
        ("unsupported", HostCapabilityInventory::default(), false),
    ];
    for source_format in [SourceFormat::Rpm, SourceFormat::Deb, SourceFormat::Arch] {
        for (profile_name, profile, expected) in &profiles {
            assert_eq!(
                profile.preflight_hooks(root.path(), &hooks).is_ok(),
                *expected,
                "{} source should resolve solely against the {profile_name} capability profile",
                source_format.as_str()
            );
        }
    }
}

#[test]
fn artix_named_source_does_not_imply_a_systemd_target() {
    let root = tempfile::tempdir().unwrap();
    // Host capability inventory has no source-identity input. An Artix
    // package therefore receives this exact target result rather than an
    // Arch-profile-derived systemd assumption.
    let inventory = HostCapabilityInventory::default();
    let mut hooks = Hooks::default();
    hooks.services.push(Service {
        name: "portable.service".to_string(),
        action: ServiceAction::Enable,
        reversible: Some(true),
    });

    assert_eq!(inventory.init_system, InitSystemCapability::Unsupported);
    assert!(inventory.systemd.is_none());
    assert!(matches!(
        inventory.preflight_hooks(root.path(), &hooks),
        Err(HostCapabilityPreflightError::MissingCapability {
            requirement: HostCapabilityRequirement::ServiceManager,
            hook: "hooks.services"
        })
    ));
}

#[cfg(unix)]
#[test]
fn runtime_service_action_preflights_for_deferred_generation_activation() {
    let root = tempfile::tempdir().unwrap();
    let systemctl = fake_interface(root.path(), "systemctl");
    let inventory = HostCapabilityInventory {
        init_system: InitSystemCapability::Systemd,
        systemd: Some(SystemdInterface::probe(systemctl, true).unwrap()),
        ..HostCapabilityInventory::default()
    };
    let mut hooks = Hooks::default();
    hooks.services.push(Service {
        name: "portable.service".to_string(),
        action: ServiceAction::Restart,
        reversible: Some(false),
    });

    inventory.preflight_hooks(root.path(), &hooks).unwrap();
}

#[cfg(unix)]
#[test]
fn arch_ldconfig_requires_target_config_and_executable_adapter() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc")).unwrap();
    fs::create_dir_all(root.path().join("sbin")).unwrap();
    link_host_tool(
        &root.path().join("sbin/ldconfig"),
        HostToolFixture::Ldconfig,
    );
    let inventory = HostCapabilityInventory {
        ldconfig: Some(
            ExecutableInterface::probe_ldconfig_in_root(root.path(), Path::new("/sbin/ldconfig"))
                .unwrap(),
        ),
        ..HostCapabilityInventory::default()
    };

    assert_eq!(inventory.arch_ldconfig_path(root.path()), None);
    fs::write(root.path().join("etc/ld.so.conf"), "").unwrap();
    assert_eq!(
        inventory.arch_ldconfig_path(root.path()),
        Some(Path::new("/sbin/ldconfig"))
    );
}

#[test]
fn retired_and_unknown_inventory_versions_require_clean_reinitialization() {
    for found in [3, 99] {
        let inventory = HostCapabilityInventory {
            schema_version: found,
            ..HostCapabilityInventory::default()
        };
        assert!(matches!(
            inventory.validate(),
            Err(HostCapabilityInventoryError::UnsupportedSchema {
                found: actual,
                expected: HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION
            }) if actual == found
        ));
    }
}
