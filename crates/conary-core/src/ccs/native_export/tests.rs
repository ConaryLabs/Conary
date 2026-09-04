// crates/conary-core/src/ccs/native_export/tests.rs

use super::*;
use crate::packages::PackageFormat;
use crate::packages::config_authority::{
    CcsConfigDeclaration, ConfigPayloadAssociation, SourceConfigDeclaration,
};
use crate::payload::PayloadNodeKind;
use std::os::unix::fs::PermissionsExt;

#[test]
fn source_backed_copy_preserves_bytes_and_computes_debian_md5() {
    let result = crate::ccs::builder::test_support::minimal_file_build_result(
        "hello",
        "1.0.0",
        b"hello world",
    );
    let output = tempfile::NamedTempFile::new().unwrap();
    let digest = copy_file_content(&result, &result.files[0], output.path()).unwrap();

    assert_eq!(std::fs::read(output.path()).unwrap(), b"hello world");
    assert_eq!(digest, "5eb63bbbe01eeed093cb22bb8f5acdc3");
}

#[test]
fn source_backed_copy_rejects_content_changed_after_authoring() {
    let source = tempfile::tempdir().unwrap();
    let source_file = source.path().join("payload");
    std::fs::write(&source_file, b"signed bytes").unwrap();
    let result = crate::ccs::builder::CcsBuilder::new(
        crate::ccs::manifest::CcsManifest::new_minimal("changed", "1.0.0"),
        source.path(),
    )
    .unwrap()
    .build()
    .unwrap();
    std::fs::write(source_file, b"changed bytes").unwrap();
    let output = tempfile::NamedTempFile::new().unwrap();

    let error = copy_file_content(&result, &result.files[0], output.path()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("payload source does not match authority")
    );
}

#[test]
fn shell_escape_handles_quotes() {
    assert_eq!(shell_escape("it's"), "'it'\\''s'");
}

#[test]
fn architecture_mapping_remains_format_specific() {
    assert_eq!(arch_for_format(Some("x86_64"), "deb").unwrap(), "amd64");
    assert_eq!(arch_for_format(Some("aarch64"), "deb").unwrap(), "arm64");
    assert_eq!(arch_for_format(Some("amd64"), "rpm").unwrap(), "x86_64");
    assert!(arch_for_format(Some("all"), "rpm").is_err());
    assert!(arch_for_format(Some("any"), "rpm").is_err());
    assert!(arch_for_format(Some("noarch"), "deb").is_err());
    assert!(arch_for_format(None, "arch").is_err());
    assert!(arch_for_format(Some("riscv64"), "arch").is_err());
}

#[cfg(unix)]
#[test]
fn native_exporters_encode_absent_config_declarations_per_format() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join("etc/demo")).unwrap();
    std::fs::write(source.path().join("etc/demo/app.conf"), b"matched\n").unwrap();

    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("config-demo", "1.0.0");
    manifest.package.license = Some("MIT".to_string());
    manifest.package.homepage = Some("https://example.invalid/config-demo".to_string());
    manifest.package.authors = Some(crate::ccs::manifest::Authors {
        maintainers: vec!["Config Demo <config@example.invalid>".to_string()],
        upstream: None,
    });
    manifest.package.platform = Some(crate::ccs::manifest::Platform {
        arch: Some("x86_64".to_string()),
        ..Default::default()
    });
    manifest.config.files = vec![
        SourceConfigDeclaration::Ccs(CcsConfigDeclaration {
            path: "/etc/demo/app.conf".to_string(),
            noreplace: true,
            payload: ConfigPayloadAssociation::Matched,
        }),
        SourceConfigDeclaration::Ccs(CcsConfigDeclaration {
            path: "/etc/demo/absent.conf".to_string(),
            noreplace: true,
            payload: ConfigPayloadAssociation::Absent,
        }),
    ];
    let result = crate::ccs::builder::CcsBuilder::new(manifest, source.path())
        .unwrap()
        .build()
        .unwrap();
    let output = tempfile::tempdir().unwrap();

    let rpm_path = output.path().join("config-demo.rpm");
    rpm::generate(&result, &rpm_path).unwrap();
    let rpm = crate::packages::rpm::RpmPackage::parse(rpm_path.to_str().unwrap()).unwrap();
    assert_absent_declaration("rpm", &rpm, "/etc/demo/absent.conf", |declaration| {
        let crate::packages::config_authority::SourceConfigDeclaration::Rpm(value) = declaration
        else {
            panic!("rpm absent declaration has the wrong source kind");
        };
        value.ghost && value.noreplace
    });

    let deb_path = output.path().join("config-demo.deb");
    deb::generate(&result, &deb_path).unwrap();
    let deb = crate::packages::deb::DebPackage::parse(deb_path.to_str().unwrap()).unwrap();
    assert_absent_declaration("deb", &deb, "/etc/demo/absent.conf", |declaration| {
        let crate::packages::config_authority::SourceConfigDeclaration::Debian(value) = declaration
        else {
            panic!("deb absent declaration has the wrong source kind");
        };
        value.remove_on_upgrade
    });

    let arch_path = output.path().join("config-demo.pkg.tar.zst");
    arch::generate(&result, &arch_path).unwrap();
    let arch = crate::packages::arch::ArchPackage::parse(arch_path.to_str().unwrap()).unwrap();
    assert_absent_declaration("arch", &arch, "/etc/demo/absent.conf", |declaration| {
        let crate::packages::config_authority::SourceConfigDeclaration::Alpm(value) = declaration
        else {
            panic!("arch absent declaration has the wrong source kind");
        };
        value.payload == ConfigPayloadAssociation::Absent
    });

    // The absent declaration never invents payload bytes in any format.
    for package in [
        &rpm as &dyn PackageFormat,
        &deb as &dyn PackageFormat,
        &arch as &dyn PackageFormat,
    ] {
        assert!(
            package
                .package_payload()
                .unwrap()
                .files()
                .iter()
                .all(|file| file.path != "/etc/demo/absent.conf"),
            "absent declaration must not invent a payload member"
        );
    }
}

