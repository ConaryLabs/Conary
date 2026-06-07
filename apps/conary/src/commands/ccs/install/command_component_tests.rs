// src/commands/ccs/install/command_component_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{seed_test_init_trove, stage_test_boot_assets};

#[tokio::test]
async fn ccs_install_respects_manifest_component_selection() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("custom-components.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let chosen_content = b"chosen component".to_vec();
    let chosen_hash = hash::sha256(&chosen_content);
    let skipped_content = b"skipped component".to_vec();
    let skipped_hash = hash::sha256(&skipped_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let chosen_files = vec![
        FileEntry {
            path: "/usr/bin/chosen-custom".to_string(),
            hash: chosen_hash.clone(),
            size: chosen_content.len() as u64,
            mode: 0o100755,
            component: "chosen".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "chosen".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let skipped_files = vec![FileEntry {
        path: "/usr/bin/skipped-custom".to_string(),
        hash: skipped_hash.clone(),
        size: skipped_content.len() as u64,
        mode: 0o100755,
        component: "skipped".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];
    let mut files = chosen_files.clone();
    files.extend(skipped_files.clone());
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("custom-components", "1.0.0"),
        components: HashMap::from([
            (
                "chosen".to_string(),
                ComponentData {
                    name: "chosen".to_string(),
                    files: chosen_files,
                    hash: "chosen".to_string(),
                    size: (chosen_content.len() + init_content.len()) as u64,
                },
            ),
            (
                "skipped".to_string(),
                ComponentData {
                    name: "skipped".to_string(),
                    files: skipped_files,
                    hash: "skipped".to_string(),
                    size: skipped_content.len() as u64,
                },
            ),
        ]),
        files,
        blobs: HashMap::from([
            (chosen_hash, chosen_content),
            (skipped_hash, skipped_content),
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
        Some(vec!["chosen".to_string()]),
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(!install_root.join("usr/bin/chosen-custom").exists());
    assert!(!install_root.join("usr/bin/skipped-custom").exists());

    let conn = conary_core::db::open(db_path_str).unwrap();
    let chosen_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/bin/chosen-custom'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let skipped_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/bin/skipped-custom'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(chosen_count, 1);
    assert_eq!(
        skipped_count, 0,
        "CCS install must honor selected manifest components before path classification"
    );
    let chosen_component_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM components WHERE name = 'chosen'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let skipped_component_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM components WHERE name = 'skipped'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(chosen_component_count, 1);
    assert_eq!(skipped_component_count, 0);
}

#[tokio::test]
async fn ccs_install_skips_post_install_hook_for_devel_only_component_selection() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::ScriptHook;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("devel-only.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    let hook_marker = install_root.join("var/lib/devel-only/post-install-ran");

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    seed_test_init_trove(db_path_str, temp_dir.path());

    let runtime_content = b"#!/bin/sh\necho runtime\n".to_vec();
    let runtime_hash = hash::sha256(&runtime_content);
    let devel_content = b"#pragma once\n".to_vec();
    let devel_hash = hash::sha256(&devel_content);

    let runtime_file = FileEntry {
        path: "/usr/bin/devel-only".to_string(),
        hash: runtime_hash.clone(),
        size: runtime_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let devel_file = FileEntry {
        path: "/usr/include/devel-only/api.h".to_string(),
        hash: devel_hash.clone(),
        size: devel_content.len() as u64,
        mode: 0o100644,
        component: "devel".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };

    let mut manifest = CcsManifest::new_minimal("devel-only", "1.0.0");
    manifest.hooks.post_install = Some(ScriptHook {
        script: format!(
            "mkdir -p '{}' && touch '{}'",
            hook_marker.parent().unwrap().display(),
            hook_marker.display()
        ),
    });

    let result = BuildResult {
        manifest,
        components: HashMap::from([
            (
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: vec![runtime_file.clone()],
                    hash: "runtime".to_string(),
                    size: runtime_content.len() as u64,
                },
            ),
            (
                "devel".to_string(),
                ComponentData {
                    name: "devel".to_string(),
                    files: vec![devel_file.clone()],
                    hash: "devel".to_string(),
                    size: devel_content.len() as u64,
                },
            ),
        ]),
        files: vec![runtime_file, devel_file],
        blobs: HashMap::from([(runtime_hash, runtime_content), (devel_hash, devel_content)]),
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
        Some(vec!["devel".to_string()]),
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(
        !hook_marker.exists(),
        "post-install hook should be skipped when only :devel is installed"
    );
}
