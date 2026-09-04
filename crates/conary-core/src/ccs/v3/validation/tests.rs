// crates/conary-core/src/ccs/v3/validation/tests.rs

use super::*;
use crate::ccs::v3::diagnostics::V3DiagnosticCode;
use crate::ccs::v3::schema::{
    AuthorityDocumentV3, DependencyKindV3, GroupDataV3, PackageKindTagV3, PackageKindV3,
    ProvidedCapabilityV3, RedirectDataV3,
};
use crate::repository::dependency_model::{ProvideArchitectureQualifier, ProvideVersionRelation};
use crate::repository::versioning::VersionScheme;

#[test]
fn accepts_payloadless_package_with_explicit_empty_component_authority() {
    let mut authority = AuthorityDocumentV3::empty_package_for_tests("empty-package");
    authority.components.insert(
        "runtime".to_string(),
        crate::ccs::v3::schema::ComponentAuthorityV3 {
            name: "runtime".to_string(),
            default: true,
            file_count: 0,
            total_size: 0,
        },
    );
    validate_authority(&authority).unwrap();
}

#[test]
fn rejects_payloadless_package_without_default_component_authority() {
    let authority = AuthorityDocumentV3::empty_package_for_tests("empty-package");
    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == V3DiagnosticCode::ComponentAuthorityMismatch })
    );
}

#[test]
fn rejects_invalid_package_capability_authority() {
    let mut authority = AuthorityDocumentV3::package_for_tests("invalid-capabilities");
    authority.execution_capabilities = Some(crate::capability::CapabilityDeclaration {
        version: crate::capability::CAPABILITY_SCHEMA_VERSION + 1,
        ..Default::default()
    });

    let error = validate_authority(&authority).unwrap_err();

    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == V3DiagnosticCode::KindContractViolation
            && diagnostic.field.as_deref() == Some("capabilities")
    }));
}

#[test]
fn same_name_source_capability_may_use_a_compatibility_version() {
    let mut authority = AuthorityDocumentV3::package_for_tests("aspnet-runtime");
    authority.identity.version = "10.0.10.sdk110-1".to_string();
    authority.identity.version_scheme = VersionScheme::Arch;
    authority.provided_capabilities = vec![ProvidedCapabilityV3 {
        kind: DependencyKindV3::Package,
        name: "aspnet-runtime".to_string(),
        provider_version: Some("10.0".to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Arch,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
            format: crate::repository::dependency_model::SourcePackageFormat::Alpm,
            record_index: 0,
        },
        target: None,
        component: None,
    }];

    validate_authority(&authority).unwrap();
}

#[test]
fn signed_authority_accepts_a_promised_path_the_package_does_not_ship() {
    let mut authority = AuthorityDocumentV3::package_for_tests("crypto-policies");
    authority.identity.version_scheme = VersionScheme::Rpm;
    authority.identity.version = "20251128-3".to_string();
    authority.provided_capabilities = vec![promised_capability_v3("/etc/example/back-end.config")];

    validate_authority(&authority).unwrap();
}

#[test]
fn signed_authority_refuses_a_promised_path_that_is_also_payload_content() {
    let mut authority = AuthorityDocumentV3::package_for_tests("contradiction");
    authority.identity.version_scheme = VersionScheme::Rpm;
    authority.identity.version = "1-1".to_string();
    // package_for_tests signs /usr/bin/hello as payload content.
    authority.provided_capabilities = vec![promised_capability_v3("/usr/bin/hello")];

    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("is also signed payload content")
        }),
        "{:?}",
        error.diagnostics
    );
}

fn promised_capability_v3(path: &str) -> ProvidedCapabilityV3 {
    ProvidedCapabilityV3 {
        kind: DependencyKindV3::File,
        name: path.to_string(),
        provider_version: None,
        version_relation: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: crate::repository::dependency_model::CapabilityProvenance::SourcePromisedPath {
            format: crate::repository::dependency_model::SourcePackageFormat::Rpm,
        },
        target: None,
        component: None,
    }
}

#[test]
fn capability_list_rejects_a_second_exact_identity_origin() {
    let mut authority = AuthorityDocumentV3::package_for_tests("identity-spoof");
    authority.provided_capabilities = vec![ProvidedCapabilityV3 {
        kind: DependencyKindV3::Package,
        name: "other-package".to_string(),
        provider_version: Some("1".to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Conary,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: crate::repository::dependency_model::CapabilityProvenance::ExactIdentity,
        target: None,
        component: None,
    }];

    let error = validate_authority(&authority).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("cannot appear in the capability list")
    }));
}