fn assert_absent_declaration(
    format: &str,
    package: &impl PackageFormat,
    path: &str,
    semantics: impl Fn(&SourceConfigDeclaration) -> bool,
) {
    let declarations = package.config_declarations().unwrap();
    let absent = declarations
        .iter()
        .find(|declaration| declaration.path() == path)
        .unwrap_or_else(|| panic!("{format} export dropped the absent declaration {path}"));
    assert!(
        semantics(absent),
        "{format} absent declaration lost its typed source semantics: {absent:?}"
    );
    let matched = declarations
        .iter()
        .find(|declaration| declaration.path() == "/etc/demo/app.conf")
        .unwrap_or_else(|| panic!("{format} export dropped the matched declaration"));
    assert_eq!(
        matched.payload(),
        ConfigPayloadAssociation::Matched,
        "{format} matched declaration changed its payload association"
    );
}

#[cfg(unix)]
#[test]
fn native_exporters_round_trip_explicit_directory_and_symlink_topology() {
    let source = tempfile::tempdir().unwrap();
    let state_dir = source.path().join("usr/lib/topology/state");
    let bin_dir = source.path().join("usr/bin");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::write(bin_dir.join("topology-tool"), b"topology\n").unwrap();
    std::os::unix::fs::symlink("topology-tool", bin_dir.join("topology-link")).unwrap();

    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("topology", "1.0.0");
    manifest.package.license = Some("MIT".to_string());
    manifest.package.homepage = Some("https://example.invalid/topology".to_string());
    manifest.package.authors = Some(crate::ccs::manifest::Authors {
        maintainers: vec!["Topology Test <topology@example.invalid>".to_string()],
        upstream: None,
    });
    manifest.package.platform = Some(crate::ccs::manifest::Platform {
        arch: Some("x86_64".to_string()),
        ..Default::default()
    });
    let mut result = crate::ccs::builder::CcsBuilder::new(manifest, source.path())
        .unwrap()
        .build()
        .unwrap();
    let state = result
        .files
        .iter_mut()
        .find(|file| file.path == "/usr/lib/topology/state")
        .expect("explicit topology directory authority");
    state.node.user = crate::payload::PayloadIdentity::Numeric { id: 0 };
    state.node.group = crate::payload::PayloadIdentity::Numeric { id: 0 };
    let output = tempfile::tempdir().unwrap();

    let rpm_path = output.path().join("topology.rpm");
    rpm::generate(&result, &rpm_path).unwrap();
    let rpm = crate::packages::rpm::RpmPackage::parse(rpm_path.to_str().unwrap()).unwrap();
    assert_topology("rpm", &rpm);

    let deb_path = output.path().join("topology.deb");
    deb::generate(&result, &deb_path).unwrap();
    let deb = crate::packages::deb::DebPackage::parse(deb_path.to_str().unwrap()).unwrap();
    assert_topology("deb", &deb);

    let arch_path = output.path().join("topology.pkg.tar.zst");
    arch::generate(&result, &arch_path).unwrap();
    let arch = crate::packages::arch::ArchPackage::parse(arch_path.to_str().unwrap()).unwrap();
    assert_topology("arch", &arch);
}

