// conary-core/src/repository/dependency_model/tests.rs

use super::*;
use std::collections::BTreeSet;

#[test]
fn dependency_flavor_maps_to_version_scheme() {
    assert_eq!(
        RepositoryDependencyFlavor::Rpm.version_scheme(),
        VersionScheme::Rpm
    );
    assert_eq!(
        RepositoryDependencyFlavor::Deb.version_scheme(),
        VersionScheme::Debian
    );
    assert_eq!(
        RepositoryDependencyFlavor::Arch.version_scheme(),
        VersionScheme::Arch
    );
}

#[test]
fn simple_requirement_group_is_simple() {
    let group = RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        RepositoryRequirementClause::name_only("glibc".to_string()),
    );
    assert!(group.is_simple());
    assert_eq!(group.behavior, ConditionalRequirementBehavior::Hard);
}

#[test]
fn alternative_group_has_multiple_clauses() {
    let group = RepositoryRequirementGroup::alternatives(
        RepositoryRequirementKind::Depends,
        vec![
            RepositoryRequirementClause::name_only("default-mta".to_string()),
            RepositoryRequirementClause::name_only("mail-transport-agent".to_string()),
        ],
    );
    assert!(!group.is_simple());
    assert_eq!(group.alternatives.len(), 2);
}

#[test]
fn optional_group_carries_description() {
    let group = RepositoryRequirementGroup::optional(
        RepositoryRequirementClause::name_only("fzf".to_string()),
        Some("fuzzy finder for interactive use".to_string()),
    );
    assert_eq!(group.kind, RepositoryRequirementKind::Optional);
    assert!(group.description.is_some());
}

#[test]
fn conditional_behavior_round_trips() {
    let group = RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        RepositoryRequirementClause::versioned("systemd".to_string(), ">= 255".to_string()),
    )
    .with_behavior(ConditionalRequirementBehavior::Conditional)
    .with_native_text("(systemd >= 255 if systemd-resolved)".to_string());

    assert_eq!(group.behavior, ConditionalRequirementBehavior::Conditional);
    assert_eq!(
        group.native_text.as_deref(),
        Some("(systemd >= 255 if systemd-resolved)")
    );
}

#[test]
fn provide_constructors_set_correct_kind() {
    let pkg = RepositoryProvide::package_name("bash".to_string(), Some("5.2-1".to_string()));
    assert_eq!(pkg.kind, RepositoryCapabilityKind::PackageName);

    let virt = RepositoryProvide::virtual_cap("java-runtime".to_string(), None);
    assert_eq!(virt.kind, RepositoryCapabilityKind::Virtual);

    let so = RepositoryProvide::soname("libc.so.6".to_string(), None);
    assert_eq!(so.kind, RepositoryCapabilityKind::Soname);

    let file = RepositoryProvide::file("/usr/bin/python3".to_string());
    assert_eq!(file.kind, RepositoryCapabilityKind::File);
    assert!(file.version.is_none());
}

#[test]
fn versioned_clause_stores_constraint() {
    let clause = RepositoryRequirementClause::versioned("libc6".to_string(), ">= 2.34".to_string());
    assert_eq!(clause.version_constraint.as_deref(), Some(">= 2.34"));
    assert!(clause.capability_kind.is_none());
}

#[test]
fn capability_validation_rejects_malformed_provenance_and_architecture() {
    // The capability name is the only path a source-path provenance has, so
    // a relative name is exactly the malformation the rule must catch.
    let mut capability = ProvidedCapability {
        kind: RepositoryCapabilityKind::File,
        name: "usr/bin/tool".to_string(),
        version: None,
        version_relation: None,
        version_scheme: VersionScheme::Arch,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::SourceDerivedFile {
            format: SourcePackageFormat::Alpm,
        },
    };
    assert!(capability.validate().is_err());

    capability.provenance = CapabilityProvenance::SourcePromisedPath {
        format: SourcePackageFormat::Alpm,
    };
    assert!(capability.validate().is_err());

    // The same relative name is fine once nothing claims it is a path.
    capability.provenance = CapabilityProvenance::AuthorDeclared;
    capability.kind = RepositoryCapabilityKind::Generic;
    capability.validate().unwrap();

    capability.architecture_qualifier = ProvideArchitectureQualifier::Exact(String::new());
    assert!(capability.validate().is_err());
}

