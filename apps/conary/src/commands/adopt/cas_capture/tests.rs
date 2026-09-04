// apps/conary/src/commands/adopt/cas_capture/tests.rs

use super::*;
use conary_core::filesystem::CasStore;
use conary_core::packages::InstalledFileAbsencePolicy;

fn tempdir_in_target() -> tempfile::TempDir {
    std::fs::create_dir_all("target").unwrap();
    tempfile::Builder::new()
        .prefix("conary-cas-capture-")
        .tempdir_in("target")
        .unwrap()
}

fn file_tuple(
    path: &str,
    size: i64,
    mode: i32,
    digest: Option<&str>,
    link_target: Option<&str>,
) -> super::super::FileInfoTuple {
    (
        path.to_string(),
        size,
        mode,
        digest.map(str::to_string),
        Some("root".to_string()),
        Some("root".to_string()),
        link_target.map(str::to_string),
        InstalledFileAbsencePolicy::Required,
    )
}

fn file_tuple_with_policy(
    path: &str,
    policy: InstalledFileAbsencePolicy,
) -> super::super::FileInfoTuple {
    let mut file = file_tuple(path, 0, 0o100644, None, None);
    file.7 = policy;
    file
}

fn assert_error_contains(error: anyhow::Error, snippets: &[&str]) {
    let error = error.to_string();
    for snippet in snippets {
        assert!(
            error.contains(snippet),
            "expected error to contain {snippet:?}; got {error}"
        );
    }
}

#[test]
fn full_adoption_regular_file_requires_cas_storage() {
    let tmp = tempdir_in_target();
    let source = tmp.path().join("hello");
    std::fs::write(&source, b"hello").unwrap();
    let cwd = std::env::current_dir().unwrap();
    let source = source.strip_prefix(cwd).unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();

    let captured = capture_package_files(
        "fixture",
        &[file_tuple(
            source.to_str().unwrap(),
            5,
            0o100644,
            Some("package-manager-digest"),
            None,
        )],
        Some(&cas),
    )
    .unwrap();
    let authority = captured[0].content.as_ref().unwrap();

    assert_ne!(authority.sha256, "package-manager-digest");
    assert_eq!(cas.retrieve(&authority.sha256).unwrap(), b"hello");
}

#[test]
fn full_adoption_cas_survives_in_place_source_mutation() {
    let tmp = tempdir_in_target();
    let source = tmp.path().join("mutable-source");
    std::fs::write(&source, b"original bytes").unwrap();
    let cwd = std::env::current_dir().unwrap();
    let source_arg = source.strip_prefix(cwd).unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();

    let captured = capture_package_files(
        "fixture",
        &[file_tuple(
            source_arg.to_str().unwrap(),
            14,
            0o100644,
            Some("package-manager-digest"),
            None,
        )],
        Some(&cas),
    )
    .unwrap();
    let hash = &captured[0].content.as_ref().unwrap().sha256;

    std::fs::write(&source, b"mutated bytes").unwrap();

    assert_eq!(cas.retrieve(hash).unwrap(), b"original bytes");
}

#[test]
fn complete_adoption_binds_package_to_captured_root_without_rereading_path() {
    use conary_core::generation::root_manifest::{
        CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
        GenerationRootManifest, MutableStateManifest,
    };

    let tmp = tempdir_in_target();
    let source = tmp.path().join("selected-root-source");
    std::fs::write(&source, b"captured bytes").unwrap();
    let cwd = std::env::current_dir().unwrap();
    let source_arg = source.strip_prefix(&cwd).unwrap().to_str().unwrap();
    let node =
        conary_core::generation::root_manifest::capture_existing_payload_node(&source).unwrap();
    let content = PayloadContentAuthority {
        sha256: conary_core::hash::sha256(b"captured bytes"),
        size: 14,
    };
    let captured_root = CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: conary_core::generation::root_manifest::capture_root_node(&cwd).unwrap(),
            entries: vec![GenerationRootEntry {
                path: source_arg.to_string(),
                node: node.clone(),
                content: Some(content.clone()),
            }],
        },
        state: MutableStateManifest::empty(),
    };
    let index = SelectedRootPayloadIndex::new(&captured_root);
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    std::fs::remove_file(&source).unwrap();

    let captured = capture_package_files_from_selected_root(
        "fixture",
        &[file_tuple(source_arg, 14, 0o100644, None, None)],
        &index,
        &cas,
    )
    .unwrap();

    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].node, node);
    assert_eq!(captured[0].content.as_ref(), Some(&content));
}

