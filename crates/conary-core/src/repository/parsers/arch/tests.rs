// crates/conary-core/src/repository/parsers/arch/tests.rs

use super::*;
use crate::repository::dependency_model::RepositoryCapabilityKind;

fn parser() -> ArchParser {
    let trust = PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Arch {
        keyring: crate::repository::ArchKeyringTrust {
            url: "https://keys.example.test/arch.gpg".to_string(),
            format: crate::repository::ArchKeyringFormat::OpenPgp,
            master_fingerprints: vec!["A".repeat(40)],
            packager_key_threshold: 1,
        },
        sig_level: crate::repository::ArchSigLevel::distribution_default(),
    });
    ArchParser::new("core".to_string(), trust).unwrap()
}

#[test]
fn test_parse_desc_file() {
    let parser = parser();
    let content = "%NAME%\nbash\n\n%VERSION%\n5.2.037-1\n\n%DESC%\nThe GNU Bourne Again shell\n";

    let fields = parser.parse_desc_file(content).unwrap();

    assert_eq!(fields.get("NAME"), Some(&vec!["bash".to_string()]));
    assert_eq!(fields.get("VERSION"), Some(&vec!["5.2.037-1".to_string()]));
    assert_eq!(
        fields.get("DESC"),
        Some(&vec!["The GNU Bourne Again shell".to_string()])
    );
}

#[test]
fn desc_parser_rejects_duplicate_fields_and_unscoped_values() {
    let parser = parser();

    assert!(
        parser
            .parse_desc_file("%NAME%\nfirst\n\n%NAME%\nsecond\n")
            .is_err()
    );
    assert!(parser.parse_desc_file("orphan\n%NAME%\nfixture\n").is_err());
}

#[test]
fn repository_dependency_uses_canonical_arch_parser() {
    let parsed = crate::repository::requirement::parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Arch,
        "glibc>=2.17",
    )
    .unwrap();
    assert_eq!(parsed.alternatives[0].name, "glibc");
    assert_eq!(
        parsed.alternatives[0].version_constraint.as_deref(),
        Some(">= 2.17")
    );
}

#[test]
fn optional_dependency_preserves_version_epoch_before_description() {
    let parser = parser();

    let groups = parser
        .parse_structured_depends("%OPTDEPENDS%\nruntime>=2:1.0-1: optional runtime\n")
        .unwrap();
    assert_eq!(groups[0].alternatives[0].name, "runtime");
    assert_eq!(
        groups[0].alternatives[0].version_constraint.as_deref(),
        Some(">= 2:1.0-1")
    );
    assert_eq!(groups[0].description.as_deref(), Some("optional runtime"));
}

#[test]
fn test_parse_desc_file_provides_persisted_in_extra_metadata() {
    let parser = parser();
    let desc = "\
%NAME%
mailer

%VERSION%
1.0-1

%FILENAME%
mailer-1.0-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64

%PROVIDES%
mail-transport-agent
smtp-server=1.0
";

    let fields = parser.parse_desc_file(desc).unwrap();
    let package = parser
        .package_from_fields("https://example.test", &fields, None)
        .unwrap();
    let metadata = package.extra_metadata.as_object().unwrap();
    let provides = metadata
        .get("arch_provides")
        .and_then(|value| value.as_array())
        .unwrap();
    let provides: Vec<&str> = provides.iter().filter_map(|value| value.as_str()).collect();

    assert!(provides.contains(&"mail-transport-agent"));
    assert!(provides.contains(&"smtp-server=1.0"));
}

#[test]
fn test_source_distro_and_version_scheme() {
    let parser = parser();
    let desc = "\
%NAME%
bash

%VERSION%
5.2.037-1

%FILENAME%
bash-5.2.037-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64
";
    let fields = parser.parse_desc_file(desc).unwrap();
    let pkg = parser
        .package_from_fields("https://example.test", &fields, None)
        .unwrap();

    assert_eq!(pkg.dependency_flavor, RepositoryDependencyFlavor::Arch);
    assert_eq!(pkg.version_scheme, VersionScheme::Arch);
}

#[test]
fn test_structured_versioned_depends() {
    let parser = parser();
    let desc = "\
%NAME%
bash

%VERSION%
5.2.037-1

%FILENAME%
bash-5.2.037-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64
";
    let depends = "\
%DEPENDS%
glibc>=2.36
readline
ncurses
";
    let fields = parser.parse_desc_file(desc).unwrap();
    let pkg = parser
        .package_from_fields("https://example.test", &fields, Some(depends))
        .unwrap();

    assert_eq!(pkg.requirements.len(), 3);

    let glibc = &pkg.requirements[0];
    assert_eq!(glibc.kind, RepositoryRequirementKind::Depends);
    assert_eq!(glibc.alternatives[0].name, "glibc");
    assert_eq!(
        glibc.alternatives[0].version_constraint.as_deref(),
        Some(">= 2.36")
    );

    let readline = &pkg.requirements[1];
    assert_eq!(readline.alternatives[0].name, "readline");
    assert!(readline.alternatives[0].version_constraint.is_none());
}

#[test]
fn repository_metadata_preserves_arch_negative_relations() {
    let parser = parser();
    let desc = "\
%NAME%
newpkg

%VERSION%
2-1

%FILENAME%
newpkg-2-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64

%CONFLICTS%
old-conflict<2

%REPLACES%
old-owner=1
";
    let fields = parser.parse_desc_file(desc).unwrap();

    let package = parser
        .package_from_fields("https://example.test", &fields, None)
        .unwrap();

    assert_eq!(
        package
            .requirements
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Replace,
        ]
    );
    assert_eq!(
        package
            .requirements
            .iter()
            .map(|relation| relation.native_text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("old-conflict<2"), Some("old-owner=1")]
    );
}