#[test]
fn a_path_capability_never_stores_its_path_in_provenance() {
    // The persisted and signed shape is the contract: `repository_provides`
    // holds one row per package-owned path for a whole distribution, so a
    // duplicated path is one copy of every file path in the repository.
    let shipped = ProvidedCapability::payload_file(SourcePackageFormat::Rpm, "/usr/bin/sh");
    let promised = ProvidedCapability::promised_path(SourcePackageFormat::Rpm, "/var/run/tool");
    let declared = ProvidedCapability {
        kind: RepositoryCapabilityKind::Generic,
        name: "webserver".to_string(),
        version: None,
        version_relation: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::SourceDeclared {
            format: SourcePackageFormat::Rpm,
            record_index: 7,
        },
    };

    for capability in [&shipped, &promised] {
        let encoded = serde_json::to_value(&capability.provenance).unwrap();
        assert_eq!(
            encoded
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["role", "format"]),
            "a path provenance carries only why the path is a capability"
        );
        assert!(
            !serde_json::to_string(&capability.provenance)
                .unwrap()
                .contains(&capability.name),
            "provenance must not restate the capability path"
        );
        capability.validate().unwrap();
    }

    assert_eq!(
        serde_json::to_string(&shipped.provenance).unwrap(),
        r#"{"role":"source-derived-file","format":"rpm"}"#
    );
    assert_eq!(
        serde_json::to_string(&promised.provenance).unwrap(),
        r#"{"role":"source-promised-path","format":"rpm"}"#
    );
    // A non-path provenance is untouched by that cut.
    assert_eq!(
        serde_json::to_string(&declared.provenance).unwrap(),
        r#"{"role":"source-declared","format":"rpm","record_index":7}"#
    );
}

#[test]
fn every_materialized_file_provider_validates_under_its_source_format() {
    for format in [
        SourcePackageFormat::Rpm,
        SourcePackageFormat::Debian,
        SourcePackageFormat::Alpm,
        SourcePackageFormat::Ccs,
    ] {
        let mut provides = Vec::new();
        extend_materialized_file_provides(
            &mut provides,
            format,
            ["/usr/bin/sh", "/usr/share/doc/tool/README", "/"],
        )
        .unwrap();

        assert_eq!(
            provides
                .iter()
                .map(|provide| provide.name.as_str())
                .collect::<Vec<_>>(),
            vec!["/usr/bin/sh", "/usr/share/doc/tool/README"],
            "the root directory is not a file provider"
        );
        for provide in &provides {
            provide.validate().unwrap();
            assert_eq!(provide.kind, RepositoryCapabilityKind::File);
            assert_eq!(provide.version_scheme, format.version_scheme());
            assert_eq!(
                provide.provenance,
                CapabilityProvenance::SourceDerivedFile { format }
            );
        }
    }
}

#[test]
fn materialized_file_providers_never_duplicate_a_declared_path() {
    let mut provides = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::File,
        name: "/usr/bin/sh".to_string(),
        version: None,
        version_relation: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::SourceDerivedFile {
            format: SourcePackageFormat::Rpm,
        },
    }];

    extend_materialized_file_provides(
        &mut provides,
        SourcePackageFormat::Rpm,
        ["/usr/bin/sh", "/usr/bin/sh", "/usr/bin/bash"],
    )
    .unwrap();

    assert_eq!(
        provides
            .iter()
            .map(|provide| provide.name.as_str())
            .collect::<Vec<_>>(),
        vec!["/usr/bin/sh", "/usr/bin/bash"]
    );
}

