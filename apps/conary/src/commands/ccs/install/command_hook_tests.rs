// apps/conary/src/commands/ccs/install/command_hook_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, ccs_regular_file, stage_test_boot_assets};

#[tokio::test]
async fn ccs_install_persists_pre_remove_hook() {
    use conary_core::ccs::manifest::ScriptHook;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
        ccs_regular_file(
            "/usr/bin/hooked".to_string(),
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
    let mut manifest = CcsManifest::new_minimal("pre-remove", "1.0.0");
    manifest.hooks.pre_remove = Some(ScriptHook {
        script: "echo removing pre-remove".to_string(),
        reversible: None,
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
    let (script, reversible): (String, Option<bool>) = conn
        .query_row(
            "SELECT script, reversible FROM installed_ccs_remove_hooks LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(script, "echo removing pre-remove");
    assert_eq!(reversible, None);
}

#[tokio::test]
async fn ccs_install_rolls_back_after_post_install_error() {
    use conary_core::ccs::manifest::ScriptHook;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
    let payload_file = ccs_regular_file(
        "/usr/bin/post-hook-fails".to_string(),
        hash.clone(),
        content.len() as u64,
        0o100755,
        "runtime".to_string(),
    );
    let files = vec![payload_file.clone(), init_file.clone()];

    let mut manifest = CcsManifest::new_minimal("post-hook-fails", "1.0.0");
    manifest.hooks.post_install = Some(ScriptHook {
        script: "exit 23".to_string(),
        reversible: None,
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
        total_size: 5 + init_file
            .content
            .as_ref()
            .expect("init fixture is regular")
            .size,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    let error = cmd_ccs_install(
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
        format!("{error:#}").contains("post-install hooks failed"),
        "unexpected post-install error: {error:#}"
    );

    let conn = conary_core::db::open(db_path_str).unwrap();
    let changesets: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
        .unwrap();
    let troves: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    assert_eq!(changesets, 0);
    assert_eq!(troves, 0);
    assert!(!install_root.join("usr/bin/post-hook-fails").exists());
}

#[tokio::test]
async fn ccs_install_discards_pre_hook_directories_when_post_hook_fails() {
    use conary_core::ccs::manifest::{DirectoryHook, ScriptHook};
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("revert-pre-hooks.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let file_content = b"blocked".to_vec();
    let file_hash = hash::sha256(&file_content);

    let files = vec![ccs_regular_file(
        "/usr/lib/revert-pre-hooks/persist".to_string(),
        file_hash.clone(),
        file_content.len() as u64,
        0o100644,
        "runtime".to_string(),
    )];

    let mut manifest = CcsManifest::new_minimal("revert-pre-hooks", "1.0.0");
    manifest.hooks.directories.push(DirectoryHook {
        path: "/var/lib/revert-pre-hooks".to_string(),
        mode: "0755".to_string(),
        owner: "root".to_string(),
        group: "root".to_string(),
        cleanup: None,
        reversible: None,
    });
    manifest.hooks.post_install = Some(ScriptHook {
        script: "exit 23".to_string(),
        reversible: None,
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
        format!("{err:#}").contains("post-install hooks failed"),
        "unexpected error: {err:#}"
    );
    assert!(
        !install_root.join("var/lib/revert-pre-hooks").exists(),
        "pre-hook directory should be reverted on failure"
    );
}
