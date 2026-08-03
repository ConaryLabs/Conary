// conary-core/src/ccs/v3/authoring/tests.rs

use super::*;
use crate::ccs::builder::test_support;
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use std::io::Read;

fn config_file(
    path: &str,
    noreplace: bool,
) -> crate::packages::config_authority::SourceConfigDeclaration {
    crate::packages::config_authority::SourceConfigDeclaration::Ccs(
        crate::packages::config_authority::CcsConfigDeclaration {
            path: path.to_string(),
            noreplace,
            payload: crate::packages::config_authority::ConfigPayloadAssociation::Matched,
        },
    )
}

#[test]
fn projection_requires_release_for_v3_package_authoring() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.package.release.clear();

    let error = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap_err();

    assert!(
        error.to_string().contains("release"),
        "expected release diagnostic, got {error}"
    );
}

#[test]
fn projection_builds_complete_local_dev_package_authority() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    assert_eq!(projected.authority.identity.name, "hello");
    assert_eq!(projected.authority.identity.release, "1");
    assert_eq!(
        projected.authority.identity.architecture.as_deref(),
        Some(crate::ccs::manifest::DEFAULT_CONARY_ARCHITECTURE)
    );
    assert_eq!(
        projected.authority.provenance.hardening_level.as_deref(),
        Some("host")
    );
    assert!(projected.authority.components["runtime"].default);
    assert!(
        projected
            .payloads
            .iter()
            .any(|payload| payload.path == "/hello")
    );
}

#[test]
fn projection_rejects_missing_architecture_authority() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.package.platform = None;

    let error = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("package identity architecture is required"),
        "expected architecture diagnostic, got {error}"
    );
}

#[test]
fn projection_preserves_exact_package_capability_authority() {
    let mut build = test_support::minimal_file_build_result("capable", "0.1.0", b"hello\n");
    let declaration = crate::capability::CapabilityDeclaration {
        rationale: Some("needs outbound repository access".to_string()),
        network: crate::capability::NetworkCapabilities {
            connect_tcp: vec![443],
            ..Default::default()
        },
        ..Default::default()
    };
    build.manifest.capabilities = Some(declaration.clone());

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap();

    assert_eq!(
        projected.authority.execution_capabilities,
        Some(declaration)
    );
}

#[test]
fn projection_signs_canonical_file_capability_authority() {
    let mut build = test_support::single_file_build_result_at(
        "file-capable",
        "0.1.0",
        "/usr/bin/file-capable",
        b"#!/bin/sh\n",
    );
    build.manifest.file_capabilities = vec![crate::ccs::manifest::FileCapability {
        path: "/usr/bin/file-capable".to_string(),
        capabilities: vec![
            "cap_net_raw".to_string(),
            "cap_net_bind_service".to_string(),
        ],
        permitted: true,
        effective: true,
        inheritable: false,
    }];

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap();

    assert_eq!(projected.authority.file_capabilities.len(), 1);
    assert_eq!(
        projected.authority.file_capabilities[0].capabilities,
        ["cap_net_bind_service", "cap_net_raw"]
    );
}

#[test]
fn projection_rejects_file_capability_without_exact_regular_build_output() {
    let mut build = test_support::single_file_build_result_at(
        "file-capable",
        "0.1.0",
        "/usr/bin/file-capable",
        b"#!/bin/sh\n",
    );
    build.manifest.file_capabilities = vec![crate::ccs::manifest::FileCapability {
        path: "/usr/bin/missing".to_string(),
        capabilities: vec!["cap_net_bind_service".to_string()],
        permitted: true,
        effective: true,
        inheritable: false,
    }];
    let error = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("absent from signed build output")
    );

    build.manifest.file_capabilities[0].path = "/usr/bin/file-capable".to_string();
    let mut symlink_node = crate::payload::PayloadNode::regular(0o777);
    symlink_node.kind = crate::payload::PayloadNodeKind::Symlink {
        target: "file-capable-real".to_string(),
    };
    symlink_node.mode = libc::S_IFLNK | 0o777;
    build.files[0].node = symlink_node.clone();
    build.files[0].content = None;
    build.components.get_mut("runtime").unwrap().files[0].node = symlink_node.clone();
    build.components.get_mut("runtime").unwrap().files[0].content = None;
    build.components.get_mut("runtime").unwrap().size = 0;
    build.payloads[0] = crate::packages::payload::PackagePayloadFile::new(
        "/usr/bin/file-capable".to_string(),
        symlink_node,
        None,
        None,
    )
    .unwrap();
    build.total_size = 0;
    let error = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not a regular signed build output file")
    );
}

