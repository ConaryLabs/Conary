// conary-core/tests/derivation_e2e.rs

//! End-to-end integration tests for the derivation pipeline.
//!
//! Exercises the full derivation chain: recipe loading, derivation ID
//! computation, CAS capture, index round-trip, EROFS composition, and
//! stage assignment -- all with real data from the project's recipe files.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use conary_core::derivation::{
    DerivationId, DerivationIndex, DerivationInputs, DerivationRecord, Stage, build_script_hash,
    capture_output, compute_build_order, source_hash,
};
use conary_core::filesystem::CasStore;
use conary_core::recipe::parse_recipe_file;
use rusqlite::Connection;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the workspace root by walking up from the manifest dir.
///
/// Integration tests run with cwd set to the workspace root, so recipe paths
/// like `recipes/system/zlib.toml` resolve directly. This helper returns a
/// `Path` to the workspace root for clarity.
fn workspace_root() -> &'static Path {
    // `CARGO_MANIFEST_DIR` points to `conary-core/`; parent is workspace root.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest_dir).parent().unwrap()
}

fn setup_test_db() -> (TempDir, Connection) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    (tmp, conn)
}

// ---------------------------------------------------------------------------
// Test 1: derivation_id_from_real_recipe
// ---------------------------------------------------------------------------

#[test]
fn derivation_id_from_real_recipe() {
    let root = workspace_root();
    let recipe_path = root.join("recipes/system/zlib.toml");
    let recipe = parse_recipe_file(&recipe_path)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", recipe_path.display()));

    assert_eq!(recipe.package.name, "zlib");
    assert_eq!(recipe.package.version, "1.3.2");

    // Compute hashes from recipe content.
    let src_hash = source_hash(&recipe);
    let script_hash = build_script_hash(&recipe);

    // Both must be 64-char hex (SHA-256).
    assert_eq!(src_hash.len(), 64, "source_hash must be 64-char hex");
    assert!(
        src_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "source_hash must be valid hex"
    );
    assert_eq!(
        script_hash.len(),
        64,
        "build_script_hash must be 64-char hex"
    );
    assert!(
        script_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "build_script_hash must be valid hex"
    );

    // Build derivation inputs using the real recipe hashes.
    let inputs = DerivationInputs {
        source_hash: src_hash.clone(),
        build_script_hash: script_hash.clone(),
        dependency_ids: BTreeMap::new(),
        build_env_hash: "a".repeat(64),
        target_triple: "x86_64-conary-linux-gnu".to_owned(),
        build_options: BTreeMap::new(),
    };

    let id = DerivationId::compute(&inputs).expect("compute must succeed");
    let id_str = id.as_str();

    // Must be 64-char hex.
    assert_eq!(id_str.len(), 64, "DerivationId must be 64-char hex");
    assert!(
        id_str.chars().all(|c| c.is_ascii_hexdigit()),
        "DerivationId must be valid hex"
    );

    // Must be deterministic: same inputs produce same ID.
    let id2 = DerivationId::compute(&inputs).expect("compute must succeed");
    assert_eq!(id, id2, "DerivationId must be deterministic");
}

// ---------------------------------------------------------------------------
// Test 2: capture_and_index_round_trip
// ---------------------------------------------------------------------------