#[test]
fn complete_adoption_preserves_one_hardlink_topology_across_packages() {
    let tmp = tempdir_in_target();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(root.join("usr/lib")).unwrap();
    std::fs::write(root.join("usr/lib/primary"), b"shared inode").unwrap();
    std::fs::hard_link(root.join("usr/lib/primary"), root.join("usr/lib/secondary")).unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let captured_root =
        conary_core::generation::root_manifest::scan_selected_root(&root, &cas).unwrap();
    let index = SelectedRootPayloadIndex::new(&captured_root);

    let primary = capture_package_files_from_selected_root(
        "package-a",
        &[file_tuple("/usr/lib/primary", 12, 0o100644, None, None)],
        &index,
        &cas,
    )
    .unwrap()
    .pop()
    .unwrap();
    let secondary = capture_package_files_from_selected_root(
        "package-b",
        &[file_tuple("/usr/lib/secondary", 12, 0o100644, None, None)],
        &index,
        &cas,
    )
    .unwrap()
    .pop()
    .unwrap();

    let PayloadNodeKind::Regular {
        hardlink_identity: Some(primary_identity),
    } = primary.node.source.kind
    else {
        panic!("global hardlink primary did not retain its identity");
    };
    let PayloadNodeKind::Hardlink { identity, target } = secondary.node.source.kind else {
        panic!("second package did not retain the global hardlink alias");
    };
    assert_eq!(identity, primary_identity);
    assert_eq!(target, "/usr/lib/primary");
    assert!(primary.content.is_some());
    assert!(secondary.content.is_none());
}

#[test]
#[cfg(unix)]
fn full_adoption_regular_file_uses_private_cas_inode() {
    use std::os::unix::fs::MetadataExt;

    let tmp = tempdir_in_target();
    let source = tmp.path().join("private-inode-source");
    std::fs::write(&source, b"private inode bytes").unwrap();
    let cwd = std::env::current_dir().unwrap();
    let source_arg = source.strip_prefix(cwd).unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();

    let captured = capture_package_files(
        "fixture",
        &[file_tuple(
            source_arg.to_str().unwrap(),
            19,
            0o100644,
            Some("package-manager-digest"),
            None,
        )],
        Some(&cas),
    )
    .unwrap();
    let cas_path = cas
        .hash_to_path(&captured[0].content.as_ref().unwrap().sha256)
        .unwrap();

    assert_ne!(
        std::fs::metadata(&source).unwrap().ino(),
        std::fs::metadata(&cas_path).unwrap().ino(),
        "live full adoption must not share an inode with mutable source files"
    );
}

#[test]
#[cfg(unix)]
fn full_adoption_captures_package_hardlink_topology() {
    let tmp = tempdir_in_target();
    let primary = tmp.path().join("hardlink-a");
    let secondary = tmp.path().join("hardlink-b");
    std::fs::write(&primary, b"shared inode bytes").unwrap();
    std::fs::hard_link(&primary, &secondary).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let primary_arg = primary.strip_prefix(&cwd).unwrap().to_str().unwrap();
    let secondary_arg = secondary.strip_prefix(&cwd).unwrap().to_str().unwrap();

    let captured = capture_package_files(
        "fixture",
        &[
            file_tuple(secondary_arg, 18, 0o100644, None, None),
            file_tuple(primary_arg, 18, 0o100644, None, None),
        ],
        None,
    )
    .unwrap();

    let primary = captured
        .iter()
        .find(|file| file.source.0 == primary_arg)
        .unwrap();
    let secondary = captured
        .iter()
        .find(|file| file.source.0 == secondary_arg)
        .unwrap();
    let PayloadNodeKind::Regular {
        hardlink_identity: Some(primary_identity),
    } = &primary.node.source.kind
    else {
        panic!("lexicographically first package hardlink is not the primary");
    };
    let PayloadNodeKind::Hardlink { target, identity } = &secondary.node.source.kind else {
        panic!("second package hardlink is not typed as a hardlink");
    };
    assert_eq!(target, primary_arg);
    assert_eq!(identity, primary_identity);
    assert!(primary.content.is_some());
    assert!(secondary.content.is_none());
}