#[test]
fn projection_gives_payloadless_packages_an_explicit_empty_default_component() {
    let mut build = test_support::minimal_file_build_result("empty", "0.1.0", b"discarded");
    build.files.clear();
    build.components.clear();
    build.payloads.clear();
    build.total_size = 0;

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap();

    let runtime = &projected.authority.components["runtime"];
    assert!(runtime.default);
    assert_eq!(runtime.file_count, 0);
    assert_eq!(runtime.total_size, 0);
    assert!(projected.payloads.is_empty());
}

#[test]
fn projection_rejects_ambiguous_or_missing_default_component_authority() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.components.default = vec!["runtime".to_string(), "lib".to_string()];
    let ambiguous = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap_err();
    assert!(ambiguous.to_string().contains("exactly one"));

    build.manifest.components.default = vec!["lib".to_string()];
    let missing = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: None,
    })
    .unwrap_err();
    assert!(missing.to_string().contains("not present"));
}

#[test]
fn projection_preserves_reopenable_payload_when_chunk_metadata_is_present() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;

    let chunks = [b"hel".to_vec(), b"lo\n".to_vec()];
    let chunk_hashes = chunks
        .iter()
        .map(|chunk| crate::hash::sha256(chunk))
        .collect::<Vec<_>>();
    build.files[0].chunks = Some(chunk_hashes.clone());
    build.components.get_mut("runtime").unwrap().files[0].chunks = Some(chunk_hashes);
    build.chunked = true;

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    let mut bytes = Vec::new();
    projected.payloads[0]
        .open_content()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"hello\n");
}

#[test]
fn projection_keeps_host_hardening_for_release_key_signing_path() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: false,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    assert_eq!(
        projected.authority.provenance.hardening_level.as_deref(),
        Some("host")
    );
}

#[test]
fn projection_marks_noreplace_config_files() {
    let mut build = test_support::single_file_build_result_at(
        "demo",
        "0.1.0",
        "/etc/conary-example/config.toml",
        b"message = \"hello\"\n",
    );
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    build.manifest.config.files = vec![config_file("/etc/conary-example/config.toml", true)];

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    let package = match &projected.authority.kind {
        PackageKindV3::Package(package) => package,
        other => panic!("expected package authority, got {other:?}"),
    };
    let expected = ConfigSemanticsV3 {
        noreplace: true,
        ghost: false,
        remove_on_upgrade: false,
    };
    assert_eq!(package.files[0].config, Some(expected));
    assert_eq!(package.config[0].path(), "/etc/conary-example/config.toml");
    assert_eq!(package.config[0].noreplace(), expected.noreplace);
}

#[test]
fn projection_marks_replace_config_when_noreplace_is_false() {
    let mut build = test_support::single_file_build_result_at(
        "demo",
        "0.1.0",
        "/etc/conary-example/config.toml",
        b"message = \"hello\"\n",
    );
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    build.manifest.config.files = vec![config_file("/etc/conary-example/config.toml", false)];

    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    let package = match &projected.authority.kind {
        PackageKindV3::Package(package) => package,
        other => panic!("expected package authority, got {other:?}"),
    };
    let expected = ConfigSemanticsV3 {
        noreplace: false,
        ghost: false,
        remove_on_upgrade: false,
    };
    assert_eq!(package.files[0].config, Some(expected));
    assert_eq!(package.config[0].noreplace(), expected.noreplace);
}

#[test]
fn projection_rejects_config_path_absent_from_payload() {
    let mut build = test_support::minimal_file_build_result("demo", "0.1.0", b"hello\n");
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    build.manifest.config.files = vec![config_file("/etc/conary-example/config.toml", true)];

    let error = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap_err();

    assert!(error.to_string().contains("config path"));
}

#[test]
fn projection_rejects_relative_config_paths() {
    let mut build = test_support::single_file_build_result_at(
        "demo",
        "0.1.0",
        "etc/conary-example/config.toml",
        b"message = \"hello\"\n",
    );
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    build.manifest.config.files = vec![config_file("etc/conary-example/config.toml", true)];

    let error = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap_err();

    assert!(error.to_string().contains("config path"));
    assert!(error.to_string().contains("absolute"));
}

#[test]
fn lint_manifest_reports_empty_release() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
    manifest.package.release.clear();
    let findings = lint_manifest_for_v3_authoring(&manifest);

    assert!(findings.iter().any(|f| f.field == Some("package.release")));
    assert!(findings.iter().all(|f| f.blocks_build));
}

