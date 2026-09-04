// conary-core/src/ccs/native_export/rpm/tests.rs

use super::*;
use crate::ccs::builder::FileEntry;
use crate::ccs::manifest::{CcsManifest, FileCapability, NativeExport, RpmExport};
use crate::packages::PackageFormat;
use crate::payload::{PayloadIdentity, PayloadNode, PayloadTimestamp};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn create_test_build_result() -> BuildResult {
    let mut manifest = CcsManifest::new_minimal("test-rpm-package", "1.0.0");
    manifest.package.license = Some("MIT".to_string());
    manifest.package.platform = Some(crate::ccs::manifest::Platform {
        arch: Some("x86_64".to_string()),
        ..Default::default()
    });
    BuildResult {
        manifest,
        components: HashMap::new(),
        files: vec![],
        payloads: Vec::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    }
}

fn directory_entry(path: &str, permissions: u32, owner: u64) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        node: PayloadNode {
            kind: PayloadNodeKind::Directory,
            mode: libc::S_IFDIR | permissions,
            user: PayloadIdentity::Numeric { id: owner },
            group: PayloadIdentity::Numeric { id: owner },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: Default::default(),
        },
        content: None,
        component: "runtime".to_string(),
        chunks: None,
    }
}

#[test]
fn test_rpm_generation_empty() {
    let result = create_test_build_result();
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("test.rpm");

    let gen_result = generate(&result, &output_path).unwrap();
    assert!(output_path.exists());
    assert!(gen_result.size > 0);
}

#[test]
fn rpm_versioned_dependency_round_trips_as_typed_header_authority() {
    let mut result = create_test_build_result();
    result.manifest.native_export = Some(NativeExport {
        rpm: Some(RpmExport {
            requires: vec![RpmRelation {
                name: "phase4-repository-fixture".to_string(),
                relation: RpmRelationOperator::Equal,
                version: Some("1.0.0-1".to_string()),
            }],
            ..RpmExport::default()
        }),
        ..NativeExport::default()
    });
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("versioned-dependency.rpm");

    generate(&result, &output_path).unwrap();
    let package = crate::packages::rpm::RpmPackage::parse(output_path.to_str().unwrap())
        .expect("parse generated RPM");
    let requirement = package
        .requirements()
        .iter()
        .find(|group| {
            group
                .alternatives
                .iter()
                .any(|clause| clause.name == "phase4-repository-fixture")
        })
        .expect("versioned RPM dependency");
    assert_eq!(requirement.alternatives.len(), 1);
    assert_eq!(
        requirement.alternatives[0].version_constraint.as_deref(),
        Some("= 1.0.0-1")
    );
}

#[test]
fn rpm_same_name_compatibility_provide_round_trips_as_typed_header_authority() {
    let mut result = create_test_build_result();
    result.manifest.native_export = Some(NativeExport {
        rpm: Some(RpmExport {
            provides: vec![RpmRelation {
                name: "test-rpm-package".to_string(),
                relation: RpmRelationOperator::Equal,
                version: Some("1.0".to_string()),
            }],
            ..RpmExport::default()
        }),
        ..NativeExport::default()
    });
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("versioned-provide.rpm");

    generate(&result, &output_path).unwrap();
    let package = crate::packages::rpm::RpmPackage::parse(output_path.to_str().unwrap())
        .expect("parse generated RPM");
    let compatibility = package
        .resolution_capabilities()
        .unwrap()
        .into_iter()
        .find(|provide| {
            provide.name == "test-rpm-package" && provide.version.as_deref() == Some("1.0")
        })
        .expect("same-name RPM compatibility provide");
    assert_eq!(
        compatibility.version_relation,
        Some(crate::repository::dependency_model::ProvideVersionRelation::Equal)
    );
    assert!(matches!(
        compatibility.provenance,
        crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
            format: crate::repository::dependency_model::SourcePackageFormat::Rpm,
            ..
        }
    ));
}

#[test]
fn rpm_versioned_dependency_rejects_empty_authority() {
    for requirement in [
        RpmRelation {
            name: " ".to_string(),
            relation: RpmRelationOperator::Any,
            version: None,
        },
        RpmRelation {
            name: "phase4-repository-fixture".to_string(),
            relation: RpmRelationOperator::Equal,
            version: Some(" ".to_string()),
        },
    ] {
        let mut result = create_test_build_result();
        result.manifest.native_export = Some(NativeExport {
            rpm: Some(RpmExport {
                requires: vec![requirement],
                ..RpmExport::default()
            }),
            ..NativeExport::default()
        });
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("invalid-dependency.rpm");

        assert!(generate(&result, &output_path).is_err());
        assert!(!output_path.exists());
    }
}

#[test]
fn rpm_versioned_provide_rejects_incomplete_authority_before_publication() {
    for provide in [
        RpmRelation {
            name: " ".to_string(),
            relation: RpmRelationOperator::Any,
            version: None,
        },
        RpmRelation {
            name: "test-rpm-package".to_string(),
            relation: RpmRelationOperator::Equal,
            version: None,
        },
        RpmRelation {
            name: "test-rpm-package".to_string(),
            relation: RpmRelationOperator::Any,
            version: Some("1.0".to_string()),
        },
    ] {
        let mut result = create_test_build_result();
        result.manifest.native_export = Some(NativeExport {
            rpm: Some(RpmExport {
                provides: vec![provide],
                ..RpmExport::default()
            }),
            ..NativeExport::default()
        });
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("invalid-provide.rpm");

        let error = generate(&result, &output_path).unwrap_err().to_string();
        assert!(error.contains("native_export.rpm.provides"), "{error}");
        assert!(!output_path.exists());
    }
}

