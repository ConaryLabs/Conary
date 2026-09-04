// crates/conary-core/src/repository/parsers/debian/tests.rs

use super::*;
use crate::repository::RepositoryTrustPolicy;
use crate::repository::dependency_model::RepositoryCapabilityKind;

fn parser() -> DebianParser {
    let trust = PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Debian {
        release_keys: vec![crate::repository::OpenPgpTrustRoot {
            url: "https://keys.example.test/debian.gpg".to_string(),
            fingerprint: "A".repeat(40),
        }],
    });
    DebianParser::new(
        "noble".to_string(),
        "main".to_string(),
        "amd64".to_string(),
        trust,
    )
    .unwrap()
}

#[test]
fn repository_dependency_uses_canonical_debian_parser() {
    let parser = parser();

    let groups = parser
        .parse_requirement_groups("libc6:any (>= 2.34)", RepositoryRequirementKind::Depends)
        .unwrap();
    assert_eq!(groups[0].alternatives[0].name, "libc6");
    assert_eq!(
        groups[0].alternatives[0].version_constraint.as_deref(),
        Some(">= 2.34")
    );
    assert_eq!(
        groups[0].alternatives[0].architecture_qualifier,
        crate::repository::dependency_model::RequirementArchitectureQualifier::Any
    );
}

#[test]
fn test_sync_metadata_persists_debian_provides_in_extra_metadata() {
    let entry = DebianPackageEntry {
        package: "mail-transport-agent".to_string(),
        version: "1.0-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: Some("Test package".to_string()),
        sha256: "d".repeat(64),
        size: "123".to_string(),
        filename: "pool/main/m/mail-transport-agent.deb".to_string(),
        depends: None,
        pre_depends: None,
        conflicts: None,
        breaks: None,
        replaces: None,
        homepage: None,
        section: None,
        installed_size: None,
        provides: Some("mail-transport-agent, smtp-server (= 1.0-1)".to_string()),
    };

    let parser = parser();
    let package = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();
    let metadata = package.extra_metadata.as_object().unwrap();
    let provides = metadata
        .get("deb_provides")
        .and_then(|value| value.as_array())
        .unwrap();
    let provides: Vec<&str> = provides.iter().filter_map(|value| value.as_str()).collect();

    assert!(provides.contains(&"mail-transport-agent"));
    assert!(provides.contains(&"smtp-server (= 1.0-1)"));
}