#[test]
fn lint_manifest_rejects_missing_or_empty_architecture_authority() {
    for platform in [
        None,
        Some(crate::ccs::manifest::Platform {
            arch: Some("  ".to_string()),
            ..Default::default()
        }),
    ] {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.package.platform = platform;

        let findings = lint_manifest_for_v3_authoring(&manifest);
        assert!(findings.iter().any(|finding| {
            finding.code == "m4b-missing-architecture"
                && finding.field == Some("package.platform.arch")
                && finding.blocks_build
        }));
    }
}

#[test]
fn lint_manifest_rejects_ambiguous_or_empty_default_component_authority() {
    for defaults in [
        Vec::new(),
        vec!["runtime".to_string(), "doc".to_string()],
        vec![" ".to_string()],
    ] {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.components.default = defaults;

        let findings = lint_manifest_for_v3_authoring(&manifest);
        assert!(findings.iter().any(|finding| {
            finding.code == "m4b-default-component"
                && finding.field == Some("components.default")
                && finding.blocks_build
        }));
    }
}

#[test]
fn lint_manifest_allows_declarative_lifecycle_without_distro_gate() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
    manifest.package.release = "1".to_string();
    manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    manifest.hooks.services.push(crate::ccs::manifest::Service {
        name: "hello.service".to_string(),
        action: crate::ccs::manifest::ServiceAction::Restart,
        reversible: None,
    });

    let findings = lint_manifest_for_v3_authoring(&manifest);
    assert!(findings.iter().all(|finding| !finding.blocks_build));
}

#[test]
fn lint_manifest_allows_script_lifecycle_with_signed_execution_contract() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
    manifest.package.release = "1".to_string();
    manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    manifest.hooks.post_install = Some(crate::ccs::manifest::ScriptHook {
        script: "echo configured".to_string(),
        reversible: None,
    });

    let findings = lint_manifest_for_v3_authoring(&manifest);
    assert!(findings.iter().all(|finding| !finding.blocks_build));
}

