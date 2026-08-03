// conary-core/src/packages/arch/tests.rs

use super::*;
use crate::packages::traits::{
    ArchAlpmHookOperation, ArchAlpmHookTriggerType, ArchNativeScriptletMetadata,
    NativeArgumentValue, NativeLifecyclePath, NativeScriptletFormat, NativeScriptletKind,
    NativeScriptletMetadata, NativeTransactionPosition,
};
use crate::repository::dependency_model::RepositoryCapabilityKind;
use std::io::Write;

#[test]
fn test_compression_detection() {
    for (magic, expected) in [
        (&[0x28, 0xb5, 0x2f, 0xfd][..], CompressionFormat::Zstd),
        (
            &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00][..],
            CompressionFormat::Xz,
        ),
        (&[0x1f, 0x8b][..], CompressionFormat::Gzip),
    ] {
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        temp.write_all(magic).unwrap();
        let mut file = File::open(temp.path()).unwrap();
        assert_eq!(
            ArchPackage::detect_compression(&mut file).unwrap(),
            expected
        );
    }

    let mut temp = tempfile::NamedTempFile::new().unwrap();
    temp.write_all(b"not compressed").unwrap();
    let mut file = File::open(temp.path()).unwrap();
    assert!(ArchPackage::detect_compression(&mut file).is_err());
}

#[test]
fn test_pkginfo_parsing() {
    let content = r#"
# Sample .PKGINFO
pkgname = test-package
pkgver = 1.0.0-1
pkgdesc = A test package
url = https://example.com
arch = x86_64
license = MIT
license = Apache
depend = glibc>=2.34
depend = zlib
optdepend = python: for scripts
makedepend = gcc
"#;

    let info = ArchPackage::parse_pkginfo(content).unwrap();
    assert_eq!(info.name, Some("test-package".to_string()));
    assert_eq!(info.version, Some("1.0.0-1".to_string()));
    assert_eq!(info.description, Some("A test package".to_string()));
    assert_eq!(info.architecture, Some("x86_64".to_string()));
    assert_eq!(info.licenses.len(), 2);
    assert_eq!(info.dependencies.len(), 2);
    assert_eq!(info.optional_deps.len(), 1);
    assert_eq!(info.make_deps.len(), 1);
}

#[test]
fn pkginfo_rejects_malformed_declared_size() {
    let error = ArchPackage::parse_pkginfo(
        "pkgname = broken\n\
         pkgver = 1-1\n\
         arch = x86_64\n\
         size = not-a-number\n",
    )
    .unwrap_err();

    assert!(error.to_string().contains(".PKGINFO size"), "{error}");
}

#[test]
fn pkginfo_preserves_arch_any_architecture_token() {
    let info =
        ArchPackage::parse_pkginfo("pkgname = portable-data\npkgver = 1-1\narch = any\n").unwrap();
    assert_eq!(info.architecture.as_deref(), Some("any"));
}

#[test]
fn pkginfo_relations_preserve_conflicts_and_replaces_as_typed_authority() {
    let info = ArchPackage::parse_pkginfo(
        "pkgname = newpkg\n\
         pkgver = 2-1\n\
         arch = x86_64\n\
         conflict = old-conflict<2\n\
         replaces = old-owner=1\n",
    )
    .unwrap();

    let mut relations =
        ArchPackage::parse_relations(&info.conflicts, RepositoryRequirementKind::Conflict).unwrap();
    relations.extend(
        ArchPackage::parse_relations(&info.replaces, RepositoryRequirementKind::Replace).unwrap(),
    );

    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Replace,
        ]
    );
    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.native_text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("old-conflict<2"), Some("old-owner=1")]
    );
}

#[test]
fn pkginfo_relation_rejects_malformed_exact_constraint() {
    let error = ArchPackage::parse_relations(
        &["oldpkg>=".to_string()],
        RepositoryRequirementKind::Conflict,
    )
    .unwrap_err();

    assert!(error.to_string().contains("empty version"));
}

