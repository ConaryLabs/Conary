// src/commands/ccs/install/command_hook_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, stage_test_boot_assets};

#[tokio::test]
async fn ccs_install_persists_pre_remove_hook() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::ScriptHook;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("pre-remove.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"hooked payload".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "/usr/bin/hooked".to_string(),
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
    let mut manifest = CcsManifest::new_minimal("pre-remove", "1.0.0");
    manifest.hooks.pre_remove = Some(ScriptHook {
        script: "echo removing pre-remove".to_string(),
    });
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
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

    let conn = conary_core::db::open(db_path_str).unwrap();
    let (phase, content, package_format): (String, String, String) = conn
        .query_row(
            "SELECT phase, content, package_format FROM scriptlets LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(phase, "pre-remove");
    assert_eq!(content, "echo removing pre-remove");
    assert_eq!(package_format, "ccs");
}

#[tokio::test]
async fn ccs_install_marks_changeset_post_hooks_failed_after_post_install_error() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::ScriptHook;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("post-hook-fails.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"hello".to_vec();
    let hash = hash::sha256(&content);
    let (init_file, init_content, init_hash) = ccs_init_file();
    let payload_file = FileEntry {
        path: "/usr/bin/post-hook-fails".to_string(),
        hash: hash.clone(),
        size: content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let files = vec![payload_file.clone(), init_file.clone()];

    let mut manifest = CcsManifest::new_minimal("post-hook-fails", "1.0.0");
    manifest.hooks.post_install = Some(ScriptHook {
        script: "exit 23".to_string(),
    });

    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(hash, content), (init_hash, init_content)]),
        total_size: 5 + init_file.size,
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
    let (status, description): (String, String) = conn
        .query_row(
            "SELECT status, description FROM changesets ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "post_hooks_failed");
    assert!(!description.contains("[post-hooks failed]"));
}

#[tokio::test]
async fn ccs_install_reverts_pre_hook_directories_when_deploy_fails() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::DirectoryHook;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let outside_root = temp_dir.path().join("outside");
    let package_path = temp_dir.path().join("revert-pre-hooks.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    std::fs::create_dir_all(&outside_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();

    let file_content = b"blocked".to_vec();
    let file_hash = hash::sha256(&file_content);

    let files = vec![FileEntry {
        path: "/usr/lib/link/cron.d/persist".to_string(),
        hash: file_hash.clone(),
        size: file_content.len() as u64,
        mode: 0o100644,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];

    let mut manifest = CcsManifest::new_minimal("revert-pre-hooks", "1.0.0");
    manifest.hooks.directories.push(DirectoryHook {
        path: "/var/lib/revert-pre-hooks".to_string(),
        mode: "0755".to_string(),
        owner: "root".to_string(),
        group: "root".to_string(),
        cleanup: None,
    });

    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: file_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash, file_content)]),
        total_size: 7,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();
    std::fs::create_dir_all(install_root.join("usr/lib")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_root, install_root.join("usr/lib/link")).unwrap();

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
    assert!(
        !install_root.join("var/lib/revert-pre-hooks").exists(),
        "pre-hook directory should be reverted on failure"
    );
}