#[test]
fn projection_maps_declarative_lifecycle_hooks_to_signed_authority() {
    let mut build = test_support::single_file_build_result_at(
        "conary-example",
        "0.1.0",
        "/usr/bin/conary-example",
        b"#!/bin/sh\necho conary-example\n",
    );
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    build
        .manifest
        .hooks
        .services
        .push(crate::ccs::manifest::Service {
            name: "conary-example.service".to_string(),
            action: crate::ccs::manifest::ServiceAction::Restart,
            reversible: None,
        });
    build
        .manifest
        .hooks
        .systemd
        .push(crate::ccs::manifest::SystemdHook {
            unit: "conary-example.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
    build
        .manifest
        .hooks
        .sysctl
        .push(crate::ccs::manifest::SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "1".to_string(),
            reversible: Some(false),
        });
    build
        .manifest
        .hooks
        .alternatives
        .push(crate::ccs::manifest::AlternativeHook {
            link: "/usr/bin/conary-example".to_string(),
            name: "conary-example".to_string(),
            path: "/usr/bin/conary-example".to_string(),
            priority: 70,
            reversible: Some(true),
        });
    build.manifest.scriptlets.capabilities.push(
        crate::ccs::manifest::ScriptletCapabilityDeclaration {
            name: "systemd-service-registration".to_string(),
            paths: vec!["/etc/systemd/system".to_string()],
        },
    );
    build.manifest.hooks.post_install = Some(crate::ccs::manifest::ScriptHook {
        script: "printf configured".to_string(),
        reversible: Some(false),
    });
    build
        .manifest
        .hooks
        .tmpfiles
        .push(crate::ccs::manifest::TmpfilesHook {
            entry_type: "L+!$".to_string(),
            path: "/var/lib/conary-example".to_string(),
            mode: "-".to_string(),
            user: "-".to_string(),
            group: "-".to_string(),
            age: "30d".to_string(),
            argument: "/srv/example target".to_string(),
            reversible: None,
        });
    build
        .manifest
        .hooks
        .users
        .push(crate::ccs::manifest::UserHook {
            name: "conary-example".to_string(),
            system: true,
            home: None,
            shell: None,
            group: Some("conary-example".to_string()),
            reversible: None,
        });
    build
        .manifest
        .hooks
        .groups
        .push(crate::ccs::manifest::GroupHook {
            name: "conary-example".to_string(),
            system: true,
            reversible: None,
        });
    build
        .manifest
        .hooks
        .directories
        .push(crate::ccs::manifest::DirectoryHook {
            path: "/var/lib/conary-example".to_string(),
            mode: "0755".to_string(),
            owner: "conary-example".to_string(),
            group: "conary-example".to_string(),
            cleanup: None,
            reversible: None,
        });
    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    assert_eq!(
        projected.authority.lifecycle.services[0].action,
        LifecycleServiceActionV3::Restart
    );
    let tmpfiles = &projected.authority.lifecycle.tmpfiles[0];
    assert_eq!(tmpfiles.entry_type, "L+!$");
    assert_eq!(tmpfiles.mode, "-");
    assert_eq!(tmpfiles.user, "-");
    assert_eq!(tmpfiles.group, "-");
    assert_eq!(tmpfiles.age, "30d");
    assert_eq!(tmpfiles.argument, "/srv/example target");
    assert!(projected.authority.lifecycle.users[0].system);
    assert_eq!(
        projected.authority.lifecycle.users[0].group.as_deref(),
        Some("conary-example")
    );
    assert!(projected.authority.lifecycle.groups[0].system);
    assert_eq!(
        projected.authority.lifecycle.directories[0].owner,
        "conary-example"
    );
    assert!(projected.authority.lifecycle.systemd[0].enable);
    assert_eq!(projected.authority.lifecycle.sysctl[0].value, "1");
    assert_eq!(
        projected.authority.lifecycle.alternatives[0].link,
        "/usr/bin/conary-example"
    );
    assert_eq!(projected.authority.lifecycle.alternatives[0].priority, 70);
    let script = projected.authority.lifecycle.post_install.as_ref().unwrap();
    assert_eq!(script.interpreter, "/bin/sh");
    assert_eq!(script.body, "printf configured");
    assert_eq!(
        script.execution,
        LifecycleScriptExecutionV3::SandboxedTargetRoot
    );
    assert_eq!(script.capabilities[0].paths, vec!["/etc/systemd/system"]);
    assert_eq!(
        projected.authority.lifecycle.script_capabilities,
        script.capabilities
    );
}

#[test]
fn projection_carries_typed_requires_and_provides_without_distro_gates() {
    let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
    build.manifest.package.release = "1".to_string();
    build.manifest.package.kind = crate::ccs::v3::PackageKindTagV3::Package;
    build.manifest.requirements.extend([
        RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause {
                name: "openssl".to_string(),
                capability_kind: Some(RepositoryCapabilityKind::PackageName),
                version_constraint: Some(">=3.0.0".to_string()),
                architecture_qualifier: Default::default(),
                native_text: None,
            },
        ),
        RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause {
                name: "tls".to_string(),
                capability_kind: Some(RepositoryCapabilityKind::Virtual),
                version_constraint: Some(">=1.3.0".to_string()),
                architecture_qualifier: Default::default(),
                native_text: None,
            },
        ),
    ]);
    build
        .manifest
        .provides
        .capabilities
        .push("web-server".to_string());
    build
        .manifest
        .provides
        .sonames
        .push("libhello.so.1".to_string());
    build.manifest.provides.binaries.push("hello".to_string());
    build.manifest.provides.pkgconfig.push("hello".to_string());

    assert!(
        lint_manifest_for_v3_authoring(&build.manifest)
            .iter()
            .all(|finding| !finding.blocks_build)
    );
    let projected = project_build_result_to_v3(V3AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    assert!(projected.authority.requirements.iter().any(|group| {
        group.alternatives.iter().any(|entry| {
            entry.capability_kind == Some(RepositoryCapabilityKind::PackageName)
                && entry.name == "openssl"
                && entry.version_constraint.as_deref() == Some(">=3.0.0")
        })
    }));
    assert!(projected.authority.requirements.iter().any(|group| {
        group.alternatives.iter().any(|entry| {
            entry.capability_kind == Some(RepositoryCapabilityKind::Virtual)
                && entry.name == "tls"
                && entry.version_constraint.as_deref() == Some(">=1.3.0")
        })
    }));
    for (kind, name) in [
        (DependencyKindV3::Capability, "web-server"),
        (DependencyKindV3::Soname, "libhello.so.1"),
        (DependencyKindV3::Binary, "hello"),
        (DependencyKindV3::PkgConfig, "hello"),
    ] {
        assert!(
            projected
                .authority
                .provided_capabilities
                .iter()
                .any(|entry| entry.kind == kind && entry.name == name)
        );
    }
}