#[test]
fn materialized_file_providers_reject_a_relative_payload_path() {
    let mut provides = Vec::new();

    let error =
        extend_materialized_file_provides(&mut provides, SourcePackageFormat::Rpm, ["usr/bin/sh"])
            .expect_err("a relative payload path has no absolute file-provider identity");

    assert!(
        error.to_string().contains("absolute path"),
        "unexpected error: {error}"
    );
    assert!(provides.is_empty());
}

#[test]
fn a_promised_path_is_a_provider_but_never_content() {
    let promised = ProvidedCapability::promised_path(
        SourcePackageFormat::Rpm,
        "/etc/crypto-policies/back-ends/krb5.config",
    );
    promised.validate().unwrap();

    assert!(promised.is_promised_path());
    assert!(!promised.witnesses_installed_content());
    assert_eq!(promised.kind, RepositoryCapabilityKind::File);
    assert_eq!(promised.version_scheme, VersionScheme::Rpm);

    let shipped = ProvidedCapability::payload_file(SourcePackageFormat::Rpm, "/usr/bin/sh");
    shipped.validate().unwrap();
    assert!(!shipped.is_promised_path());
    assert!(shipped.witnesses_installed_content());
}

#[test]
fn a_promised_path_cannot_claim_content_semantics() {
    let mut promised =
        ProvidedCapability::promised_path(SourcePackageFormat::Rpm, "/etc/tool/generated.conf");

    promised.version = Some("1".to_string());
    promised.version_relation = Some(ProvideVersionRelation::Equal);
    assert!(promised.validate().is_err(), "a promise carries no version");

    let mut wrong_kind =
        ProvidedCapability::promised_path(SourcePackageFormat::Rpm, "/etc/tool/generated.conf");
    wrong_kind.kind = RepositoryCapabilityKind::Generic;
    assert!(wrong_kind.validate().is_err(), "a promise is a file path");

    let mut relative =
        ProvidedCapability::promised_path(SourcePackageFormat::Rpm, "etc/tool/generated.conf");
    relative.name = "etc/tool/generated.conf".to_string();
    assert!(
        relative.validate().is_err(),
        "a promise is an absolute path"
    );
}

#[test]
fn one_package_cannot_both_ship_and_promise_a_path() {
    let mut provides = Vec::new();
    extend_promised_path_provides(
        &mut provides,
        SourcePackageFormat::Rpm,
        ["/etc/pam.d/system-auth"],
    )
    .unwrap();

    let error = extend_materialized_file_provides(
        &mut provides,
        SourcePackageFormat::Rpm,
        ["/etc/pam.d/system-auth"],
    )
    .expect_err("shipping a promised path contradicts the promise");
    assert!(
        error.to_string().contains("shipped payload"),
        "unexpected error: {error}"
    );

    // The reverse order is the same contradiction.
    let mut shipped = Vec::new();
    extend_materialized_file_provides(&mut shipped, SourcePackageFormat::Rpm, ["/usr/bin/sh"])
        .unwrap();
    assert!(
        extend_promised_path_provides(&mut shipped, SourcePackageFormat::Rpm, ["/usr/bin/sh"])
            .is_err()
    );
}

#[test]
fn repeating_the_same_ownership_claim_is_not_a_contradiction() {
    let mut provides = Vec::new();
    for _ in 0..2 {
        extend_promised_path_provides(
            &mut provides,
            SourcePackageFormat::Rpm,
            ["/etc/pam.d/system-auth"],
        )
        .unwrap();
    }
    assert_eq!(provides.len(), 1);
}

#[test]
fn source_format_and_version_scheme_recover_each_other() {
    for format in [
        SourcePackageFormat::Rpm,
        SourcePackageFormat::Debian,
        SourcePackageFormat::Alpm,
        SourcePackageFormat::Ccs,
    ] {
        assert_eq!(
            SourcePackageFormat::for_version_scheme(format.version_scheme()),
            format
        );
    }
}