fn assert_topology(format: &str, package: &impl PackageFormat) {
    let payload = package.package_payload().unwrap();
    let directory = payload
        .files()
        .iter()
        .find(|file| file.path == "/usr/lib/topology/state")
        .expect("explicit topology directory");
    assert!(matches!(directory.node.kind, PayloadNodeKind::Directory));
    assert_eq!(directory.node.mode & 0o7777, 0o750);

    if format == "rpm" {
        assert!(
            payload.files().iter().all(|file| file.path != "/usr/bin"),
            "rpm must not claim the target filesystem package's default parent directory"
        );
        assert!(
            payload
                .files()
                .iter()
                .all(|file| file.path != "/usr/lib/topology"),
            "rpm must leave default parent directories implicit"
        );
    }

    let symlink = payload
        .files()
        .iter()
        .find(|file| file.path == "/usr/bin/topology-link")
        .expect("topology symlink");
    let PayloadNodeKind::Symlink { target } = &symlink.node.kind else {
        panic!("{format} topology link is {:?}", symlink.node.kind);
    };
    assert_eq!(target, "topology-tool", "{format} symlink target");
}

#[test]
fn native_exporters_round_trip_exact_hardlink_topology() {
    let result = hardlink_build_result();
    let output = tempfile::tempdir().unwrap();

    let rpm_path = output.path().join("hardlinks.rpm");
    rpm::generate(&result, &rpm_path).unwrap();
    let rpm = crate::packages::rpm::RpmPackage::parse(rpm_path.to_str().unwrap()).unwrap();
    assert_hardlink_topology("rpm", &rpm);

    let deb_path = output.path().join("hardlinks.deb");
    deb::generate(&result, &deb_path).unwrap();
    let deb = crate::packages::deb::DebPackage::parse(deb_path.to_str().unwrap()).unwrap();
    assert_hardlink_topology("deb", &deb);

    let arch_path = output.path().join("hardlinks.pkg.tar.zst");
    arch::generate(&result, &arch_path).unwrap();
    let arch = crate::packages::arch::ArchPackage::parse(arch_path.to_str().unwrap()).unwrap();
    assert_hardlink_topology("arch", &arch);
}

fn assert_hardlink_topology(format: &str, package: &impl PackageFormat) {
    use std::io::Read;

    let payload = package.package_payload().unwrap();
    let anchor = payload
        .files()
        .iter()
        .find(|file| file.path == "/usr/lib/hardlinks/zz-anchor")
        .expect("hardlink anchor");
    let PayloadNodeKind::Regular {
        hardlink_identity: Some(anchor_identity),
    } = &anchor.node.kind
    else {
        panic!("{format} hardlink anchor is {:?}", anchor.node.kind);
    };
    let alias = payload
        .files()
        .iter()
        .find(|file| file.path == "/usr/lib/hardlinks/aa-alias")
        .expect("hardlink alias");
    let PayloadNodeKind::Hardlink { target, identity } = &alias.node.kind else {
        panic!("{format} hardlink alias is {:?}", alias.node.kind);
    };
    assert_eq!(target, "/usr/lib/hardlinks/zz-anchor");
    assert_eq!(identity, anchor_identity);
    assert_eq!(anchor.node.mode & 0o7777, 0o640);
    assert_eq!(anchor.node.mtime.seconds, 1_700_000_000);
    assert_eq!(anchor.node.mtime.nanoseconds, 0);
    if format == "rpm" {
        assert_eq!(
            anchor.node.user,
            crate::payload::PayloadIdentity::Named {
                name: "root".to_string()
            }
        );
        assert_eq!(
            anchor.node.group,
            crate::payload::PayloadIdentity::Named {
                name: "root".to_string()
            }
        );
    } else {
        assert_eq!(
            anchor.node.user,
            crate::payload::PayloadIdentity::Numeric { id: 0 }
        );
        assert_eq!(
            anchor.node.group,
            crate::payload::PayloadIdentity::Numeric { id: 0 }
        );
    }
    assert!(anchor.node.xattrs.is_empty());
    assert_eq!(alias.node.mode, anchor.node.mode);
    assert_eq!(alias.node.mtime, anchor.node.mtime);
    assert_eq!(alias.node.user, anchor.node.user);
    assert_eq!(alias.node.group, anchor.node.group);
    assert_eq!(alias.node.xattrs, anchor.node.xattrs);
    assert!(alias.content_authority.is_none());
    let authority = anchor
        .content_authority
        .as_ref()
        .expect("hardlink anchor content authority");
    assert_eq!(authority.sha256, crate::hash::sha256(b"shared\n"));
    assert_eq!(authority.size, 7);
    let mut content = Vec::new();
    anchor
        .open_content()
        .unwrap()
        .read_to_end(&mut content)
        .unwrap();
    assert_eq!(content, b"shared\n");
}

