// src/commands/ccs/install/command_payload_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::stage_test_boot_assets;

#[tokio::test]
async fn ccs_install_rejects_child_write_beneath_package_symlink() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::filesystem::CasStore;
    use conary_core::hash;

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let outside_root = temp_dir.path().join("outside");
    let package_path = temp_dir.path().join("symlink-escape.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    std::fs::create_dir_all(&outside_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();

    let symlink_target = outside_root.to_string_lossy().to_string();
    let symlink_hash = CasStore::compute_symlink_hash(&symlink_target);
    let child_path = "/usr/lib/link/cron.d/persist".to_string();
    let child_content = b"persist".to_vec();
    let child_hash = hash::sha256(&child_content);

    let files = vec![
        FileEntry {
            path: "/usr/lib/link".to_string(),
            hash: symlink_hash.clone(),
            size: symlink_target.len() as u64,
            mode: 0o120777,
            component: "runtime".to_string(),
            file_type: FileType::Symlink,
            target: Some(symlink_target.clone()),
            chunks: None,
        },
        FileEntry {
            path: child_path.clone(),
            hash: child_hash.clone(),
            size: child_content.len() as u64,
            mode: 0o100644,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("symlink-escape", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "test-runtime".to_string(),
                size: (symlink_target.len() + child_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (symlink_hash, symlink_target.as_bytes().to_vec()),
            (child_hash, child_content.clone()),
        ]),
        total_size: (symlink_target.len() + child_content.len()) as u64,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("path traversal") || err.to_string().contains("symlink"),
        "unexpected error: {err:#}"
    );
    assert!(!outside_root.join("cron.d/persist").exists());
}

#[tokio::test]
async fn ccs_install_rejects_child_before_package_symlink() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::filesystem::CasStore;
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let outside_root = temp_dir.path().join("outside");
    let package_path = temp_dir.path().join("reversed-symlink-escape.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    std::fs::create_dir_all(&outside_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let symlink_target = outside_root.to_string_lossy().to_string();
    let symlink_hash = CasStore::compute_symlink_hash(&symlink_target);
    let child_path = "/usr/lib/link/cron.d/persist".to_string();
    let child_content = b"persist".to_vec();
    let child_hash = hash::sha256(&child_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);

    let files = vec![
        FileEntry {
            path: child_path.clone(),
            hash: child_hash.clone(),
            size: child_content.len() as u64,
            mode: 0o100644,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/lib/link".to_string(),
            hash: symlink_hash.clone(),
            size: symlink_target.len() as u64,
            mode: 0o120777,
            component: "runtime".to_string(),
            file_type: FileType::Symlink,
            target: Some(symlink_target.clone()),
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("reversed-symlink-escape", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "test-runtime".to_string(),
                size: (symlink_target.len() + child_content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (child_hash, child_content.clone()),
            (symlink_hash, symlink_target.as_bytes().to_vec()),
            (init_hash, init_content),
        ]),
        total_size: (symlink_target.len() + child_content.len()) as u64,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("path traversal") || err.to_string().contains("symlink"),
        "unexpected error: {err:#}"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let persisted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            [&child_path],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn ccs_install_persists_usrmerge_payload_under_usr_path() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("usrmerge.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"chkconfig".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let total_size = (content.len() + init_content.len()) as u64;
    let files = vec![
        FileEntry {
            path: "bin/chkconfig".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("usrmerge", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: total_size,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash, content.clone()), (init_hash, init_content)]),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(
        !install_root.join("usr/bin/chkconfig").exists(),
        "usr-merge package payload must be recorded for generation build, not written live"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let stored_path: String = conn
        .query_row(
            "SELECT path FROM files WHERE path = '/usr/bin/chkconfig'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_path, "/usr/bin/chkconfig");
    let legacy_path_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'bin/chkconfig'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_path_count, 0);
    assert!(std::fs::read_link(temp_dir.path().join("current")).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn ccs_install_allows_identical_existing_symlink_destination() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::filesystem::CasStore;
    use std::path::PathBuf;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("bash-link.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(install_root.join("usr/bin")).unwrap();
    std::os::unix::fs::symlink("bash", install_root.join("usr/bin/sh")).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let target = "bash".to_string();
    let symlink_hash = CasStore::compute_symlink_hash(&target);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "/usr/bin/sh".to_string(),
            hash: symlink_hash.clone(),
            size: target.len() as u64,
            mode: 0o120777,
            component: "runtime".to_string(),
            file_type: FileType::Symlink,
            target: Some(target.clone()),
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("bash-link", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (target.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (symlink_hash, target.as_bytes().to_vec()),
            (init_hash, init_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_link(install_root.join("usr/bin/sh")).unwrap(),
        PathBuf::from("bash")
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let symlink_target: String = conn
        .query_row(
            "SELECT symlink_target FROM files WHERE path = '/usr/bin/sh'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(symlink_target, "bash");
}

#[cfg(unix)]
#[tokio::test]
async fn ccs_install_replaces_existing_leaf_symlink_destination() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::filesystem::CasStore;
    use std::path::PathBuf;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("library-link.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(install_root.join("usr/lib64")).unwrap();
    std::os::unix::fs::symlink(
        "libtasn1.so.6.6.4",
        install_root.join("usr/lib64/libtasn1.so.6"),
    )
    .unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let target = "libtasn1.so.6.6.5".to_string();
    let symlink_hash = CasStore::compute_symlink_hash(&target);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "/usr/lib64/libtasn1.so.6".to_string(),
            hash: symlink_hash.clone(),
            size: target.len() as u64,
            mode: 0o120777,
            component: "runtime".to_string(),
            file_type: FileType::Symlink,
            target: Some(target.clone()),
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("library-link", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (target.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (symlink_hash, target.as_bytes().to_vec()),
            (init_hash, init_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_link(install_root.join("usr/lib64/libtasn1.so.6")).unwrap(),
        PathBuf::from("libtasn1.so.6.6.4")
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let symlink_target: String = conn
        .query_row(
            "SELECT symlink_target FROM files WHERE path = '/usr/lib64/libtasn1.so.6'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(symlink_target, "libtasn1.so.6.6.5");
}

#[tokio::test]
async fn ccs_install_coalesces_identical_usrmerge_duplicate_files() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("usrmerge-duplicate.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(install_root.join("usr/bin")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("usr/bin", install_root.join("bin")).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"chkconfig".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "bin/chkconfig".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "usr/bin/chkconfig".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("usrmerge-duplicate", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: content.len() as u64 * 2 + init_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash, content.clone()), (init_hash, init_content)]),
        total_size: content.len() as u64 * 2,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let chkconfig_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/bin/chkconfig'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(chkconfig_count, 1);
    let legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path IN ('bin/chkconfig', 'usr/bin/chkconfig')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 0);
}

#[tokio::test]
async fn ccs_install_rejects_conflicting_usrmerge_duplicate_files() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("usrmerge-conflict.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let bin_content = b"from-bin".to_vec();
    let bin_hash = hash::sha256(&bin_content);
    let usr_content = b"from-usr".to_vec();
    let usr_hash = hash::sha256(&usr_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "bin/chkconfig".to_string(),
            hash: bin_hash.clone(),
            size: bin_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "usr/bin/chkconfig".to_string(),
            hash: usr_hash.clone(),
            size: usr_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("usrmerge-conflict", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (bin_content.len() + usr_content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (bin_hash, bin_content),
            (usr_hash, usr_content),
            (init_hash, init_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("duplicate deployment path"),
        "unexpected error: {err:#}"
    );
}
