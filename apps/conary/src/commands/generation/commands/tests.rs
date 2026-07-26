// apps/conary/src/commands/generation/commands/tests.rs
//! Tests for generation command parsing, GC, publication, and rendering.

use super::super::gc::{
    booted_generation_from_cmdline, load_gc_roots, parse_gc_root_setting,
    runtime_root_for_generation_db_path,
};
use super::{
    classify_side_effect_reasons, removed_members_for_side_effect_warning, render_generation_info,
};
use conary_core::db::models::settings;
use conary_core::db::models::{StateDiff, StateMember};
use conary_core::db::schema;
use conary_core::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
use rusqlite::Connection;
use tempfile::TempDir;

fn member(name: &str, version: &str) -> StateMember {
    StateMember {
        id: None,
        state_id: 1,
        trove_name: name.to_string(),
        trove_version: version.to_string(),
        package_release: None,
        architecture: Some("x86_64".to_string()),
        install_reason: "explicit".to_string(),
        selection_reason: None,
    }
}

#[test]
fn classify_side_effect_reasons_detects_all_requested_categories() {
    let reasons = classify_side_effect_reasons(
        [
            "/usr/lib/systemd/system/example.service",
            "/etc/cron.d/example",
            "/usr/lib/sysusers.d/example.conf",
        ],
        [
            "groupadd example",
            "systemctl preset example.service",
            "crontab -r",
        ],
    );

    assert_eq!(reasons, vec!["users/groups", "systemd units", "cron jobs"]);
}

#[test]
fn removed_members_for_side_effect_warning_includes_replaced_versions_once() {
    let removed = member("removed-only", "1.0.0");
    let upgraded_old = member("replaced", "2.0.0");
    let upgraded_new = member("replaced", "1.5.0");
    let diff = StateDiff {
        added: Vec::new(),
        removed: vec![removed.clone()],
        upgraded: vec![
            (upgraded_old.clone(), upgraded_new),
            (upgraded_old.clone(), member("replaced", "1.0.0")),
        ],
    };

    let members = removed_members_for_side_effect_warning(&diff);
    let rendered: Vec<_> = members
        .into_iter()
        .map(|member| (member.trove_name, member.trove_version))
        .collect();
    assert_eq!(
        rendered,
        vec![
            ("removed-only".to_string(), "1.0.0".to_string()),
            ("replaced".to_string(), "2.0.0".to_string()),
        ]
    );
}

#[test]
fn parse_gc_root_setting_sorts_and_deduplicates_values() {
    assert_eq!(parse_gc_root_setting("[7,3,7,5]").unwrap(), vec![3, 5, 7]);
}

#[test]
fn parse_gc_root_setting_rejects_corrupt_pin_authority() {
    let error = parse_gc_root_setting("not-json")
        .expect_err("corrupt GC roots must not become an empty pin set");
    assert!(
        error
            .to_string()
            .contains("refusing to discard pin authority")
    );
}

#[test]
fn load_gc_roots_ignores_filesystem_entries_without_db_registration() {
    let temp_dir = TempDir::new().unwrap();
    let gc_roots_dir = temp_dir.path().join("gc-roots");
    std::fs::create_dir_all(&gc_roots_dir).unwrap();
    std::fs::write(gc_roots_dir.join("7"), b"").unwrap();

    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();

    assert_eq!(load_gc_roots(&conn).unwrap(), Vec::<i64>::new());

    settings::set(&conn, "generation.gc_roots", "[7,5]").unwrap();
    assert_eq!(load_gc_roots(&conn).unwrap(), vec![5, 7]);
}

#[test]
fn default_generation_db_path_uses_canonical_runtime_root() {
    let runtime_root = runtime_root_for_generation_db_path("/var/lib/conary/conary.db");

    assert_eq!(runtime_root.root(), std::path::Path::new("/conary"));
    assert_eq!(
        runtime_root.generations_dir(),
        std::path::PathBuf::from("/conary/generations")
    );
    assert_eq!(
        runtime_root.objects_dir(),
        std::path::PathBuf::from("/conary/objects")
    );
}

#[test]
fn test_generation_db_path_keeps_generation_state_self_contained() {
    let runtime_root = runtime_root_for_generation_db_path("/tmp/conary-test/conary.db");

    assert_eq!(
        runtime_root.root(),
        std::path::Path::new("/tmp/conary-test")
    );
    assert_eq!(
        runtime_root.gc_roots_dir(),
        std::path::PathBuf::from("/tmp/conary-test/gc-roots")
    );
}

#[test]
fn booted_generation_ignores_missing_generation_directory() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::create_dir_all(temp_dir.path().join("generations")).unwrap();

    assert_eq!(
        booted_generation_from_cmdline("quiet conary.generation=7", temp_dir.path()).unwrap(),
        None
    );
}

#[test]
fn booted_generation_accepts_existing_generation_directory() {
    let temp_dir = TempDir::new().unwrap();
    let gen_dir = temp_dir.path().join("generations/7");
    std::fs::create_dir_all(&gen_dir).unwrap();

    assert_eq!(
        booted_generation_from_cmdline("quiet conary.generation=7", temp_dir.path()).unwrap(),
        Some(7)
    );
}

#[test]
fn booted_generation_rejects_malformed_kernel_authority() {
    let temp_dir = TempDir::new().unwrap();
    let error =
        booted_generation_from_cmdline("quiet conary.generation=not-a-number", temp_dir.path())
            .expect_err("malformed boot authority must stop generation GC");
    assert!(
        error
            .to_string()
            .contains("Invalid conary.generation value")
    );
}

#[test]
fn generation_info_reports_capability_xattr_count_when_present() {
    let metadata = GenerationMetadata {
        generation: 7,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(4096),
        cas_objects_referenced: Some(2),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: Some(3),
        created_at: "2026-07-08T00:00:00Z".to_string(),
        package_count: 2,
        kernel_version: Some("6.19.8-conary".to_string()),
        summary: "fixture".to_string(),
    };

    let rendered = render_generation_info(7, &metadata, false, 4096);

    assert!(rendered.contains("  Cap xattrs: 3"));
}

#[test]
fn generation_info_omits_capability_xattr_count_when_zero() {
    let metadata = GenerationMetadata {
        generation: 7,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(4096),
        cas_objects_referenced: Some(2),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: Some(0),
        created_at: "2026-07-08T00:00:00Z".to_string(),
        package_count: 2,
        kernel_version: Some("6.19.8-conary".to_string()),
        summary: "fixture".to_string(),
    };

    let rendered = render_generation_info(7, &metadata, false, 4096);

    assert!(!rendered.contains("Cap xattrs"));
}
