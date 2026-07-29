// apps/conary/tests/rpm_named_ownership.rs

//! End-to-end proof that RPM header names remain source authority while the
//! selected target root supplies the numeric identity used at apply time.

use conary_core::db::models::{FileEntry, InstallSource, Trove, TroveType};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use conary_core::repository::versioning::VersionScheme;
use conary_core::runtime_root::ConaryRuntimeRoot;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const PACKAGE_NAME: &str = "named-owner-fixture";
const PACKAGE_PATH: &str = "/usr/lib/named-owner-fixture/payload";
const USER_NAME: &str = "conary-fixture-user";
const GROUP_NAME: &str = "conary-fixture-group";

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write_named_owner_rpm(directory: &Path) -> std::path::PathBuf {
    let mut builder = rpm::PackageBuilder::new(
        PACKAGE_NAME,
        "1.0.0",
        "MIT",
        std::env::consts::ARCH,
        "named payload ownership fixture",
    );
    builder
        .with_file_contents(
            b"named owner payload\n".to_vec(),
            rpm::FileOptions::new(PACKAGE_PATH)
                .permissions(0o640)
                .user(USER_NAME)
                .group(GROUP_NAME),
        )
        .unwrap();
    let package = builder.build().unwrap();
    let path = directory.join("named-owner-fixture.rpm");
    package.write_file(&path).unwrap();
    path
}

fn resolved_test_node(mut node: PayloadNode) -> ResolvedPayloadNode {
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn directory_entry(path: &str, trove_id: i64) -> FileEntry {
    let mut node = PayloadNode::regular(0o755);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | 0o755;
    FileEntry::new(path.to_string(), resolved_test_node(node), None, trove_id)
}

fn regular_entry(
    runtime_root: &ConaryRuntimeRoot,
    path: &str,
    content: &[u8],
    mode: u32,
    trove_id: i64,
) -> FileEntry {
    let sha256 = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        .unwrap()
        .store(content)
        .unwrap();
    FileEntry::new(
        path.to_string(),
        resolved_test_node(PayloadNode::regular(mode)),
        Some(PayloadContentAuthority {
            sha256,
            size: content.len() as u64,
        }),
        trove_id,
    )
}

fn seed_selected_root(db_path: &Path, uid: u32, gid: u32) {
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    stage_test_boot_assets(runtime_root.root());
    let conn = conary_core::db::open(db_path).unwrap();
    let mut base = Trove::new_with_source(
        "named-owner-base".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        VersionScheme::Conary,
    );
    let base_id = base.insert(&conn).unwrap();
    for path in [
        "/etc",
        "/sbin",
        "/usr",
        "/usr/lib",
        "/usr/lib/named-owner-fixture",
    ] {
        directory_entry(path, base_id).insert(&conn).unwrap();
    }
    regular_entry(
        &runtime_root,
        "/etc/passwd",
        format!(
            "root:x:0:0:root:/root:/bin/sh\n{USER_NAME}:x:{uid}:{gid}:fixture:/:/sbin/nologin\n"
        )
        .as_bytes(),
        0o644,
        base_id,
    )
    .insert(&conn)
    .unwrap();
    regular_entry(
        &runtime_root,
        "/etc/group",
        format!("root:x:0:\n{GROUP_NAME}:x:{gid}:\n").as_bytes(),
        0o644,
        base_id,
    )
    .insert(&conn)
    .unwrap();
    regular_entry(
        &runtime_root,
        "/sbin/init",
        b"#!/bin/sh\nexec true\n",
        0o755,
        base_id,
    )
    .insert(&conn)
    .unwrap();
    conary_core::ccs::HostCapabilityInventory::discover()
        .unwrap()
        .persist(&conn)
        .unwrap();
    let selected_root = tempfile::tempdir().unwrap();
    conary_core::generation::builder::materialize_selected_root_from_db(
        &conn,
        &runtime_root.objects_dir(),
        selected_root.path(),
    )
    .unwrap();
    let captured = conary_core::generation::root_manifest::scan_selected_root(
        selected_root.path(),
        &conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap(),
    )
    .unwrap();
    let (generation, _) =
        conary_core::generation::builder::build_generation_from_captured_root_with_boot_root_and_activation(
            &conn,
            &runtime_root.generations_dir(),
            "named ownership selected-root base",
            &runtime_root.root().join("boot"),
            conary_core::generation::builder::GenerationActivation::Active,
            captured,
        )
        .unwrap();
    conary_core::generation::mount::update_current_symlink(runtime_root.root(), generation)
        .unwrap();
}

fn stage_test_boot_assets(runtime_root: &Path) {
    let boot_root = runtime_root.join("boot");
    fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    fs::write(boot_root.join("vmlinuz-test-kernel"), b"test-kernel").unwrap();
    fs::write(
        boot_root.join("initramfs-test-kernel.img"),
        b"test-initramfs",
    )
    .unwrap();
    fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
}

#[test]
fn rpm_install_resolves_named_ownership_from_selected_target_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    fs::create_dir_all(&root).unwrap();
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    conary_core::db::init(&db_path).unwrap();
    seed_selected_root(&db_path, uid, gid);

    let rpm_path = write_named_owner_rpm(temp.path());
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args([
            "install",
            rpm_path.to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
            "--no-deps",
            "--sandbox",
            "always",
            "--yes",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));

    let conn = conary_core::db::open(&db_path).unwrap();
    let file = FileEntry::find_by_path(&conn, PACKAGE_PATH)
        .unwrap()
        .expect("installed payload row");
    assert_eq!(
        file.node.source.user,
        PayloadIdentity::Named {
            name: USER_NAME.to_string()
        }
    );
    assert_eq!(
        file.node.source.group,
        PayloadIdentity::Named {
            name: GROUP_NAME.to_string()
        }
    );
    assert_eq!(file.node.uid, u64::from(uid));
    assert_eq!(file.node.gid, u64::from(gid));
    assert_eq!(
        file.content.as_ref().unwrap().sha256,
        conary_core::hash::sha256(b"named owner payload\n")
    );

    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path.clone());
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .expect("RPM install should publish a selected generation");
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .unwrap();
    let installed = artifact
        .generation_root
        .entries
        .iter()
        .find(|entry| entry.path == PACKAGE_PATH)
        .expect("installed payload in selected generation");
    assert_eq!(installed.node.uid, u64::from(uid));
    assert_eq!(installed.node.gid, u64::from(gid));
    assert_eq!(installed.node.source.mode & 0o7777, 0o640);
    let content = installed.content.as_ref().expect("payload content");
    assert_eq!(
        conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
            .unwrap()
            .retrieve(&content.sha256)
            .unwrap(),
        b"named owner payload\n"
    );
}
