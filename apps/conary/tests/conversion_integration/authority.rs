// tests/conversion_integration/authority.rs

use super::*;

#[test]
#[ignore = "requires CONARY_ISSUE_104_FIXTURE_DIR with the two pinned official Arch artifacts"]
fn pinned_arch_same_name_provides_convert_verify_reopen_and_resolve() {
    use conary_core::packages::arch::ArchPackage;
    use conary_core::repository::dependency_model::{
        CapabilityProvenance, ProvideVersionRelation, RepositoryCapabilityKind, SourcePackageFormat,
    };

    const FIXTURES: [(&str, &str, &str); 2] = [
        (
            "aspnet-runtime-10.0.10.sdk110-1-x86_64.pkg.tar.zst",
            "d3983ae27b3e6e470bc33fd060a29c2e9a4a0c5a363ea76a07dd4ce300709c9c",
            "aspnet-runtime",
        ),
        (
            "aspnet-targeting-pack-10.0.10.sdk110-1-x86_64.pkg.tar.zst",
            "c4f557ea81af86713971cd20b193fbd4282fdbd30874973d9bd9ae0bfe15bc86",
            "aspnet-targeting-pack",
        ),
    ];

    let fixture_dir = PathBuf::from(
        std::env::var_os("CONARY_ISSUE_104_FIXTURE_DIR")
            .expect("CONARY_ISSUE_104_FIXTURE_DIR must name the pinned fixture directory"),
    );
    for (filename, expected_sha256, expected_name) in FIXTURES {
        let fixture = fixture_dir.join(filename);
        let bytes = std::fs::read(&fixture).expect("read pinned Arch artifact");
        assert_eq!(conary_core::hash::sha256(&bytes), expected_sha256);

        let package = ArchPackage::parse(fixture.to_str().expect("fixture path is UTF-8"))
            .expect("parse pinned Arch artifact");
        let metadata = ForeignConversionInput::from_package(fixture, &package)
            .expect("project source-package authority");
        let payload = package.package_payload().expect("open Arch payload");
        let source_hash =
            conary_core::hash::Hash::parse_prefixed(&format!("sha256:{expected_sha256}"))
                .expect("valid pinned fixture hash");
        let output = TempDir::new().expect("converter output");
        let converter = signed_test_converter(ConversionOptions {
            output_dir: output.path().to_path_buf(),
        });
        let result = converter
            .convert_payload(&metadata, payload.files(), "arch", &source_hash)
            .expect("convert and sign pinned Arch artifact");
        let converted = parse_converted_package(&result);
        let authority = converted.v3_authority().expect("signed CCS v3 authority");

        assert_eq!(authority.identity.name, expected_name);
        assert_eq!(authority.identity.version, "10.0.10.sdk110-1");
        assert_eq!(authority.identity.version_scheme, VersionScheme::Arch);
        assert_eq!(authority.identity.architecture.as_deref(), Some("x86_64"));
        let same_name = authority
            .provided_capabilities
            .iter()
            .find(|capability| {
                capability.kind == conary_core::ccs::v3::schema::DependencyKindV3::Package
                    && capability.name == expected_name
                    && capability.provider_version.as_deref() == Some("10.0")
            })
            .expect("signed same-name compatibility capability");
        assert_eq!(
            same_name.version_relation,
            Some(ProvideVersionRelation::Equal)
        );
        assert!(matches!(
            same_name.provenance,
            CapabilityProvenance::SourceDeclared {
                format: SourcePackageFormat::Alpm,
                ..
            }
        ));

        let resolution = converted
            .resolution_capabilities()
            .expect("project verified resolution authority");
        assert_eq!(
            resolution
                .iter()
                .filter(|capability| capability.provenance == CapabilityProvenance::ExactIdentity)
                .count(),
            1
        );
        assert!(resolution.iter().any(|capability| {
            capability.kind == RepositoryCapabilityKind::PackageName
                && capability.name == expected_name
                && capability.version.as_deref() == Some("10.0")
                && capability.version_relation == Some(ProvideVersionRelation::Equal)
                && capability.version_scheme == VersionScheme::Arch
                && matches!(
                    capability.provenance,
                    CapabilityProvenance::SourceDeclared {
                        format: SourcePackageFormat::Alpm,
                        ..
                    }
                )
        }));
    }
}

#[test]
fn golden_conversion_native_free_is_current_without_entries() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let metadata = create_test_metadata("adapter-registry-native-free");
    let files = create_test_files("adapter-registry-native-free");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("native-free conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .native_lifecycle
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert!(bundle.entries.is_empty());
    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::NativeFree);
    assert_eq!(result.scriptlet_metadata.scriptlet_fidelity, "native-free");
}

