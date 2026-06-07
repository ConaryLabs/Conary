// src/commands/ccs/install/command_metadata_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, seed_test_init_trove, stage_test_boot_assets};

#[tokio::test]
async fn ccs_install_records_payload_without_direct_live_root_write() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("composefs-only.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"from ccs".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let total_size = (content.len() + init_content.len()) as u64;
    let files = vec![
        FileEntry {
            path: "/usr/bin/from-ccs".to_string(),
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
        manifest: CcsManifest::new_minimal("composefs-only", "1.0.0"),
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
        blobs: HashMap::from([
            (file_hash.clone(), content.clone()),
            (init_hash, init_content),
        ]),
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
        !install_root.join("usr/bin/from-ccs").exists(),
        "CCS install must not deploy package payloads directly into the live root"
    );

    let conn = conary_core::db::open(db_path_str).unwrap();
    let stored_path: String = conn
        .query_row(
            "SELECT path FROM files WHERE path = '/usr/bin/from-ccs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_path, "/usr/bin/from-ccs");

    let current = std::fs::read_link(temp_dir.path().join("current"));
    assert!(
        current.is_ok(),
        "test-mode composefs apply must still publish an active generation pointer"
    );
}

#[tokio::test]
async fn ccs_install_strips_special_permission_bits_from_db_metadata() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("special-mode.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"setid tool".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let total_size = (content.len() + init_content.len()) as u64;
    let files = vec![
        FileEntry {
            path: "/usr/bin/setid-tool".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o106755,
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
        manifest: CcsManifest::new_minimal("special-mode", "1.0.0"),
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
        blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
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

    let conn = conary_core::db::open(db_path_str).unwrap();
    let permissions: i32 = conn
        .query_row(
            "SELECT permissions FROM files WHERE path = '/usr/bin/setid-tool'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(permissions, 0o100755);
    assert_eq!(permissions & 0o6000, 0);
}

#[tokio::test]
async fn ccs_install_persists_manifest_provides() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("manifest-provides.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![FileEntry {
        path: "/usr/sbin/init".to_string(),
        hash: init_hash.clone(),
        size: init_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];

    let mut manifest = CcsManifest::new_minimal("manifest-provides", "1.0.0");
    manifest.provides.capabilities = vec!["virtual-web-server".to_string()];
    manifest.provides.sonames = vec!["libmanifest.so.1".to_string()];
    manifest.provides.binaries = vec!["manifestctl".to_string()];
    manifest.provides.pkgconfig = vec!["manifest".to_string()];

    let result = BuildResult {
        manifest,
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
        blobs: HashMap::from([(init_hash.clone(), init_content.clone())]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };

    let package_root = temp_dir.path().join("package-root");
    let components_dir = package_root.join("components");
    let object_path = package_root
        .join("objects")
        .join(&init_hash[..2])
        .join(&init_hash[2..]);
    std::fs::create_dir_all(&components_dir).unwrap();
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    std::fs::write(
        package_root.join("MANIFEST.toml"),
        result.manifest.to_toml().unwrap(),
    )
    .unwrap();
    std::fs::write(
        components_dir.join("runtime.json"),
        serde_json::to_string_pretty(result.components.get("runtime").unwrap()).unwrap(),
    )
    .unwrap();
    std::fs::write(object_path, &init_content).unwrap();

    let output = std::fs::File::create(&package_path).unwrap();
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    archive.append_dir_all(".", &package_root).unwrap();
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap();

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
    let rows: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT kind, capability FROM provides ORDER BY kind, capability")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };

    assert!(
        rows.contains(&("package".to_string(), "virtual-web-server".to_string())),
        "manifest capability provides must be persisted"
    );
    assert!(
        rows.contains(&("soname".to_string(), "libmanifest.so.1".to_string())),
        "manifest soname provides must be persisted"
    );
    assert!(
        rows.contains(&("binary".to_string(), "manifestctl".to_string())),
        "manifest binary provides must be persisted"
    );
    assert!(
        rows.contains(&("pkgconfig".to_string(), "manifest".to_string())),
        "manifest pkgconfig provides must be persisted"
    );
}

#[tokio::test]
async fn ccs_install_persists_typed_provide_when_name_collides() {
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("collision-tool.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![FileEntry {
        path: "/usr/sbin/init".to_string(),
        hash: init_hash.clone(),
        size: init_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];

    let mut manifest = CcsManifest::new_minimal("collision-tool", "1.0.0");
    manifest.provides.binaries = vec!["collision-tool".to_string()];

    let result = BuildResult {
        manifest,
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
        blobs: HashMap::from([(init_hash.clone(), init_content.clone())]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };

    let package_root = temp_dir.path().join("package-root");
    let components_dir = package_root.join("components");
    let object_path = package_root
        .join("objects")
        .join(&init_hash[..2])
        .join(&init_hash[2..]);
    std::fs::create_dir_all(&components_dir).unwrap();
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    std::fs::write(
        package_root.join("MANIFEST.toml"),
        result.manifest.to_toml().unwrap(),
    )
    .unwrap();
    std::fs::write(
        components_dir.join("runtime.json"),
        serde_json::to_string_pretty(result.components.get("runtime").unwrap()).unwrap(),
    )
    .unwrap();
    std::fs::write(object_path, &init_content).unwrap();

    let output = std::fs::File::create(&package_path).unwrap();
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    archive.append_dir_all(".", &package_root).unwrap();
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap();

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
    let typed =
        conary_core::db::models::ProvideEntry::find_typed(&conn, "binary", "collision-tool")
            .unwrap();
    assert!(
        typed.is_some(),
        "typed manifest provide must remain resolvable when its raw capability equals the package name"
    );
}

#[tokio::test]
async fn ccs_install_registers_metadata_only_package_without_files() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest};

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("metadata-only.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    seed_test_init_trove(db_path_str, temp_dir.path());

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("metadata-only", "1.0.0"),
        components: HashMap::new(),
        files: Vec::new(),
        blobs: HashMap::new(),
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
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'metadata-only'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let file_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) \
             FROM files f \
             JOIN troves t ON t.id = f.trove_id \
             WHERE t.name = 'metadata-only'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
    assert_eq!(file_count, 0);
}

#[tokio::test]
async fn ccs_install_records_ldconfig_trigger_for_shared_libraries() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("shared-lib.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"not a real elf; trigger matching is path-based".to_vec();
    let file_hash = hash::sha256(&content);
    let (init_file, init_content, init_hash) = ccs_init_file();
    let lib_file = FileEntry {
        path: "/usr/lib64/libtrigger-test.so.1".to_string(),
        hash: file_hash.clone(),
        size: content.len() as u64,
        mode: 0o100644,
        component: "lib".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let files = vec![lib_file.clone(), init_file.clone()];

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("shared-lib", "1.0.0"),
        components: HashMap::from([
            (
                "lib".to_string(),
                ComponentData {
                    name: "lib".to_string(),
                    files: vec![lib_file],
                    hash: "lib".to_string(),
                    size: content.len() as u64,
                },
            ),
            (
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: vec![init_file],
                    hash: "runtime".to_string(),
                    size: init_content.len() as u64,
                },
            ),
        ]),
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
    let (status, matched_files): (String, i64) = conn
        .query_row(
            "SELECT ct.status, ct.matched_files \
             FROM changeset_triggers ct \
             JOIN triggers t ON t.id = ct.trigger_id \
             WHERE t.name = 'ldconfig'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(matched_files, 1);
    assert_eq!(status, "completed");
}