#[test]
fn native_export_hardlink_preflight_rejects_invalid_sets() {
    let mut missing = hardlink_build_result();
    let PayloadNodeKind::Hardlink { target, .. } = &mut missing.files[0].node.kind else {
        panic!("fixture alias kind");
    };
    *target = "/usr/lib/hardlinks/missing".to_string();
    let error = hardlinks::Topology::validate(&missing).unwrap_err();
    assert!(error.to_string().contains("targets missing path"));

    let mut cycle = hardlink_build_result();
    let alias_path = cycle.files[0].path.clone();
    let PayloadNodeKind::Hardlink { target, .. } = &mut cycle.files[0].node.kind else {
        panic!("fixture alias kind");
    };
    *target = alias_path;
    let error = hardlinks::Topology::validate(&cycle).unwrap_err();
    assert!(error.to_string().contains("hardlink cycle"));

    let mut metadata = hardlink_build_result();
    metadata.files[0].node.mode = libc::S_IFREG | 0o600;
    let error = hardlinks::Topology::validate(&metadata).unwrap_err();
    assert!(error.to_string().contains("incompatible metadata"));

    let mut partial = hardlink_build_result();
    partial.files.remove(0);
    let error = hardlinks::Topology::validate(&partial).unwrap_err();
    assert!(error.to_string().contains("partial set"));

    let mut alias_content = hardlink_build_result();
    alias_content.files[0].content = alias_content.files[1].content.clone();
    let error = hardlinks::Topology::validate(&alias_content).unwrap_err();
    assert!(error.to_string().contains("non-anchor content authority"));
}

fn hardlink_build_result() -> BuildResult {
    use crate::ccs::builder::FileEntry;
    use crate::payload::{PayloadContentAuthority, PayloadNode};

    let content = b"shared\n".to_vec();
    let identity = "fixture:shared".to_string();
    let mut anchor_node = PayloadNode::regular(0o640);
    anchor_node.mtime = crate::payload::PayloadTimestamp {
        seconds: 1_700_000_000,
        nanoseconds: 0,
    };
    anchor_node.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some(identity.clone()),
    };
    let alias_node = PayloadNode {
        kind: PayloadNodeKind::Hardlink {
            target: "/usr/lib/hardlinks/zz-anchor".to_string(),
            identity,
        },
        ..anchor_node.clone()
    };
    let authority = PayloadContentAuthority {
        sha256: crate::hash::sha256(&content),
        size: content.len() as u64,
    };
    let files = vec![
        FileEntry {
            path: "/usr/lib/hardlinks/aa-alias".to_string(),
            node: alias_node,
            content: None,
            component: "runtime".to_string(),
            chunks: None,
        },
        FileEntry {
            path: "/usr/lib/hardlinks/zz-anchor".to_string(),
            node: anchor_node,
            content: Some(authority.clone()),
            component: "runtime".to_string(),
            chunks: None,
        },
    ];
    let payloads = crate::ccs::builder::payloads_from_bounded_memory_for_tests(
        &files,
        std::collections::HashMap::from([(authority.sha256.clone(), content)]),
    )
    .unwrap();
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hardlinks", "1.0.0");
    manifest.package.license = Some("MIT".to_string());
    manifest.package.homepage = Some("https://example.invalid/hardlinks".to_string());
    manifest.package.authors = Some(crate::ccs::manifest::Authors {
        maintainers: vec!["Hardlink Test <hardlinks@example.invalid>".to_string()],
        upstream: None,
    });
    manifest.package.platform = Some(crate::ccs::manifest::Platform {
        arch: Some("x86_64".to_string()),
        ..Default::default()
    });
    BuildResult {
        manifest,
        components: std::collections::HashMap::new(),
        files,
        payloads,
        total_size: authority.size,
        chunked: false,
        chunk_stats: None,
    }
}

#[test]
fn loss_report_tracks_unsupported_features() {
    let mut report = LossReport::default();
    assert!(report.is_empty());

    report.add_unsupported("merkle tree verification");
    assert!(!report.is_empty());
    assert_eq!(report.unsupported_features.len(), 1);
}