#[test]
fn arch_provides_are_exact_typed_provider_contracts() {
    let parsed = ArchPackage::parse_declared_capability_records(
        "example",
        &[
            "example=0.9".to_string(),
            "virtual-abi=2".to_string(),
            "unversioned".to_string(),
            "libwlroots-0.18.so=libwlroots-0.18.so-64".to_string(),
            "lib:libwayland-client.so.0".to_string(),
        ],
    )
    .unwrap();

    assert_eq!(parsed.len(), 5);
    assert_eq!(parsed[0].kind, RepositoryCapabilityKind::PackageName);
    assert_eq!(parsed[0].version.as_deref(), Some("0.9"));
    assert_eq!(parsed[1].kind, RepositoryCapabilityKind::Virtual);
    assert_eq!(parsed[1].version.as_deref(), Some("2"));
    assert_eq!(parsed[2].version, None);
    assert_eq!(parsed[3].name, "libwlroots-0.18.so=libwlroots-0.18.so-64");
    assert_eq!(parsed[3].kind, RepositoryCapabilityKind::Soname);
    assert_eq!(parsed[3].version, None);
    assert_eq!(parsed[4].name, "lib:libwayland-client.so.0");
    assert_eq!(parsed[4].kind, RepositoryCapabilityKind::Soname);
    assert_eq!(parsed[4].version, None);

    let error =
        ArchPackage::parse_declared_capability_records("example", &["virtual-abi>=2".to_string()])
            .unwrap_err();
    assert!(error.to_string().contains("exact '='"));
}

#[test]
fn arch_runtime_optional_and_build_requirements_keep_typed_kinds() {
    let runtime = ArchPackage::parse_requirements(
        &[
            "glibc>=2.34".to_string(),
            "libwlroots-0.18.so=libwlroots-0.18.so-64".to_string(),
        ],
        RepositoryRequirementKind::Depends,
    )
    .unwrap();
    let optional = ArchPackage::parse_requirements(
        &["python: for scripts".to_string()],
        RepositoryRequirementKind::Optional,
    )
    .unwrap();
    let build =
        ArchPackage::parse_requirements(&["gcc".to_string()], RepositoryRequirementKind::Build)
            .unwrap();

    assert_eq!(runtime[0].kind, RepositoryRequirementKind::Depends);
    assert_eq!(
        runtime[0].alternatives[0].version_constraint.as_deref(),
        Some(">= 2.34")
    );
    assert_eq!(
        runtime[1].alternatives[0].name,
        "libwlroots-0.18.so=libwlroots-0.18.so-64"
    );
    assert_eq!(
        runtime[1].alternatives[0].capability_kind,
        Some(RepositoryCapabilityKind::Soname)
    );
    assert!(runtime[1].alternatives[0].version_constraint.is_none());
    assert_eq!(optional[0].kind, RepositoryRequirementKind::Optional);
    assert_eq!(optional[0].description.as_deref(), Some("for scripts"));
    assert_eq!(build[0].kind, RepositoryRequirementKind::Build);
}

#[test]
fn arch_native_abi_preserves_full_install_source_and_libalpm_selection() {
    let install = br#"
pre_install() {
    echo "before $1"
}

post_upgrade() {
    echo "after $2 -> $1"
}
"#;

    let entries = ArchPackage::native_abi_from_install_bytes(install).expect("parse .INSTALL");

    assert_eq!(entries.len(), 2);
    let pre = entries
        .iter()
        .find(|entry| entry.native_slot == "pre_install")
        .expect("pre_install entry");
    assert_eq!(pre.format, NativeScriptletFormat::Arch);
    assert_eq!(pre.primary_lifecycle, NativeLifecyclePath::PreInstall);
    assert_eq!(pre.interpreter, None);
    assert_eq!(pre.body.bytes, install);

    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(meta)) = &pre.metadata
    else {
        panic!("expected arch install metadata");
    };
    assert_eq!(meta.function_name, "pre_install");
    assert_eq!(
        meta.selection_contract,
        crate::packages::native_abi::ArchInstallSelectionContract::LibalpmGrepV1
    );

    let post_upgrade = entries
        .iter()
        .find(|entry| entry.native_slot == "post_upgrade")
        .expect("post_upgrade entry");
    assert_eq!(post_upgrade.invocation.args[0].name, "new-version");
    assert_eq!(
        post_upgrade.invocation.args[0].value,
        NativeArgumentValue::NewVersion
    );
    assert_eq!(post_upgrade.invocation.args[1].name, "old-version");
    assert_eq!(
        post_upgrade.invocation.args[1].value,
        NativeArgumentValue::OldVersion
    );
}