#[test]
fn test_source_distro_and_version_scheme() {
    let entry = DebianPackageEntry {
        package: "test".to_string(),
        version: "1.0-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: Some("foreign".to_string()),
        description: None,
        sha256: "d".repeat(64),
        size: "100".to_string(),
        filename: "pool/main/t/test.deb".to_string(),
        depends: None,
        pre_depends: None,
        conflicts: None,
        breaks: None,
        replaces: None,
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();
    let pkg = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();
    assert_eq!(pkg.dependency_flavor, RepositoryDependencyFlavor::Deb);
    assert_eq!(pkg.version_scheme, VersionScheme::Debian);
    assert_eq!(pkg.debian_multi_arch, Some(DebianMultiArch::Foreign));
}

#[test]
fn repository_rejects_unknown_debian_multi_arch_value() {
    let entry = DebianPackageEntry {
        package: "test".to_string(),
        version: "1.0-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: Some("sometimes".to_string()),
        description: None,
        sha256: "d".repeat(64),
        size: "100".to_string(),
        filename: "pool/main/t/test.deb".to_string(),
        depends: None,
        pre_depends: None,
        conflicts: None,
        breaks: None,
        replaces: None,
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let error = parser()
        .package_from_entry("https://example.test", entry)
        .unwrap_err();
    assert!(error.to_string().contains("Multi-Arch"), "{error}");
}

#[test]
fn test_structured_versioned_depends() {
    let entry = DebianPackageEntry {
        package: "curl".to_string(),
        version: "8.0-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: None,
        sha256: "a".repeat(64),
        size: "200".to_string(),
        filename: "pool/main/c/curl.deb".to_string(),
        depends: Some("libc6 (>= 2.34), libssl3 (>= 3.0)".to_string()),
        pre_depends: None,
        conflicts: None,
        breaks: None,
        replaces: None,
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();
    let pkg = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();

    assert_eq!(pkg.requirements.len(), 2);
    assert_eq!(pkg.requirements[0].kind, RepositoryRequirementKind::Depends);
    assert_eq!(pkg.requirements[0].alternatives[0].name, "libc6");
    assert_eq!(
        pkg.requirements[0].alternatives[0]
            .version_constraint
            .as_deref(),
        Some(">= 2.34")
    );
    assert_eq!(pkg.requirements[1].alternatives[0].name, "libssl3");
}

#[test]
fn repository_metadata_preserves_debian_negative_relations() {
    let entry = DebianPackageEntry {
        package: "newpkg".to_string(),
        version: "2".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: None,
        sha256: "a".repeat(64),
        size: "200".to_string(),
        filename: "pool/main/n/newpkg.deb".to_string(),
        depends: None,
        pre_depends: None,
        conflicts: Some("old-conflict (<< 2)".to_string()),
        breaks: Some("old-breaks (<= 1)".to_string()),
        replaces: Some("old-owner (= 1)".to_string()),
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();

    let package = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();

    assert_eq!(
        package
            .requirements
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Breaks,
            RepositoryRequirementKind::Replace,
        ]
    );
    assert_eq!(
        package
            .requirements
            .iter()
            .map(|relation| relation.native_text.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("old-conflict (<< 2)"),
            Some("old-breaks (<= 1)"),
            Some("old-owner (= 1)"),
        ]
    );
}

#[test]
fn repository_metadata_rejects_debian_negative_alternatives() {
    let entry = DebianPackageEntry {
        package: "newpkg".to_string(),
        version: "2".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: None,
        sha256: "a".repeat(64),
        size: "200".to_string(),
        filename: "pool/main/n/newpkg.deb".to_string(),
        depends: None,
        pre_depends: None,
        conflicts: Some("old-a | old-b".to_string()),
        breaks: None,
        replaces: None,
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();

    let error = parser
        .package_from_entry("https://example.test", entry)
        .unwrap_err();

    assert!(error.to_string().contains("do not permit alternatives"));
}

#[test]
fn test_structured_or_deps() {
    let entry = DebianPackageEntry {
        package: "postfix".to_string(),
        version: "3.8-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: None,
        sha256: "a".repeat(64),
        size: "300".to_string(),
        filename: "pool/main/p/postfix.deb".to_string(),
        depends: Some("default-mta | mail-transport-agent, libc6".to_string()),
        pre_depends: None,
        conflicts: None,
        breaks: None,
        replaces: None,
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();
    let pkg = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();

    assert_eq!(pkg.requirements.len(), 2);
    // First group has alternatives
    assert_eq!(pkg.requirements[0].alternatives.len(), 2);
    assert_eq!(pkg.requirements[0].alternatives[0].name, "default-mta");
    assert_eq!(
        pkg.requirements[0].alternatives[1].name,
        "mail-transport-agent"
    );
    // Second group is simple
    assert_eq!(pkg.requirements[1].alternatives.len(), 1);
    assert_eq!(pkg.requirements[1].alternatives[0].name, "libc6");
}

#[test]
fn test_structured_versioned_and_unversioned_provides() {
    let entry = DebianPackageEntry {
        package: "exim4".to_string(),
        version: "4.97-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: None,
        sha256: "a".repeat(64),
        size: "400".to_string(),
        filename: "pool/main/e/exim4.deb".to_string(),
        depends: None,
        pre_depends: None,
        conflicts: None,
        breaks: None,
        replaces: None,
        provides: Some(
            "exim4 (= 4.96-compat), mail-transport-agent, smtp-server (= 1.0)".to_string(),
        ),
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();
    let pkg = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();

    // Exact identity + same-name compatibility provide + 2 virtual provides.
    assert!(pkg.provides.len() >= 4);

    let self_prov = pkg
        .provides
        .iter()
        .find(|p| p.name == "exim4" && p.kind == RepositoryCapabilityKind::PackageName)
        .expect("self-provide missing");
    assert_eq!(self_prov.version.as_deref(), Some("4.97-1"));

    let compatibility_prov = pkg
        .provides
        .iter()
        .find(|provide| {
            provide.name == "exim4"
                && provide.version.as_deref() == Some("4.96-compat")
                && matches!(
                    provide.provenance,
                    crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                        format: crate::repository::dependency_model::SourcePackageFormat::Debian,
                        record_index: 0,
                    }
                )
        })
        .expect("same-name compatibility provide missing");
    assert_eq!(
        compatibility_prov.kind,
        RepositoryCapabilityKind::PackageName
    );

    let mta = pkg
        .provides
        .iter()
        .find(|p| p.name == "mail-transport-agent")
        .expect("virtual provide missing");
    assert_eq!(mta.kind, RepositoryCapabilityKind::Virtual);
    assert!(mta.version.is_none());
    assert_eq!(
        mta.provenance,
        crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
            format: crate::repository::dependency_model::SourcePackageFormat::Debian,
            record_index: 1,
        }
    );

    let smtp = pkg
        .provides
        .iter()
        .find(|p| p.name == "smtp-server")
        .expect("versioned virtual provide missing");
    assert_eq!(smtp.kind, RepositoryCapabilityKind::Virtual);
    assert_eq!(smtp.version.as_deref(), Some("1.0"));
    assert_eq!(
        smtp.provenance,
        crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
            format: crate::repository::dependency_model::SourcePackageFormat::Debian,
            record_index: 2,
        }
    );
}

#[test]
fn structured_provides_preserve_explicit_architecture_qualifiers() {
    let provides = parser()
        .parse_structured_provides("mail-api:any (= 2), helper:arm64", "fixture")
        .unwrap();
    assert_eq!(provides[0].name, "mail-api");
    assert_eq!(provides[0].version.as_deref(), Some("2"));
    assert_eq!(
        provides[0].architecture_qualifier,
        crate::repository::dependency_model::ProvideArchitectureQualifier::Any
    );
    assert_eq!(provides[1].name, "helper");
    assert_eq!(
        provides[1].architecture_qualifier,
        crate::repository::dependency_model::ProvideArchitectureQualifier::Exact(
            "arm64".to_string()
        )
    );
}

#[test]
fn structured_provides_reject_non_exact_versions_and_empty_atoms() {
    for invalid in ["mail-api (>= 2)", "mail-api,,helper"] {
        let error = parser()
            .parse_structured_provides(invalid, "fixture")
            .unwrap_err();
        assert!(error.to_string().contains("Provides"), "{error}");
    }
}

#[test]
fn test_structured_pre_depends() {
    let entry = DebianPackageEntry {
        package: "libc6".to_string(),
        version: "2.39-1".to_string(),
        architecture: "amd64".to_string(),
        multi_arch: None,
        description: None,
        sha256: "a".repeat(64),
        size: "500".to_string(),
        filename: "pool/main/g/glibc.deb".to_string(),
        depends: Some("libgcc-s1".to_string()),
        pre_depends: Some("ld-linux-x86-64 (>= 2.39)".to_string()),
        conflicts: None,
        breaks: None,
        replaces: None,
        provides: None,
        homepage: None,
        section: None,
        installed_size: None,
    };
    let parser = parser();
    let pkg = parser
        .package_from_entry("https://example.test", entry)
        .unwrap();

    // Should have 1 Depends + 1 PreDepends
    let depends: Vec<_> = pkg
        .requirements
        .iter()
        .filter(|r| r.kind == RepositoryRequirementKind::Depends)
        .collect();
    let pre_depends: Vec<_> = pkg
        .requirements
        .iter()
        .filter(|r| r.kind == RepositoryRequirementKind::PreDepends)
        .collect();

    assert_eq!(depends.len(), 1);
    assert_eq!(depends[0].alternatives[0].name, "libgcc-s1");

    assert_eq!(pre_depends.len(), 1);
    assert_eq!(pre_depends[0].alternatives[0].name, "ld-linux-x86-64");
    assert_eq!(
        pre_depends[0].alternatives[0].version_constraint.as_deref(),
        Some(">= 2.39")
    );
}
