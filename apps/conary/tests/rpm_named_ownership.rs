// apps/conary/tests/rpm_named_ownership.rs

//! End-to-end proof that RPM header names remain source authority while the
//! selected target root supplies the numeric identity used at apply time.

use conary_core::db::models::FileEntry;
use conary_core::payload::PayloadIdentity;
use std::fs;
use std::os::unix::fs::MetadataExt;
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

#[test]
fn rpm_install_resolves_named_ownership_from_selected_target_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    fs::create_dir_all(root.join("etc")).unwrap();
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    fs::write(
        root.join("etc/passwd"),
        format!(
            "root:x:0:0:root:/root:/bin/sh\n{USER_NAME}:x:{uid}:{gid}:fixture:/:/sbin/nologin\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("etc/group"),
        format!("root:x:0:\n{GROUP_NAME}:x:{gid}:\n"),
    )
    .unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    conary_core::ccs::HostCapabilityInventory::discover()
        .persist(&conn)
        .unwrap();
    drop(conn);

    let rpm_path = write_named_owner_rpm(temp.path());
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
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

    let installed = root.join(PACKAGE_PATH.trim_start_matches('/'));
    assert_eq!(fs::read(&installed).unwrap(), b"named owner payload\n");
    let metadata = fs::metadata(&installed).unwrap();
    assert_eq!(metadata.uid(), uid);
    assert_eq!(metadata.gid(), gid);
    assert_eq!(metadata.mode() & 0o7777, 0o640);

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
}