#[test]
fn arch_native_abi_preserves_alpm_hook_control_artifact() {
    let hook = br#"
[Trigger]
Operation = Install
Operation = Upgrade
Type = Path
Target = usr/share/mime/*

[Action]
Description = update mime cache
When = PostTransaction
Exec = /usr/bin/update-mime-database /usr/share/mime
Depends = shared-mime-info
NeedsTargets
"#;

    let entry = ArchPackage::native_abi_from_alpm_hook(
        "/usr/share/libalpm/hooks/30-update-mime.hook",
        hook,
    )
    .unwrap();

    assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(
        entry.native_slot,
        "alpm-hook:/usr/share/libalpm/hooks/30-update-mime.hook"
    );
    assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::Trigger);
    assert_eq!(
        entry.order.position,
        NativeTransactionPosition::ControlArtifact
    );
    assert_eq!(entry.interpreter, None);
    assert_eq!(entry.body.bytes, hook);

    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(meta)) =
        &entry.metadata
    else {
        panic!("expected arch alpm hook metadata");
    };
    assert_eq!(meta.triggers.len(), 1);
    assert_eq!(
        meta.triggers[0].operations,
        vec![
            ArchAlpmHookOperation::Install,
            ArchAlpmHookOperation::Upgrade,
        ]
    );
    assert_eq!(meta.triggers[0].trigger_type, ArchAlpmHookTriggerType::Path);
    assert_eq!(
        meta.triggers[0].targets,
        vec!["usr/share/mime/*".to_string()]
    );
    assert_eq!(
        meta.action.when,
        NativeTransactionPosition::AfterTransaction
    );
    assert!(meta.action.needs_targets);
}

#[test]
fn arch_native_abi_defers_shell_syntax_to_the_configured_runtime() {
    let install = b"pre_install() {\necho '{' unbalanced\n";
    let entries =
        ArchPackage::native_abi_from_install_bytes(install).expect("libalpm selects by name");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].native_slot, "pre_install");
    assert_eq!(entries[0].body.bytes, install);
}

/// Pinned arp-scan fixture carries `security.capability` in both
/// `LIBARCHIVE.xattr` (unpadded base64) and `SCHILY.xattr` (raw) families
/// with identical 20-byte v2 capability blobs.  After projection the parsed
/// node must carry exactly one `security.capability` equal to the SCHILY raw
/// bytes.
#[test]
fn pinned_arp_scan_fixture_dual_family_xattr_projects_one_capability() {
    const FIXTURE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/arch/arp-scan-1.10.0-5-x86_64.pkg.tar.zst"
    ));
    assert_eq!(
        crate::hash::sha256(FIXTURE),
        "add946b61066f95020d51470ae263ffaf660614417c9db3f835037ba106c0e8c"
    );

    let mut temp = tempfile::NamedTempFile::new().unwrap();
    temp.write_all(FIXTURE).unwrap();
    let path = temp.path().to_str().unwrap().to_string();

    let package = ArchPackage::parse(&path).unwrap();
    let files = package.files();

    let arp_scan = files
        .iter()
        .find(|f| f.path == "/usr/bin/arp-scan")
        .expect("arp-scan binary must be in payload");

    // One security.capability xattr projected from dual-family agreement.
    let caps = arp_scan
        .node
        .xattrs
        .get("security.capability")
        .expect("security.capability must be projected");
    assert_eq!(caps.len(), 20, "v2 capability blob must be 20 bytes");
    // The SCHILY raw value from the fixture:
    // 00000002 0020 0000000000000000000000000000
    let expected: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x02, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(
        *caps, expected,
        "projected capability must equal SCHILY raw bytes"
    );
}
