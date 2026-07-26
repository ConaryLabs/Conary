// apps/conary/src/commands/ccs/install/command_payload_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{
    ccs_regular_file, ccs_symlink, seed_test_root_layout, stage_test_boot_assets,
};

#[tokio::test]
async fn ccs_install_rejects_child_write_beneath_package_symlink() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
    let child_path = "/usr/lib/link/cron.d/persist".to_string();
    let child_content = b"persist".to_vec();
    let child_hash = hash::sha256(&child_content);

    let files = vec![
        ccs_symlink(
            "/usr/lib/link".to_string(),
            symlink_target.clone(),
            0o120777,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            child_path.clone(),
            child_hash.clone(),
            child_content.len() as u64,
            0o100644,
            "runtime".to_string(),
        ),
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("symlink-escape", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "test-runtime".to_string(),
                size: child_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(child_hash, child_content.clone())]),
        total_size: child_content.len() as u64,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
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
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
    let child_path = "/usr/lib/link/cron.d/persist".to_string();
    let child_content = b"persist".to_vec();
    let child_hash = hash::sha256(&child_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);

    let files = vec![
        ccs_regular_file(
            child_path.clone(),
            child_hash.clone(),
            child_content.len() as u64,
            0o100644,
            "runtime".to_string(),
        ),
        ccs_symlink(
            "/usr/lib/link".to_string(),
            symlink_target.clone(),
            0o120777,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
    ];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("reversed-symlink-escape", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "test-runtime".to_string(),
                size: (child_content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (child_hash, child_content.clone()),
            (init_hash, init_content.clone()),
        ]),
        total_size: (child_content.len() + init_content.len()) as u64,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
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
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("usrmerge.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(install_root.join("usr/bin")).unwrap();
    std::os::unix::fs::symlink("usr/bin", install_root.join("bin")).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    seed_test_root_layout(
        db_path_str,
        "usrmerge",
        &["/usr", "/usr/bin"],
        &[("/bin", "usr/bin")],
    );
    stage_test_boot_assets(temp_dir.path());

    let content = b"chkconfig".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let total_size = (content.len() + init_content.len()) as u64;
    let files = vec![
        ccs_regular_file(
            "bin/chkconfig".to_string(),
            file_hash.clone(),
            content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
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
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
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
    let retired_alias_path_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'bin/chkconfig'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired_alias_path_count, 0);
    let publication_debts =
        conary_core::db::models::GenerationPublication::pending_recoverable(&conn).unwrap();
    assert!(
        std::fs::read_link(temp_dir.path().join("current")).is_ok(),
        "usr-merge install must publish its active generation: {publication_debts:#?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ccs_install_resolves_safe_existing_lib64_ancestor_inside_root() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("lib64-ancestor.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(install_root.join("usr/lib")).unwrap();
    std::os::unix::fs::symlink("lib", install_root.join("usr/lib64")).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    seed_test_root_layout(
        db_path_str,
        "lib64",
        &["/usr", "/usr/lib"],
        &[("/usr/lib64", "lib")],
    );
    stage_test_boot_assets(temp_dir.path());

    let library_content = b"libform".to_vec();
    let library_hash = hash::sha256(&library_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let total_size = (library_content.len() + init_content.len()) as u64;
    let files = vec![
        ccs_regular_file(
            "/usr/lib64/libform.so.6".to_string(),
            library_hash.clone(),
            library_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
    ];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("lib64-ancestor", "1.0.0"),
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
        blobs: HashMap::from([(library_hash, library_content), (init_hash, init_content)]),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let resolved_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/lib/libform.so.6'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let unresolved_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/lib64/libform.so.6'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(resolved_count, 1);
    assert_eq!(unresolved_count, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn ccs_install_allows_identical_existing_symlink_destination() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    let files = vec![
        ccs_symlink(
            "/usr/bin/sh".to_string(),
            target.clone(),
            0o120777,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
    ];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("bash-link", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: init_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(init_hash, init_content.clone())]),
        total_size: init_content.len() as u64,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_link(install_root.join("usr/bin/sh")).unwrap(),
        PathBuf::from("bash")
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let stored = conary_core::db::models::FileEntry::find_by_path(&conn, "/usr/bin/sh")
        .unwrap()
        .unwrap();
    let conary_core::payload::PayloadNodeKind::Symlink { target } = &stored.node.source.kind else {
        panic!("stored /usr/bin/sh must remain exact symlink authority");
    };
    assert_eq!(target, "bash");
}

#[cfg(unix)]
#[tokio::test]
async fn generation_install_records_replacement_without_mutating_current_leaf_symlink() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    let files = vec![
        ccs_symlink(
            "/usr/lib64/libtasn1.so.6".to_string(),
            target.clone(),
            0o120777,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
    ];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("library-link", "1.0.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: init_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(init_hash, init_content.clone())]),
        total_size: init_content.len() as u64,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_link(install_root.join("usr/lib64/libtasn1.so.6")).unwrap(),
        PathBuf::from("libtasn1.so.6.6.4"),
        "generation-aware install must not mutate the current live root before publication"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let stored =
        conary_core::db::models::FileEntry::find_by_path(&conn, "/usr/lib64/libtasn1.so.6")
            .unwrap()
            .unwrap();
    let conary_core::payload::PayloadNodeKind::Symlink { target } = &stored.node.source.kind else {
        panic!("stored /usr/lib64/libtasn1.so.6 must remain exact symlink authority");
    };
    assert_eq!(target, "libtasn1.so.6.6.5");
}

#[tokio::test]
async fn ccs_install_coalesces_identical_usrmerge_duplicate_files() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
    seed_test_root_layout(
        db_path_str,
        "usrmerge-duplicate",
        &["/usr", "/usr/bin"],
        &[("/bin", "usr/bin")],
    );
    stage_test_boot_assets(temp_dir.path());

    let content = b"chkconfig".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        ccs_regular_file(
            "bin/chkconfig".to_string(),
            file_hash.clone(),
            content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "usr/bin/chkconfig".to_string(),
            file_hash.clone(),
            content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
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
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
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
    let retired_alias_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path IN ('bin/chkconfig', 'usr/bin/chkconfig')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired_alias_count, 0);
}

#[tokio::test]
async fn ccs_install_rejects_conflicting_usrmerge_duplicate_files() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("usrmerge-conflict.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(install_root.join("usr/bin")).unwrap();
    std::os::unix::fs::symlink("usr/bin", install_root.join("bin")).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    seed_test_root_layout(
        db_path_str,
        "usrmerge-conflict",
        &["/usr", "/usr/bin"],
        &[("/bin", "usr/bin")],
    );
    stage_test_boot_assets(temp_dir.path());

    let bin_content = b"from-bin".to_vec();
    let bin_hash = hash::sha256(&bin_content);
    let usr_content = b"from-usr".to_vec();
    let usr_hash = hash::sha256(&usr_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        ccs_regular_file(
            "bin/chkconfig".to_string(),
            bin_hash.clone(),
            bin_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "usr/bin/chkconfig".to_string(),
            usr_hash.clone(),
            usr_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
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
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("duplicate deployment path"),
        "unexpected error: {err:#}"
    );
}
