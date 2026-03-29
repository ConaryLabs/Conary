// tests/component.rs

//! Component classification and selective installation tests.

mod common;

use conary_core::db;

/// Test that the classifier correctly categorizes files into components
#[test]
fn test_component_classifier_categorization() {
    use conary_core::components::{ComponentClassifier, ComponentType};
    use std::path::Path;

    // :devel files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/include/myapp.h")),
        ComponentType::Devel,
        "Header files should be :devel"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/pkgconfig/myapp.pc")),
        ComponentType::Devel,
        "pkg-config files should be :devel"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/libmyapp.a")),
        ComponentType::Devel,
        "Static libraries should be :devel"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/cmake/myapp/myappConfig.cmake")),
        ComponentType::Devel,
        "CMake files should be :devel"
    );

    // :runtime files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/bin/myapp")),
        ComponentType::Runtime,
        "Binaries should be :runtime"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/sbin/myapp-daemon")),
        ComponentType::Runtime,
        "System binaries should be :runtime"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/share/myapp/helper.sh")),
        ComponentType::Runtime,
        "Helper scripts should be :runtime (not :data)"
    );

    // :lib files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/libmyapp.so.1")),
        ComponentType::Lib,
        "Shared libraries should be :lib"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib64/libmyapp.so")),
        ComponentType::Lib,
        "64-bit shared libraries should be :lib"
    );

    // :doc files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/share/doc/myapp/README")),
        ComponentType::Doc,
        "Documentation should be :doc"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/share/man/man1/myapp.1.gz")),
        ComponentType::Doc,
        "Man pages should be :doc"
    );

    // :config files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/etc/myapp.conf")),
        ComponentType::Config,
        "Config files should be :config"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/etc/myapp/settings.ini")),
        ComponentType::Config,
        "Nested config files should be :config"
    );
}

/// Test that default components are correctly identified
#[test]
fn test_default_component_types() {
    use conary_core::components::ComponentType;

    // Default components (installed by default)
    assert!(
        ComponentType::Runtime.is_default(),
        ":runtime should be default"
    );
    assert!(ComponentType::Lib.is_default(), ":lib should be default");
    assert!(
        ComponentType::Config.is_default(),
        ":config should be default"
    );

    // Non-default components (require explicit request)
    assert!(
        !ComponentType::Devel.is_default(),
        ":devel should NOT be default"
    );
    assert!(
        !ComponentType::Doc.is_default(),
        ":doc should NOT be default"
    );
}

/// Test scriptlet gating: scriptlets only run when :runtime or :lib is installed
#[test]
fn test_scriptlet_gating() {
    use conary_core::components::{ComponentType, should_run_scriptlets};

    // Scriptlets SHOULD run when :runtime or :lib is present
    assert!(
        should_run_scriptlets(&[ComponentType::Runtime]),
        "Scriptlets should run when installing :runtime"
    );
    assert!(
        should_run_scriptlets(&[ComponentType::Lib]),
        "Scriptlets should run when installing :lib"
    );
    assert!(
        should_run_scriptlets(&[
            ComponentType::Runtime,
            ComponentType::Lib,
            ComponentType::Config
        ]),
        "Scriptlets should run when installing defaults"
    );

    // Scriptlets should NOT run when only :devel, :doc, or :config
    assert!(
        !should_run_scriptlets(&[ComponentType::Devel]),
        "Scriptlets should NOT run when installing only :devel"
    );
    assert!(
        !should_run_scriptlets(&[ComponentType::Doc]),
        "Scriptlets should NOT run when installing only :doc"
    );
    assert!(
        !should_run_scriptlets(&[ComponentType::Devel, ComponentType::Doc]),
        "Scriptlets should NOT run when installing :devel + :doc"
    );
    // Note: :config alone also shouldn't run scriptlets (contentious, but safe)
    assert!(
        !should_run_scriptlets(&[ComponentType::Config]),
        "Scriptlets should NOT run when installing only :config"
    );
}