#[test]
fn capture_and_index_round_trip() {
    let tmp = TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("cas")).unwrap();

    // Build a fake DESTDIR with files and symlinks.
    let destdir = tmp.path().join("destdir");
    std::fs::create_dir_all(destdir.join("usr/lib")).unwrap();
    std::fs::create_dir_all(destdir.join("usr/bin")).unwrap();

    // Regular file.
    let bin_content = b"#!/bin/sh\necho hello world\n";
    std::fs::write(destdir.join("usr/bin/hello"), bin_content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            destdir.join("usr/bin/hello"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    // Library file.
    std::fs::write(destdir.join("usr/lib/libfoo.so.1.0"), b"ELF-fake-library").unwrap();

    // Symlink.
    std::os::unix::fs::symlink("libfoo.so.1.0", destdir.join("usr/lib/libfoo.so")).unwrap();

    // Capture the DESTDIR into CAS.
    let derivation_id = "a".repeat(64);
    let manifest =
        capture_output(&destdir, &cas, &derivation_id, 7).expect("capture_output must succeed");

    assert_eq!(manifest.files.len(), 2, "should capture 2 regular files");
    assert_eq!(manifest.symlinks.len(), 1, "should capture 1 symlink");
    assert_eq!(manifest.derivation_id, derivation_id);
    assert_eq!(manifest.build_duration_secs, 7);
    assert_eq!(manifest.output_hash.len(), 64);

    // Verify CAS contains the captured files.
    for file in &manifest.files {
        assert!(
            cas.exists(&file.hash),
            "CAS must contain file: {}",
            file.path
        );
    }

    // Serialize manifest to TOML and store in CAS.
    let manifest_toml = toml::to_string_pretty(&manifest).expect("serialize manifest");
    let manifest_cas_hash = cas
        .store(manifest_toml.as_bytes())
        .expect("store manifest in CAS");

    // Record in derivation index (SQLite).
    let (_db_tmp, conn) = setup_test_db();
    let index = DerivationIndex::new(&conn);

    let record = DerivationRecord {
        derivation_id: derivation_id.clone(),
        output_hash: manifest.output_hash.clone(),
        package_name: "test-pkg".to_owned(),
        package_version: "1.0.0".to_owned(),
        manifest_cas_hash: manifest_cas_hash.clone(),
        stage: Some("system".to_owned()),
        build_env_hash: None,
        built_at: manifest.built_at.clone(),
        build_duration_secs: manifest.build_duration_secs,
        trust_level: 0,
        provenance_cas_hash: None,
        reproducible: None,
    };

    index.insert(&record).expect("insert into index");

    // Verify cache lookup works.
    let found = index
        .lookup(&derivation_id)
        .expect("lookup must succeed")
        .expect("record must be found");

    assert_eq!(found.derivation_id, derivation_id);
    assert_eq!(found.output_hash, manifest.output_hash);
    assert_eq!(found.manifest_cas_hash, manifest_cas_hash);
    assert_eq!(found.package_name, "test-pkg");

    // Verify manifest can be loaded back from CAS.
    let retrieved_bytes = cas
        .retrieve(&manifest_cas_hash)
        .expect("retrieve manifest from CAS");
    let retrieved_toml = String::from_utf8(retrieved_bytes).expect("valid UTF-8");
    let loaded_manifest: conary_core::derivation::OutputManifest =
        toml::from_str(&retrieved_toml).expect("deserialize manifest");

    assert_eq!(loaded_manifest.derivation_id, derivation_id);
    assert_eq!(loaded_manifest.output_hash, manifest.output_hash);
    assert_eq!(loaded_manifest.files.len(), manifest.files.len());
    assert_eq!(loaded_manifest.symlinks.len(), manifest.symlinks.len());
}

// ---------------------------------------------------------------------------
// Test 3: compose_erofs_from_captured_output
// ---------------------------------------------------------------------------

#[cfg(feature = "composefs-rs")]
#[test]
fn compose_erofs_from_captured_output() {
    use conary_core::derivation::{compose_erofs, erofs_image_hash};

    let tmp = TempDir::new().unwrap();
    let cas = CasStore::new(tmp.path().join("cas")).unwrap();

    // Build a DESTDIR with files and a symlink.
    let destdir = tmp.path().join("destdir");
    std::fs::create_dir_all(destdir.join("usr/bin")).unwrap();
    std::fs::create_dir_all(destdir.join("usr/lib")).unwrap();

    std::fs::write(destdir.join("usr/bin/tool"), b"#!/bin/sh\necho tool\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            destdir.join("usr/bin/tool"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    std::fs::write(destdir.join("usr/lib/libbar.so.2.0"), b"ELF-bar-library").unwrap();
    std::os::unix::fs::symlink("libbar.so.2.0", destdir.join("usr/lib/libbar.so")).unwrap();

    // Capture to CAS.
    let drv_id = "b".repeat(64);
    let manifest = capture_output(&destdir, &cas, &drv_id, 3).expect("capture_output must succeed");

    assert!(!manifest.files.is_empty(), "must have captured files");
    assert!(!manifest.symlinks.is_empty(), "must have captured symlinks");

    // Compose an EROFS image from the captured output.
    let output_dir = tmp.path().join("erofs_output");
    std::fs::create_dir_all(&output_dir).unwrap();

    let build_result =
        compose_erofs(&[&manifest], &output_dir).expect("compose_erofs must succeed");

    // Verify image exists and has non-zero size.
    assert!(
        build_result.image_path.exists(),
        "EROFS image must exist at {}",
        build_result.image_path.display()
    );
    assert!(
        build_result.image_size > 0,
        "EROFS image must have non-zero size"
    );

    // Compute the image hash.
    let img_hash =
        erofs_image_hash(&build_result.image_path).expect("erofs_image_hash must succeed");

    assert_eq!(img_hash.len(), 64, "image hash must be 64-char hex");
    assert!(
        img_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "image hash must be valid hex"
    );

    // Hash must be deterministic (same image bytes).
    let img_hash2 =
        erofs_image_hash(&build_result.image_path).expect("erofs_image_hash must succeed");
    assert_eq!(img_hash, img_hash2, "image hash must be deterministic");
}

// ---------------------------------------------------------------------------
// Test 4: stage_assignment_with_real_recipes
// ---------------------------------------------------------------------------

#[test]
fn stage_assignment_with_real_recipes() {
    let root = workspace_root();

    // Load a few real recipes from the project.
    let recipe_names = ["zlib", "xz", "zstd"];
    let mut recipes = HashMap::new();

    for name in &recipe_names {
        let path = root.join(format!("recipes/system/{name}.toml"));
        let recipe = parse_recipe_file(&path)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
        recipes.insert(name.to_string(), recipe);
    }

    assert_eq!(recipes.len(), 3, "should have loaded 3 recipes");

    // Run build ordering with no custom packages.
    let custom = HashSet::new();
    let steps = compute_build_order(&recipes, &custom).expect("compute_build_order must succeed");

    assert_eq!(
        steps.len(),
        3,
        "should have 3 build steps, got {}",
        steps.len()
    );

    // zlib and xz are in FOUNDATION_NAMED. zstd is not, so it goes to System.
    for step in &steps {
        match step.package.as_str() {
            "zlib" | "xz" => {
                assert_eq!(
                    step.stage,
                    Stage::Foundation,
                    "{} should be in Foundation",
                    step.package
                );
            }
            "zstd" => {
                assert_eq!(step.stage, Stage::System, "zstd should be in System");
            }
            other => panic!("unexpected package in build steps: {other}"),
        }
    }

    // Build orders should be unique.
    let orders: Vec<usize> = steps.iter().map(|s| s.order).collect();
    let unique: HashSet<usize> = orders.iter().copied().collect();
    assert_eq!(orders.len(), unique.len(), "build orders must be unique");
}