#[test]
fn root_child_directories_round_trip_with_canonical_paths_and_exact_metadata() {
    let mut result = create_test_build_result();
    result.files = [("/usr", 0o751), ("/opt", 0o750)]
        .into_iter()
        .map(|(path, permissions)| directory_entry(path, permissions, 0))
        .collect();
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("root-directories.rpm");

    let generated = generate(&result, &output_path).unwrap();
    assert!(
        generated
            .loss_report
            .unsupported_features
            .iter()
            .all(|feature| !feature.contains("top-level directory"))
    );
    let package = crate::packages::rpm::RpmPackage::parse(output_path.to_str().unwrap())
        .expect("parse generated RPM");
    let payload = package.package_payload().expect("read RPM payload");

    for (path, permissions) in [("/usr", 0o751), ("/opt", 0o750)] {
        let file = payload
            .files()
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("missing canonical RPM directory {path}"));
        assert!(matches!(file.node.kind, PayloadNodeKind::Directory));
        assert_eq!(file.node.mode & 0o7777, permissions);
        assert_eq!(
            file.node.user,
            PayloadIdentity::Named {
                name: "root".to_string()
            }
        );
        assert_eq!(
            file.node.group,
            PayloadIdentity::Named {
                name: "root".to_string()
            }
        );
    }
    assert!(
        payload
            .files()
            .iter()
            .all(|file| !file.path.starts_with("//"))
    );
}

#[test]
fn directory_export_rejects_nonzero_numeric_ownership() {
    let mut result = create_test_build_result();
    result.files = vec![directory_entry("/opt", 0o750, 1001)];
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("numeric-owner.rpm");

    let error = generate(&result, &output_path).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot represent numeric user identity 1001 exactly")
    );
    assert!(!output_path.exists());
}

#[test]
fn rpm_generation_rejects_missing_license_authority() {
    let mut result = create_test_build_result();
    result.manifest.package.license = None;
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("missing-license.rpm");

    let error = generate(&result, &output_path).unwrap_err();
    assert!(error.to_string().contains("exact package.license"));
    assert!(!output_path.exists());
}

#[test]
fn rpm_file_capability_round_trips_from_typed_manifest_authority() {
    let temp_dir = TempDir::new().unwrap();
    let source = temp_dir.path().join("source");
    let tool = source.join("usr/bin/tool");
    fs::create_dir_all(tool.parent().unwrap()).unwrap();
    fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

    let mut manifest = create_test_build_result().manifest;
    manifest.file_capabilities = vec![FileCapability {
        path: "/usr/bin/tool".to_string(),
        capabilities: vec!["cap_net_bind_service".to_string()],
        permitted: true,
        effective: true,
        inheritable: false,
    }];
    let result = crate::ccs::builder::CcsBuilder::new(manifest, &source)
        .unwrap()
        .build()
        .unwrap();
    let output_path = temp_dir.path().join("capability.rpm");

    generate(&result, &output_path).unwrap();
    let package = crate::packages::rpm::RpmPackage::parse(output_path.to_str().unwrap())
        .expect("parse generated capability RPM");
    let payload = package
        .package_payload()
        .expect("read capability RPM payload");
    let file = payload
        .files()
        .iter()
        .find(|file| file.path == "/usr/bin/tool")
        .expect("capability payload path");
    assert_eq!(
        file.node.xattrs.get("security.capability"),
        Some(
            &crate::generation::builder::encode_security_capability_xattr(
                &result.manifest.file_capabilities[0]
            )
            .unwrap()
        )
    );
}

#[test]
fn rpm_export_rejects_unrepresentable_generic_xattrs() {
    let mut result = create_test_build_result();
    let mut entry = directory_entry("/opt", 0o750, 0);
    entry
        .node
        .xattrs
        .insert("user.conary".to_string(), b"exact".to_vec());
    result.files = vec![entry];
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("generic-xattr.rpm");

    let error = generate(&result, &output_path).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot represent payload xattrs exactly")
    );
    assert!(!output_path.exists());
}

#[test]
fn test_hook_converter_post_install() {
    let mut hooks = Hooks::default();
    hooks.systemd.push(crate::ccs::manifest::SystemdHook {
        unit: "myapp.service".to_string(),
        enable: true,
        reversible: None,
    });

    let converter = RpmHookConverter;
    let script = converter.post_install(&hooks).unwrap();
    assert!(script.contains("systemctl"));
    assert!(script.contains("myapp.service"));
}

#[test]
fn empty_hooks_do_not_invent_ldconfig_scriptlets() {
    let converter = RpmHookConverter;
    assert!(converter.post_install(&Hooks::default()).is_none());
    assert!(converter.post_remove(&Hooks::default()).is_none());
}

#[test]
fn test_hook_converter_preserves_script_hooks() {
    let hooks = Hooks {
        post_install: Some(crate::ccs::manifest::ScriptHook {
            script: "echo installed > /var/lib/myapp/installed".to_string(),
            reversible: None,
        }),
        pre_remove: Some(crate::ccs::manifest::ScriptHook {
            script: "echo removed > /var/lib/myapp/removed".to_string(),
            reversible: None,
        }),
        ..Default::default()
    };

    let converter = RpmHookConverter;
    let post = converter.post_install(&hooks).unwrap();
    let pre_remove = converter.pre_remove(&hooks).unwrap();

    assert!(post.contains("echo installed > /var/lib/myapp/installed"));
    assert!(pre_remove.contains("echo removed > /var/lib/myapp/removed"));
}
