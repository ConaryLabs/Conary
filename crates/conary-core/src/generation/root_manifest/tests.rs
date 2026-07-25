// conary-core/src/generation/root_manifest/tests.rs

use super::*;
use crate::filesystem::CasStore;
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::Path;

#[test]
fn root_path_domains_are_an_exact_finite_contract() {
    assert_eq!(
        classify_root_path("/opt/vendor/tool").unwrap(),
        RootPathDomain::Immutable
    );
    assert_eq!(
        classify_root_path("/etc/vendor.conf").unwrap(),
        RootPathDomain::ConfigState
    );
    assert_eq!(
        classify_root_path("/var/lib/vendor/state").unwrap(),
        RootPathDomain::MutableState
    );
    assert_eq!(
        classify_root_path("/srv/vendor/data").unwrap(),
        RootPathDomain::MutableState
    );
    for top in [
        "proc", "sys", "dev", "run", "tmp", "home", "root", "mnt", "media",
    ] {
        assert_eq!(
            classify_root_path(&format!("/{top}/anything")).unwrap(),
            RootPathDomain::EphemeralMountOrUser
        );
    }
    // Similar spelling is not authority for exclusion.
    assert_eq!(
        classify_root_path("/temporary/file").unwrap(),
        RootPathDomain::Immutable
    );
}

#[test]
fn mutable_state_transaction_derives_exact_touched_paths() {
    let before = MutableStateManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        entries: vec![
            directory_entry("/etc", 0o755),
            regular_entry("/etc/changed", b"before"),
            regular_entry("/etc/deleted", b"deleted"),
        ],
    };
    let after = MutableStateManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        entries: vec![
            directory_entry("/etc", 0o755),
            regular_entry("/etc/added", b"added"),
            regular_entry("/etc/changed", b"after"),
        ],
    };

    let transaction = MutableStateTransaction::between(before, after).unwrap();
    assert_eq!(
        transaction.touched_paths,
        ["/etc/added", "/etc/changed", "/etc/deleted"]
    );
    transaction.validate().unwrap();
}

#[test]
fn manifest_requires_explicit_parent_directories() {
    let manifest = GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root: directory_node(0o755),
        entries: vec![regular_entry("/usr/bin/tool", b"tool")],
    };
    assert!(
        manifest
            .validate()
            .unwrap_err()
            .to_string()
            .contains("explicit parent directory")
    );
}

#[test]
fn selected_root_round_trip_preserves_typed_tree_and_omits_ephemeral_domains() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    std::fs::create_dir(&source).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o751)).unwrap();

    for directory in [
        "usr", "usr/bin", "opt", "etc", "var", "var/lib", "srv", "tmp",
    ] {
        std::fs::create_dir(source.join(directory)).unwrap();
    }
    std::fs::write(source.join("usr/bin/tool"), b"immutable executable").unwrap();
    std::fs::set_permissions(
        source.join("usr/bin/tool"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::hard_link(
        source.join("usr/bin/tool"),
        source.join("usr/bin/tool-hardlink"),
    )
    .unwrap();
    std::os::unix::fs::symlink("usr/bin", source.join("bin")).unwrap();
    std::fs::write(source.join("opt/vendor"), b"opt is immutable").unwrap();
    std::fs::write(source.join("etc/vendor.conf"), b"configured=true\n").unwrap();
    std::fs::write(source.join("tmp/ignored"), b"ephemeral").unwrap();
    create_fifo(&source.join("var/lib/events"));
    let socket = UnixListener::bind(source.join("srv/service.sock")).unwrap();
    drop(socket);

    set_user_xattr(&source.join("usr/bin/tool"), "user.conary-test", b"exact");
    let captured = scan_selected_root(&source, &cas).unwrap();
    assert!(
        captured
            .generation
            .entries
            .iter()
            .any(|entry| entry.path == "/opt/vendor")
    );
    assert!(
        captured
            .generation
            .entries
            .iter()
            .all(|entry| !entry.path.starts_with("/tmp"))
    );
    assert!(
        captured
            .state
            .entries
            .iter()
            .any(|entry| entry.path == "/srv/service.sock")
    );

    materialize_captured_selected_root(&captured, &cas, &destination).unwrap();
    let round_trip = scan_selected_root(&destination, &cas).unwrap();
    assert_eq!(round_trip, captured);

    let original = std::fs::metadata(source.join("usr/bin/tool")).unwrap();
    let original_link = std::fs::metadata(source.join("usr/bin/tool-hardlink")).unwrap();
    let restored = std::fs::metadata(destination.join("usr/bin/tool")).unwrap();
    let restored_link = std::fs::metadata(destination.join("usr/bin/tool-hardlink")).unwrap();
    assert_eq!(original.ino(), original_link.ino());
    assert_eq!(restored.ino(), restored_link.ino());
    assert_ne!(original.ino(), restored.ino());
}

#[test]
fn scanner_rejects_hardlinks_across_publication_domains() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("usr")).unwrap();
    std::fs::create_dir(root.join("etc")).unwrap();
    std::fs::write(root.join("usr/shared"), b"shared").unwrap();
    std::fs::hard_link(root.join("usr/shared"), root.join("etc/shared")).unwrap();

    let error = scan_selected_root(&root, &cas).unwrap_err();
    assert!(error.to_string().contains("publication domains"));
}

#[cfg(feature = "composefs-rs")]
#[test]
fn erofs_builder_serializes_the_manifest_tree_and_persists_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let generation = temp.path().join("generation");
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("usr")).unwrap();
    std::fs::write(root.join("usr/tool"), b"tool").unwrap();

    let captured = scan_selected_root(&root, &cas).unwrap();
    let result = build_erofs_image_from_root_manifest(&captured.generation, &generation).unwrap();

    assert!(result.image_path.is_file());
    assert!(result.image_size > 0);
    assert_eq!(result.cas_objects_referenced, 1);
    assert_eq!(
        GenerationRootManifest::read_from(&generation).unwrap(),
        captured.generation
    );
}

fn directory_node(permissions: u32) -> ResolvedPayloadNode {
    resolved(PayloadNode {
        kind: PayloadNodeKind::Directory,
        mode: libc::S_IFDIR | permissions,
        user: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        },
        group: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
}

fn directory_entry(path: &str, permissions: u32) -> GenerationRootEntry {
    GenerationRootEntry {
        path: path.to_string(),
        node: directory_node(permissions),
        content: None,
    }
}

fn regular_entry(path: &str, bytes: &[u8]) -> GenerationRootEntry {
    GenerationRootEntry {
        path: path.to_string(),
        node: resolved(PayloadNode {
            kind: PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            mode: libc::S_IFREG | 0o644,
            user: PayloadIdentity::Numeric {
                id: u64::from(unsafe { libc::geteuid() }),
            },
            group: PayloadIdentity::Numeric {
                id: u64::from(unsafe { libc::getegid() }),
            },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: BTreeMap::new(),
        }),
        content: Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        }),
    }
}

fn resolved(node: PayloadNode) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn create_fifo(path: &Path) {
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o640) }, 0);
}

fn set_user_xattr(path: &Path, name: &str, value: &[u8]) {
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let name = CString::new(name).unwrap();
    assert_eq!(
        unsafe {
            libc::lsetxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast::<libc::c_void>(),
                value.len(),
                0,
            )
        },
        0
    );
}