#[test]
fn rejects_group_without_members() {
    let mut authority = AuthorityDocumentV3::empty_package_for_tests("empty-group");
    authority.identity.kind = PackageKindTagV3::Group;
    authority.kind = PackageKindV3::Group(GroupDataV3 {
        members: Vec::new(),
        provides: Vec::new(),
        conflicts: Vec::new(),
        policy: Default::default(),
    });
    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code == V3DiagnosticCode::KindContractViolation)
    );
}

#[test]
fn accepts_redirect_with_target() {
    let mut authority = AuthorityDocumentV3::empty_package_for_tests("old-name");
    authority.identity.kind = PackageKindTagV3::Redirect;
    authority.kind = PackageKindV3::Redirect(RedirectDataV3 {
        to: "new-name".to_string(),
        version_constraint: None,
        reason: Some("renamed".to_string()),
    });
    validate_authority(&authority).unwrap();
}

#[test]
fn rejects_kind_tag_payload_mismatch() {
    let mut authority = AuthorityDocumentV3::package_for_tests("mismatch");
    authority.identity.kind = PackageKindTagV3::Group;
    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code == V3DiagnosticCode::KindContractViolation)
    );
}

#[test]
fn requirement_groups_need_valid_atoms() {
    let mut authority = AuthorityDocumentV3::package_for_tests("bad-dep");
    authority.requirements.push(
        crate::repository::dependency_model::RepositoryRequirementGroup::simple(
            crate::repository::dependency_model::RepositoryRequirementKind::Depends,
            crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                String::new(),
            ),
        ),
    );
    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.field.as_deref() == Some("requirements[0]"))
    );
}

#[test]
fn rejects_incomplete_identity_provenance_and_component_totals() {
    let mut authority = AuthorityDocumentV3::package_for_tests("bad-authority");
    authority.identity.release.clear();
    authority.provenance.origin_class = None;
    authority.provenance.hardening_level = None;
    authority.components.get_mut("main").unwrap().file_count = 2;
    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.field.as_deref() == Some("identity.release"))
    );
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.field.as_deref() == Some("provenance.origin_class"))
    );
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code == V3DiagnosticCode::ComponentAuthorityMismatch)
    );
}

#[test]
fn rejects_missing_or_empty_identity_architecture() {
    for architecture in [None, Some("  ".to_string())] {
        let mut authority = AuthorityDocumentV3::package_for_tests("bad-architecture");
        authority.identity.architecture = architecture;

        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == V3DiagnosticCode::MissingAuthority
                && diagnostic.field.as_deref() == Some("identity.architecture")
        }));
    }
}

#[test]
fn rejects_symlink_with_invalid_signed_target() {
    let mut authority = AuthorityDocumentV3::package_for_tests("bad-link");
    let PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture should be package");
    };
    data.files[0].node.kind = crate::payload::PayloadNodeKind::Symlink {
        target: String::new(),
    };
    data.files[0].node.mode = libc::S_IFLNK | 0o777;
    data.files[0].content = None;
    let error = validate_authority(&authority).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.field.as_deref() == Some("kind.package.files.node"))
    );
}

#[test]
fn rejects_unimplemented_signed_file_conflict_replacement() {
    let mut authority = AuthorityDocumentV3::package_for_tests("replace-conflict");
    let PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture should be package");
    };
    data.files[0].conflict = ConflictPolicyV3::Replace;

    let error = validate_authority(&authority).unwrap_err();

    assert!(
        error.diagnostics.iter().any(|diagnostic| {
            diagnostic.field.as_deref() == Some("kind.package.files.conflict")
        })
    );
}

#[test]
fn rejects_unimplemented_signed_host_mutation_policy() {
    let mut authority = AuthorityDocumentV3::package_for_tests("host-mutation");
    let PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture should be package");
    };
    data.policy.allow_host_mutation = true;

    let error = validate_authority(&authority).unwrap_err();

    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.field.as_deref() == Some("kind.package.policy.allow_host_mutation")
    }));
}

#[test]
fn config_package_and_file_authority_must_match_exactly() {
    let mut authority = AuthorityDocumentV3::package_for_tests("config-mirror");
    let PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture should be package");
    };
    let semantics = ConfigSemanticsV3 {
        noreplace: true,
        ghost: false,
        remove_on_upgrade: false,
    };
    data.files[0].config = Some(semantics);
    data.config.push(
        crate::packages::config_authority::SourceConfigDeclaration::Ccs(
            crate::packages::config_authority::CcsConfigDeclaration {
                path: data.files[0].path.clone(),
                noreplace: true,
                payload: crate::packages::config_authority::ConfigPayloadAssociation::Matched,
            },
        ),
    );
    validate_authority(&authority).unwrap();

    let PackageKindV3::Package(data) = &mut authority.kind else {
        unreachable!();
    };
    data.files[0].config.as_mut().unwrap().noreplace = false;
    let error = validate_authority(&authority).unwrap_err();

    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.field.as_deref() == Some("kind.package.files.config") })
    );
}