#[test]
fn golden_conversion_preserves_source_lifecycle_without_static_effect_authority() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("adapter-registry-fully-replaced");
    metadata.native_scriptlet_abi = vec![rpm_lifecycle_entry(
        "%post",
        RpmScriptletSlot::Post,
        NativeLifecyclePath::PostInstall,
        NativeTransactionPosition::AfterPayload,
        "\
/sbin/ldconfig
systemctl daemon-reload
systemctl enable demo.service
systemd-tmpfiles --create /usr/lib/tmpfiles.d/demo.conf
systemd-sysusers /usr/lib/sysusers.d/demo.conf
update-mime-database /usr/share/mime
restorecon -R /usr/bin/adapter-registry-fully-replaced
semanage fcontext -a -t demo_exec_t /usr/bin/adapter-registry-fully-replaced
semodule -i /usr/share/selinux/packages/demo.pp
setsebool -P demo_can_network on
apparmor_parser -r /etc/apparmor.d/usr.bin.demo
update-alternatives --install /usr/bin/editor editor /usr/bin/demo-editor 50
",
    )];
    let files = golden_payload_files("adapter-registry-fully-replaced");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .expect("adapter-backed conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .native_lifecycle
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert_eq!(
        bundle.scriptlet_fidelity,
        ScriptletFidelity::NativeLifecycle
    );
    let entry = bundle.entries.first().expect("preserved lifecycle entry");
    assert!(entry.body.contains("/sbin/ldconfig"));
    assert!(entry.body.contains("restorecon -R"));
    assert!(entry.body.contains("apparmor_parser -r"));
    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "native-lifecycle"
    );
}

#[test]
fn golden_conversion_unknown_command_preserves_typed_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("lifecycle-execution-unknown-shell");
    metadata.native_scriptlet_abi = vec![rpm_lifecycle_entry(
        "%post",
        RpmScriptletSlot::Post,
        NativeLifecyclePath::PostInstall,
        NativeTransactionPosition::AfterPayload,
        "custom-helper --do-thing\n",
    )];
    let files = create_test_files("lifecycle-execution-unknown-shell");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .expect("unknown shell conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .native_lifecycle
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert_eq!(
        bundle.scriptlet_fidelity,
        ScriptletFidelity::NativeLifecycle
    );
    assert!(
        bundle
            .entries
            .iter()
            .any(|entry| entry.body == "custom-helper --do-thing\n")
    );
    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "native-lifecycle"
    );
}

#[test]
fn golden_conversion_preserves_pinned_rpm_hardlink_transaction_owner() {
    use conary_core::packages::rpm::RpmPackage;
    use conary_core::packages::traits::PackageFormat;
    use conary_core::payload::PayloadNodeKind;

    const FIXTURE_SHA256: &str = "cf48a9380416daf02e934cbddd15d5356b3b6ea6b5f0824187074b66dd0fe14a";
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/conary-core/tests/fixtures/rpm/2ping-4.5.1-24.fc44.noarch.rpm");
    let bytes = std::fs::read(&fixture).expect("read pinned Fedora 44 2ping fixture");
    assert_eq!(conary_core::hash::sha256(&bytes), FIXTURE_SHA256);

    let package =
        RpmPackage::parse(fixture.to_str().expect("fixture path is utf-8")).expect("parse fixture");
    let metadata =
        ForeignConversionInput::from_package(fixture, &package).expect("project metadata");
    let payload = package
        .package_payload()
        .expect("open projected fixture payload");
    let source_hash = conary_core::hash::Hash::parse_prefixed(&format!("sha256:{FIXTURE_SHA256}"))
        .expect("valid pinned fixture hash");
    let output = TempDir::new().expect("converter output");
    let result = passive_converter(output.path())
        .convert_payload(&metadata, payload.files(), "rpm", &source_hash)
        .expect("convert pinned fixture");
    let converted = parse_converted_package(&result);

    let owner = converted
        .file_entries()
        .iter()
        .find(|entry| entry.path == "/usr/bin/2ping6")
        .expect("projected RPM transaction owner");
    assert_eq!(owner.node.mode & 0o7777, 0o755);
    assert_eq!(
        owner.node.user,
        conary_core::payload::PayloadIdentity::Named {
            name: "root".to_string()
        }
    );
    assert_eq!(
        owner.node.group,
        conary_core::payload::PayloadIdentity::Named {
            name: "root".to_string()
        }
    );
    assert_eq!(owner.node.mtime.seconds, 1_768_521_600);
    assert_eq!(owner.node.mtime.nanoseconds, 0);
    assert_eq!(
        owner.node.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some("rpm:1:1".to_string())
        }
    );
    assert_eq!(
        owner
            .content
            .as_ref()
            .expect("owner content authority")
            .sha256,
        "581a92b326b83518a80d3d35411e6d0ed1380969306a39d042d5922dd45528aa"
    );

    let alias = converted
        .file_entries()
        .iter()
        .find(|entry| entry.path == "/usr/bin/2ping")
        .expect("projected hardlink alias");
    assert_eq!(
        alias.node.kind,
        PayloadNodeKind::Hardlink {
            target: "/usr/bin/2ping6".to_string(),
            identity: "rpm:1:1".to_string(),
        }
    );
    assert_eq!(alias.node.mode, owner.node.mode);
    assert_eq!(alias.node.user, owner.node.user);
    assert_eq!(alias.node.group, owner.node.group);
    assert_eq!(alias.node.mtime, owner.node.mtime);
    assert_eq!(alias.node.xattrs, owner.node.xattrs);
    assert!(alias.content.is_none());
}
