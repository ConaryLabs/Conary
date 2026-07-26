// conary/tests/component.rs

//! Explicit component-contract and selective-install integration tests.

mod common;

use conary_core::db;

#[test]
fn component_type_names_are_exact_metadata_values() {
    use conary_core::components::ComponentType;

    for component in ComponentType::all() {
        assert_eq!(ComponentType::parse(component.as_str()), Some(*component));
    }
    assert_eq!(ComponentType::parse("shared-library"), None);
    assert_eq!(ComponentType::parse("Runtime"), None);
}

#[test]
fn component_listing_reads_persisted_component_authority() {
    use conary_core::db::models::{Component, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let nginx = Trove::find_by_name(&conn, "nginx").unwrap();
    let nginx_id = nginx[0].id.unwrap();
    let components = Component::find_by_trove(&conn, nginx_id).unwrap();

    assert_eq!(components.len(), 2, "nginx should have 2 components");
    let comp_names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
    assert!(comp_names.contains(&"runtime"));
    assert!(comp_names.contains(&"config"));
}

#[test]
fn ccs_archive_preserves_exact_author_component_membership() {
    use conary_core::ccs::builder::write_signed_current_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry as CcsFileEntry};
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};
    use std::collections::HashMap;

    fn regular_file(path: &str, component: &str, mode: u32, content: &[u8]) -> CcsFileEntry {
        CcsFileEntry {
            path: path.to_string(),
            node: PayloadNode::regular(mode),
            content: Some(PayloadContentAuthority {
                sha256: conary_core::hash::sha256(content),
                size: content.len() as u64,
            }),
            component: component.to_string(),
            chunks: None,
        }
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let package_path = temp_dir.path().join("component-fixture.ccs");

    let runtime_content = b"#!/bin/sh\necho runtime\n".to_vec();
    let config_content = b"mode = \"runtime\"\n".to_vec();
    let devel_content = b"#pragma once\n".to_vec();
    let runtime_file = regular_file(
        "/usr/bin/component-fixture",
        "runtime",
        0o755,
        &runtime_content,
    );
    let config_file = regular_file(
        "/etc/component-fixture/app.conf",
        "config",
        0o644,
        &config_content,
    );
    let devel_file = regular_file(
        "/usr/include/component-fixture/api.h",
        "devel",
        0o644,
        &devel_content,
    );

    let mut manifest = CcsManifest::new_minimal("component-fixture", "1.0.0");
    manifest.components.default = vec!["runtime".to_string()];
    manifest.config.files = vec![conary_core::packages::traits::ConfigFileInfo {
        path: "/etc/component-fixture/app.conf".to_string(),
        noreplace: true,
        ghost: false,
        remove_on_upgrade: false,
    }];

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
            (conary_core::hash::sha256(&runtime_content), runtime_content),
            (conary_core::hash::sha256(&config_content), config_content),
            (conary_core::hash::sha256(&devel_content), devel_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = conary_core::ccs::SigningKeyPair::generate();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, false).unwrap();
    let verified = conary_core::ccs::verify::verify_package(
        &package_path,
        &conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .unwrap();
    let package = conary_core::ccs::CcsPackage::from_verified_archive(
        package_path.to_str().unwrap(),
        &verified,
    )
    .unwrap();
    let devel = &package.components()["devel"];
    assert_eq!(devel.files.len(), 1);
    assert_eq!(devel.files[0].path, "/usr/include/component-fixture/api.h");
    assert_eq!(devel.files[0].component, "devel");
    assert_eq!(package.components()["runtime"].files.len(), 1);
    assert_eq!(package.components()["config"].files.len(), 1);
}