#[test]
fn rejects_noncanonical_signed_config_path() {
    let mut authority = AuthorityDocumentV3::package_for_tests("config-path");
    let PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture should be package");
    };
    let semantics = ConfigSemanticsV3 {
        noreplace: true,
        ghost: false,
        remove_on_upgrade: false,
    };
    data.files[0].path = "/usr//bin/hello".to_string();
    data.files[0].config = Some(semantics);
    data.config.push(
        crate::packages::config_authority::SourceConfigDeclaration::Ccs(
            crate::packages::config_authority::CcsConfigDeclaration {
                path: data.files[0].path.clone(),
                noreplace: true,
                payload: crate::packages::config_authority::ConfigPayloadAssociation::Matched,
            },
        ),
    );

    let error = validate_authority(&authority).unwrap_err();

    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.field.as_deref() == Some("kind.package.config.path") })
    );
}

#[test]
fn lifecycle_authority_is_source_independent_structural_data() {
    let mut authority = AuthorityDocumentV3::package_for_tests("lifecycle");
    authority.lifecycle.services = vec![LifecycleServiceV3 {
        name: "example".to_string(),
        action: LifecycleServiceActionV3::Restart,
        reversible: Some(false),
    }];
    authority.lifecycle.systemd = vec![LifecycleSystemdV3 {
        unit: "example.service".to_string(),
        enable: true,
        reversible: Some(true),
    }];
    authority.lifecycle.tmpfiles = vec![LifecycleTmpfilesV3 {
        entry_type: "C+!$".to_string(),
        path: "/var/lib/example".to_string(),
        mode: "-".to_string(),
        user: "-".to_string(),
        group: "example-group".to_string(),
        age: "~30d".to_string(),
        argument: "/usr/share/example seed".to_string(),
        reversible: Some(true),
    }];
    authority.lifecycle.sysctl = vec![LifecycleSysctlV3 {
        key: "net.ipv4.ip_forward".to_string(),
        value: "1".to_string(),
        reversible: Some(false),
    }];
    authority.lifecycle.users = vec![LifecycleUserV3 {
        name: "example-user".to_string(),
        system: true,
        home: Some("/var/lib/example".to_string()),
        shell: Some("/usr/sbin/nologin".to_string()),
        group: Some("example-group".to_string()),
        reversible: Some(true),
    }];
    authority.lifecycle.groups = vec![LifecycleGroupV3 {
        name: "example-group".to_string(),
        system: true,
        reversible: Some(true),
    }];
    authority.lifecycle.directories = vec![LifecycleDirectoryV3 {
        path: "/var/lib/example".to_string(),
        mode: "0750".to_string(),
        owner: "example-user".to_string(),
        group: "example-group".to_string(),
        cleanup: Some("30d".to_string()),
        reversible: Some(true),
    }];
    authority.lifecycle.alternatives = vec![LifecycleAlternativeV3 {
        link: "/usr/bin/example".to_string(),
        name: "example".to_string(),
        path: "/usr/bin/example-v3".to_string(),
        priority: 70,
        reversible: Some(true),
    }];
    authority.lifecycle.script_capabilities = vec![LifecycleScriptCapabilityV3 {
        name: "systemd-service-registration".to_string(),
        paths: vec!["/etc/systemd/system".to_string()],
    }];
    authority.lifecycle.post_install = Some(LifecycleScriptV3 {
        interpreter: "/bin/sh".to_string(),
        body: "printf installed".to_string(),
        capabilities: authority.lifecycle.script_capabilities.clone(),
        reversible: Some(false),
        execution: LifecycleScriptExecutionV3::SandboxedTargetRoot,
    });

    validate_authority(&authority).unwrap();
}

#[test]
fn rejects_script_contract_the_hook_executor_cannot_honor() {
    let mut authority = AuthorityDocumentV3::package_for_tests("lifecycle");
    authority.lifecycle.post_install = Some(LifecycleScriptV3 {
        interpreter: "/usr/bin/python3".to_string(),
        body: "print('installed')".to_string(),
        capabilities: Vec::new(),
        reversible: None,
        execution: LifecycleScriptExecutionV3::SandboxedTargetRoot,
    });

    let error = validate_authority(&authority).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.field.as_deref() == Some("lifecycle.post_install.interpreter")
    }));
}