/// Smoke test: Simulate devel-only installation and verify correct behavior
#[test]
fn test_devel_only_install_smoke_test() {
    use conary_core::components::{ComponentClassifier, ComponentType, should_run_scriptlets};
    use std::collections::HashSet;

    // Simulate a package with files in all component types
    let package_files = vec![
        // :runtime files
        "/usr/bin/zlib-tool".to_string(),
        "/usr/sbin/zlibd".to_string(),
        // :lib files
        "/usr/lib/libz.so.1".to_string(),
        "/usr/lib/libz.so.1.2.13".to_string(),
        "/usr/lib64/libz.so".to_string(),
        // :devel files (what we want)
        "/usr/include/zlib.h".to_string(),
        "/usr/include/zconf.h".to_string(),
        "/usr/lib/pkgconfig/zlib.pc".to_string(),
        "/usr/lib/libz.a".to_string(),
        // :doc files
        "/usr/share/doc/zlib/README".to_string(),
        "/usr/share/man/man3/zlib.3.gz".to_string(),
        // :config files
        "/etc/zlib.conf".to_string(),
    ];

    // Classify all files
    let classified = ComponentClassifier::classify_all(&package_files);

    // Verify classification counts
    assert!(
        classified.contains_key(&ComponentType::Runtime),
        "Should have :runtime files"
    );
    assert!(
        classified.contains_key(&ComponentType::Lib),
        "Should have :lib files"
    );
    assert!(
        classified.contains_key(&ComponentType::Devel),
        "Should have :devel files"
    );
    assert!(
        classified.contains_key(&ComponentType::Doc),
        "Should have :doc files"
    );
    assert!(
        classified.contains_key(&ComponentType::Config),
        "Should have :config files"
    );

    // Simulate selecting ONLY :devel
    let selected_component = ComponentType::Devel;
    let selected_files: HashSet<&str> = classified
        .get(&selected_component)
        .map(|files| files.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    // Verify ONLY :devel files would be installed
    assert!(
        selected_files.contains("/usr/include/zlib.h"),
        "Header file should be selected"
    );
    assert!(
        selected_files.contains("/usr/include/zconf.h"),
        "Header file should be selected"
    );
    assert!(
        selected_files.contains("/usr/lib/pkgconfig/zlib.pc"),
        "pkg-config file should be selected"
    );
    assert!(
        selected_files.contains("/usr/lib/libz.a"),
        "Static library should be selected"
    );

    // Verify :runtime files are NOT selected
    assert!(
        !selected_files.contains("/usr/bin/zlib-tool"),
        "Binary should NOT be selected for :devel-only"
    );
    assert!(
        !selected_files.contains("/usr/sbin/zlibd"),
        "Daemon should NOT be selected for :devel-only"
    );

    // Verify :lib files are NOT selected
    assert!(
        !selected_files.contains("/usr/lib/libz.so.1"),
        "Shared library should NOT be selected for :devel-only"
    );

    // Verify :doc files are NOT selected
    assert!(
        !selected_files.contains("/usr/share/doc/zlib/README"),
        "Documentation should NOT be selected for :devel-only"
    );

    // Verify :config files are NOT selected
    assert!(
        !selected_files.contains("/etc/zlib.conf"),
        "Config should NOT be selected for :devel-only"
    );

    // CRITICAL: Verify scriptlets would NOT run for :devel-only install
    let installed_components = vec![ComponentType::Devel];
    assert!(
        !should_run_scriptlets(&installed_components),
        "Scriptlets should NOT run for :devel-only install (would likely fail without /usr/bin)"
    );
}

/// Test default installation behavior (runtime + lib + config only)
#[test]
fn test_default_install_excludes_devel_and_doc() {
    use conary_core::components::{ComponentClassifier, ComponentType};
    use std::collections::HashSet;

    // Simulate a package
    let package_files = vec![
        "/usr/bin/nginx".to_string(),                 // :runtime
        "/usr/lib/libnginx.so".to_string(),           // :lib
        "/etc/nginx/nginx.conf".to_string(),          // :config
        "/usr/include/nginx.h".to_string(),           // :devel
        "/usr/lib/pkgconfig/nginx.pc".to_string(),    // :devel
        "/usr/share/doc/nginx/README".to_string(),    // :doc
        "/usr/share/man/man8/nginx.8.gz".to_string(), // :doc
    ];

    let classified = ComponentClassifier::classify_all(&package_files);

    // Simulate default selection (runtime + lib + config)
    let default_types: HashSet<ComponentType> = [
        ComponentType::Runtime,
        ComponentType::Lib,
        ComponentType::Config,
    ]
    .into_iter()
    .collect();

    let selected_files: HashSet<&str> = classified
        .iter()
        .filter(|(comp_type, _)| default_types.contains(comp_type))
        .flat_map(|(_, files)| files.iter().map(|s| s.as_str()))
        .collect();

    // Should include defaults
    assert!(
        selected_files.contains("/usr/bin/nginx"),
        "Binary should be included"
    );
    assert!(
        selected_files.contains("/usr/lib/libnginx.so"),
        "Shared lib should be included"
    );
    assert!(
        selected_files.contains("/etc/nginx/nginx.conf"),
        "Config should be included"
    );

    // Should exclude non-defaults
    assert!(
        !selected_files.contains("/usr/include/nginx.h"),
        "Headers should be excluded"
    );
    assert!(
        !selected_files.contains("/usr/lib/pkgconfig/nginx.pc"),
        "pkg-config should be excluded"
    );
    assert!(
        !selected_files.contains("/usr/share/doc/nginx/README"),
        "Docs should be excluded"
    );
    assert!(
        !selected_files.contains("/usr/share/man/man8/nginx.8.gz"),
        "Man pages should be excluded"
    );
}

/// Test component selection with database (full integration)
#[test]
fn test_component_selective_install_database() {
    use conary_core::components::{ComponentClassifier, ComponentType};
    use conary_core::db::models::{
        Changeset, ChangesetStatus, Component, FileEntry, Trove, TroveType,
    };

    let (_dir, _path, mut conn) = common::create_test_db();

    // Simulate installing only :devel component
    let package_files = vec![
        "/usr/bin/myapp".to_string(),              // :runtime - NOT installed
        "/usr/lib/libmyapp.so".to_string(),        // :lib - NOT installed
        "/usr/include/myapp.h".to_string(),        // :devel - INSTALLED
        "/usr/lib/pkgconfig/myapp.pc".to_string(), // :devel - INSTALLED
    ];

    let classified = ComponentClassifier::classify_all(&package_files);
    let selected_component = ComponentType::Devel;

    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install myapp:devel".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new("myapp".to_string(), "1.0.0".to_string(), TroveType::Package);
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        // Only create the :devel component
        let mut devel_comp = Component::from_type(trove_id, ComponentType::Devel);
        let devel_comp_id = devel_comp.insert(tx)?;

        // Only insert files from :devel component
        if let Some(devel_files) = classified.get(&selected_component) {
            for path in devel_files {
                let mut file_entry = FileEntry::new(
                    path.clone(),
                    "fakehash123".to_string(),
                    100,
                    0o644,
                    trove_id,
                );
                file_entry.component_id = Some(devel_comp_id);
                file_entry.insert(tx)?;
            }
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    // Verify: only :devel component exists
    let troves = Trove::find_by_name(&conn, "myapp").unwrap();
    assert_eq!(troves.len(), 1);
    let trove_id = troves[0].id.unwrap();

    let components = Component::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(components.len(), 1, "Should have exactly one component");
    assert_eq!(components[0].name, "devel", "Should be :devel component");

    // Verify: only :devel files are in the database
    let files = FileEntry::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(files.len(), 2, "Should have 2 files (header + pkg-config)");

    let file_paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert!(
        file_paths.contains(&"/usr/include/myapp.h"),
        "Should have header"
    );
    assert!(
        file_paths.contains(&"/usr/lib/pkgconfig/myapp.pc"),
        "Should have pkg-config"
    );
    assert!(
        !file_paths.contains(&"/usr/bin/myapp"),
        "Should NOT have binary"
    );
    assert!(
        !file_paths.contains(&"/usr/lib/libmyapp.so"),
        "Should NOT have shared lib"
    );
}

/// Test component listing (equivalent to cmd_list_components)
#[test]
fn test_component_listing() {
    use conary_core::db::models::{Component, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Get nginx components
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap();
    let nginx_id = nginx[0].id.unwrap();
    let components = Component::find_by_trove(&conn, nginx_id).unwrap();

    assert_eq!(components.len(), 2, "nginx should have 2 components");
    let comp_names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
    assert!(comp_names.contains(&"runtime"));
    assert!(comp_names.contains(&"config"));
}

#[test]
fn test_ccs_install_components_only_installs_requested_component() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{
        BuildResult, CcsManifest, ComponentData, FileEntry as CcsFileEntry, FileType,
    };
    use conary_core::db::models::{Component, FileEntry, Trove};
    use conary_core::hash;
    use std::collections::HashMap;
    use std::process::Command;

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let package_path = temp_dir.path().join("component-fixture.ccs");

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path.to_str().unwrap()).unwrap();

    let runtime_content = b"#!/bin/sh\necho runtime\n".to_vec();
    let runtime_hash = hash::sha256(&runtime_content);
    let config_content = b"mode = \"runtime\"\n".to_vec();
    let config_hash = hash::sha256(&config_content);
    let devel_content = b"#pragma once\n".to_vec();
    let devel_hash = hash::sha256(&devel_content);

    let runtime_file = CcsFileEntry {
        path: "/usr/bin/component-fixture".to_string(),
        hash: runtime_hash.clone(),
        size: runtime_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let config_file = CcsFileEntry {
        path: "/etc/component-fixture/app.conf".to_string(),
        hash: config_hash.clone(),
        size: config_content.len() as u64,
        mode: 0o100644,
        component: "config".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let devel_file = CcsFileEntry {
        path: "/usr/include/component-fixture/api.h".to_string(),
        hash: devel_hash.clone(),
        size: devel_content.len() as u64,
        mode: 0o100644,
        component: "devel".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };

    let mut manifest = CcsManifest::new_minimal("component-fixture", "1.0.0");
    manifest.components.default = vec!["runtime".to_string(), "config".to_string()];
    manifest.config.files = vec!["/etc/component-fixture/app.conf".to_string()];

    let result = BuildResult {
        manifest,
        components: HashMap::from([
            (
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: vec![runtime_file.clone()],
                    hash: "runtime-component".to_string(),
                    size: runtime_content.len() as u64,
                },
            ),
            (
                "config".to_string(),
                ComponentData {
                    name: "config".to_string(),
                    files: vec![config_file.clone()],
                    hash: "config-component".to_string(),
                    size: config_content.len() as u64,
                },
            ),
            (
                "devel".to_string(),
                ComponentData {
                    name: "devel".to_string(),
                    files: vec![devel_file.clone()],
                    hash: "devel-component".to_string(),
                    size: devel_content.len() as u64,
                },
            ),
        ]),
        files: vec![runtime_file, config_file, devel_file],
        blobs: HashMap::from([
            (runtime_hash, runtime_content),
            (config_hash, config_content),
            (devel_hash, devel_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("ccs")
        .arg("install")
        .arg(package_path.to_str().unwrap())
        .arg("--components")
        .arg("devel")
        .arg("--allow-unsigned")
        .arg("--sandbox")
        .arg("never")
        .arg("--db-path")
        .arg(db_path.to_str().unwrap())
        .arg("--root")
        .arg(install_root.to_str().unwrap())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "ccs install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let conn = db::open(db_path.to_str().unwrap()).unwrap();
    let trove = Trove::find_by_name(&conn, "component-fixture")
        .unwrap()
        .pop()
        .unwrap();
    let trove_id = trove.id.unwrap();

    let components = Component::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(components.len(), 1, "expected only the requested component");
    assert_eq!(components[0].name, "devel");

    let files = FileEntry::find_by_trove(&conn, trove_id).unwrap();
    let file_paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(file_paths.len(), 1, "expected only one installed file");
    assert!(file_paths.contains(&"/usr/include/component-fixture/api.h"));
    assert!(!file_paths.contains(&"/usr/bin/component-fixture"));
    assert!(!file_paths.contains(&"/etc/component-fixture/app.conf"));

    assert!(install_root.join("usr/include/component-fixture/api.h").exists());
    assert!(!install_root.join("usr/bin/component-fixture").exists());
    assert!(!install_root.join("etc/component-fixture/app.conf").exists());
}