#[test]
fn full_adoption_symlink_persists_typed_target_without_fake_content() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let link = tmp.path().join("libfoo.so");
    std::os::unix::fs::symlink("libfoo.so.1", &link).unwrap();

    let captured = capture_package_files(
        "fixture",
        &[file_tuple(
            link.to_str().unwrap(),
            11,
            0o120777,
            Some("package-manager-digest"),
            Some("wrong-discovery-target"),
        )],
        Some(&cas),
    )
    .unwrap();

    assert!(captured[0].content.is_none());
    assert_eq!(
        captured[0].node.source.kind,
        PayloadNodeKind::Symlink {
            target: "libfoo.so.1".to_string()
        }
    );
}

#[test]
fn full_adoption_directory_does_not_require_cas_content() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let directory = tmp.path().join("doc");
    std::fs::create_dir(&directory).unwrap();

    let captured = capture_package_files(
        "fixture",
        &[file_tuple(
            directory.to_str().unwrap(),
            0,
            0o040755,
            Some("directory-digest"),
            None,
        )],
        Some(&cas),
    )
    .unwrap();

    assert!(captured[0].content.is_none());
    assert!(matches!(
        captured[0].node.source.kind,
        PayloadNodeKind::Directory
    ));
}

#[test]
fn full_adoption_special_nodes_have_typed_authority_without_fake_content() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let socket = tmp.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

    let captured = capture_package_files(
        "fixture",
        &[file_tuple(
            socket.to_str().unwrap(),
            0,
            0o140777,
            None,
            None,
        )],
        Some(&cas),
    )
    .unwrap();

    assert!(captured[0].content.is_none());
    assert!(matches!(
        captured[0].node.source.kind,
        PayloadNodeKind::Socket
    ));
}

#[test]
fn full_adoption_validation_checks_inputs_without_creating_cas_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source = tmp.path().join("preview-source");
    std::fs::write(&source, b"preview bytes").unwrap();
    let objects = tmp.path().join("objects");
    let files = vec![file_tuple(
        source.to_str().unwrap(),
        13,
        0o100644,
        None,
        None,
    )];

    validate_package_files("preview", &files).unwrap();

    assert!(!objects.exists());
}

#[test]
fn full_adoption_missing_regular_file_fails_package_preparation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let files = vec![file_tuple(
        "/usr/bin/missing",
        7,
        0o100755,
        Some("package-manager-digest"),
        None,
    )];

    let error = capture_package_files("broken", &files, Some(&cas)).unwrap_err();

    assert_error_contains(
        error,
        &[
            "package broken",
            "/usr/bin/missing",
            "failed to capture exact node",
        ],
    );
}

#[test]
fn native_optional_absence_is_omitted_from_the_live_payload_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let ghost = file_tuple_with_policy(
        tmp.path().join("absent-ghost").to_str().unwrap(),
        InstalledFileAbsencePolicy::RpmGhost,
    );
    let missing_ok = file_tuple_with_policy(
        tmp.path().join("absent-missing-ok").to_str().unwrap(),
        InstalledFileAbsencePolicy::RpmMissingOk,
    );

    let captured =
        capture_package_files("native-optional", &[ghost, missing_ok], Some(&cas)).unwrap();

    assert!(captured.is_empty());
    validate_package_files(
        "native-optional",
        &[file_tuple_with_policy(
            tmp.path().join("still-absent").to_str().unwrap(),
            InstalledFileAbsencePolicy::RpmGhostAndMissingOk,
        )],
    )
    .unwrap();
}

#[test]
fn present_rpm_ghost_is_captured_as_live_payload() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("present-ghost");
    std::fs::write(&path, b"runtime state").unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();

    let captured = capture_package_files(
        "present-ghost",
        &[file_tuple_with_policy(
            path.to_str().unwrap(),
            InstalledFileAbsencePolicy::RpmGhost,
        )],
        Some(&cas),
    )
    .unwrap();

    assert_eq!(captured.len(), 1);
    assert_eq!(
        cas.retrieve(&captured[0].content.as_ref().unwrap().sha256)
            .unwrap(),
        b"runtime state"
    );
}

#[test]
fn native_selected_root_ownership_is_not_removable_package_payload() {
    let tmp = tempfile::TempDir::new().unwrap();
    let below_root = tmp.path().join("owned-directory");
    std::fs::create_dir(&below_root).unwrap();
    let cas = CasStore::new(tmp.path().join("objects")).unwrap();
    let root = file_tuple("/", 0, 0o040755, None, None);
    let child = file_tuple(below_root.to_str().unwrap(), 0, 0o040755, None, None);

    let captured = capture_package_files("filesystem", &[root.clone(), child], Some(&cas)).unwrap();

    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].source.0, below_root.to_string_lossy());
    validate_package_files("filesystem", &[root]).unwrap();
}