#[test]
fn repository_metadata_rejects_malformed_arch_relation() {
    let parser = parser();
    let desc = "\
%NAME%
newpkg

%VERSION%
2-1

%FILENAME%
newpkg-2-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64

%CONFLICTS%
oldpkg>=
";
    let fields = parser.parse_desc_file(desc).unwrap();

    let error = parser
        .package_from_fields("https://example.test", &fields, None)
        .unwrap_err();

    assert!(error.to_string().contains("empty version"));
}

#[test]
fn test_structured_versioned_provides() {
    let parser = parser();
    let desc = "\
%NAME%
glibc

%VERSION%
2.40-1

%FILENAME%
glibc-2.40-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
200

%ARCH%
x86_64

%PROVIDES%
libm.so=6-64
libpthread.so
libwlroots-0.18.so=libwlroots-0.18.so-64
lib:libwayland-client.so.0
";

    let fields = parser.parse_desc_file(desc).unwrap();
    let pkg = parser
        .package_from_fields("https://example.test", &fields, None)
        .unwrap();

    // Self-provide + 4 explicit provides
    assert_eq!(pkg.provides.len(), 5);

    let self_prov = pkg
        .provides
        .iter()
        .find(|p| p.name == "glibc" && p.kind == RepositoryCapabilityKind::PackageName)
        .expect("self-provide missing");
    assert_eq!(self_prov.version.as_deref(), Some("2.40-1"));

    let libm = pkg
        .provides
        .iter()
        .find(|p| p.name == "libm.so=6-64")
        .expect("libm.so provide missing");
    assert_eq!(libm.kind, RepositoryCapabilityKind::Soname);
    assert!(libm.version.is_none());

    let libpthread = pkg
        .provides
        .iter()
        .find(|p| p.name == "libpthread.so")
        .expect("libpthread.so provide missing");
    assert_eq!(libpthread.kind, RepositoryCapabilityKind::Soname);
    assert!(libpthread.version.is_none());

    let wlroots = pkg
        .provides
        .iter()
        .find(|p| p.name == "libwlroots-0.18.so=libwlroots-0.18.so-64")
        .expect("versioned soname v1 provide missing");
    assert_eq!(wlroots.kind, RepositoryCapabilityKind::Soname);
    assert!(wlroots.version.is_none());

    let wayland = pkg
        .provides
        .iter()
        .find(|p| p.name == "lib:libwayland-client.so.0")
        .expect("soname v2 provide missing");
    assert_eq!(wayland.kind, RepositoryCapabilityKind::Soname);
    assert!(wayland.version.is_none());
}

#[test]
fn repository_runtime_sonames_are_atomic_typed_requirements() {
    let parser = parser();
    let requirements = parser
        .parse_structured_depends(
            "%DEPENDS%\n\
                 libwlroots-0.18.so=libwlroots-0.18.so-64\n\
                 lib:libwayland-client.so.0\n",
        )
        .unwrap();

    assert_eq!(requirements.len(), 2);
    for (requirement, identity) in requirements.iter().zip([
        "libwlroots-0.18.so=libwlroots-0.18.so-64",
        "lib:libwayland-client.so.0",
    ]) {
        let clause = &requirement.alternatives[0];
        assert_eq!(clause.name, identity);
        assert_eq!(
            clause.capability_kind,
            Some(RepositoryCapabilityKind::Soname)
        );
        assert!(clause.version_constraint.is_none());
    }
}

#[test]
fn repository_rejects_non_exact_arch_provide_version() {
    let parser = parser();
    let mut fields = HashMap::new();
    fields.insert("PROVIDES".to_string(), vec!["mail-api>=2".to_string()]);

    let error = parser
        .build_structured_provides("mail-provider", "1", &fields)
        .unwrap_err();
    assert!(error.to_string().contains("exact '='"), "{error}");
}

#[test]
fn test_implicit_self_provide_always_present() {
    let parser = parser();
    let desc = "\
%NAME%
coreutils

%VERSION%
9.5-1

%FILENAME%
coreutils-9.5-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
100

%ARCH%
x86_64
";
    let fields = parser.parse_desc_file(desc).unwrap();
    let pkg = parser
        .package_from_fields("https://example.test", &fields, None)
        .unwrap();

    assert!(!pkg.provides.is_empty());
    let self_prov = &pkg.provides[0];
    assert_eq!(self_prov.name, "coreutils");
    assert_eq!(self_prov.kind, RepositoryCapabilityKind::PackageName);
    assert_eq!(self_prov.version.as_deref(), Some("9.5-1"));
}

#[test]
fn repository_rejects_malformed_arch_dependency() {
    let parser = parser();
    let error = parser
        .parse_structured_depends("%DEPENDS%\nopenssl>=\n")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid Arch %DEPENDS% entry 'openssl>='"),
        "{error}"
    );
}

#[test]
fn snapshot_identity_owns_the_served_compressed_database_bytes() {
    let served = b"compressed alpm database bytes";
    assert_eq!(
        AuthenticatedSnapshotIdentity::for_bytes(served).size(),
        Some(served.len() as u64)
    );
    assert_eq!(
        AuthenticatedSnapshotIdentity::for_bytes(served),
        AuthenticatedSnapshotIdentity::for_bytes(served)
    );
    assert_ne!(
        AuthenticatedSnapshotIdentity::for_bytes(served),
        AuthenticatedSnapshotIdentity::for_bytes(b"decoded tar bytes")
    );
}
