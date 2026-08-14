// apps/conary/src/commands/generation/gc/tests.rs

use super::super::selected_root::persist_captured_publication_snapshot;
use super::*;
use crate::commands::{
    FileSnapshot, RollbackSystemAuthority, TroveSnapshot, metadata_with_removed_troves,
};
use conary_core::db::models::{Changeset, ChangesetStatus};
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    SelectedRootSnapshot,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use tempfile::TempDir;

fn fixture() -> (TempDir, PathBuf, Connection, ConaryRuntimeRoot, CasStore) {
    let temporary = TempDir::new().unwrap();
    let db_path = temporary.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path.clone());
    let cas = CasStore::new(runtime_root.objects_dir()).unwrap();
    (temporary, db_path, conn, runtime_root, cas)
}

fn directory_node() -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(0o755);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | 0o755;
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn regular_node(mode: u32) -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(mode);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn directory(path: &str) -> GenerationRootEntry {
    GenerationRootEntry {
        path: path.to_string(),
        node: directory_node(),
        content: None,
    }
}

fn selected_root_with_mutable_file(hash: &str, size: u64) -> CapturedSelectedRoot {
    CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: directory_node(),
            entries: Vec::new(),
        },
        state: MutableStateManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            entries: vec![
                directory("/var"),
                directory("/var/lib"),
                directory("/var/lib/lifecycle-fixture"),
                GenerationRootEntry {
                    path: "/var/lib/lifecycle-fixture/state".to_string(),
                    node: regular_node(0o600),
                    content: Some(PayloadContentAuthority {
                        sha256: hash.to_string(),
                        size,
                    }),
                },
            ],
        },
    }
}

fn empty_selected_root() -> CapturedSelectedRoot {
    CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: directory_node(),
            entries: Vec::new(),
        },
        state: MutableStateManifest::empty(),
    }
}

fn object_path(runtime_root: &ConaryRuntimeRoot, hash: &str) -> PathBuf {
    runtime_root.objects_dir().join(&hash[..2]).join(&hash[2..])
}

fn make_object_older_than_gc_grace(path: &Path) {
    let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
    let timestamps = [
        libc::timespec {
            tv_sec: 1,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: 1,
            tv_nsec: 0,
        },
    ];
    // SAFETY: `raw` is NUL-terminated, `timestamps` contains the two entries
    // required by `utimensat`, and both buffers outlive the call.
    let result = unsafe { libc::utimensat(libc::AT_FDCWD, raw.as_ptr(), timestamps.as_ptr(), 0) };
    assert_eq!(result, 0, "failed to age CAS fixture object");
}

fn insert_rollback_bearing_mutation(
    conn: &Connection,
    description: &str,
    path: &str,
    hash: &str,
    size: u64,
) -> i64 {
    let snapshot = TroveSnapshot::test_package(
        description,
        "1",
        vec![FileSnapshot {
            path: path.to_string(),
            node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap(),
            content: Some(PayloadContentAuthority {
                sha256: hash.to_string(),
                size,
            }),
            installed_at: "2026-01-01T00:00:00Z".to_string(),
            component: None,
        }],
    );
    let root = SelectedRootSnapshot::capture(conn, &empty_selected_root()).unwrap();
    let metadata = metadata_with_removed_troves(
        vec![snapshot],
        Vec::new(),
        root,
        RollbackSystemAuthority::default(),
    )
    .unwrap();
    let mut changeset = Changeset::new(description.to_string());
    let id = changeset.insert(conn).unwrap();
    conn.execute(
        "UPDATE changesets
         SET metadata = ?1, rollback_selected_root_snapshot_id = ?2
         WHERE id = ?3",
        rusqlite::params![metadata, root.id(), id],
    )
    .unwrap();
    changeset
        .update_status(conn, ChangesetStatus::Applied)
        .unwrap();
    id
}

fn complete_rollback_lineage(conn: &Connection, target: i64) {
    let mut rollback = Changeset::new_rollback(format!("rollback {target}"), target);
    let rollback_id = rollback.insert(conn).unwrap();
    rollback
        .update_status(conn, ChangesetStatus::Applied)
        .unwrap();
    conn.execute(
        "UPDATE changesets
         SET status = 'rolled_back',
             rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1
         WHERE id = ?2",
        rusqlite::params![rollback_id, target],
    )
    .unwrap();
}

#[test]
fn lifecycle_only_mutable_state_content_survives_generation_gc() {
    let (_temporary, db_path, _conn, runtime_root, cas) = fixture();
    let bytes = b"lifecycle-created mutable state";
    let hash = cas.store(bytes).unwrap();
    let generation = runtime_root.generation_path(1);
    std::fs::create_dir_all(&generation).unwrap();
    let root = selected_root_with_mutable_file(&hash, bytes.len() as u64);
    root.generation.write_to(&generation).unwrap();
    root.state.write_to(&generation).unwrap();
    let path = object_path(&runtime_root, &hash);
    make_object_older_than_gc_grace(&path);

    cmd_generation_gc_locked(1, db_path.to_str().unwrap(), &runtime_root).unwrap();

    assert!(
        path.exists(),
        "mutable-state manifest content must remain live"
    );
}

#[test]
fn old_recoverable_publication_snapshot_content_survives_generation_gc() {
    let (_temporary, db_path, conn, runtime_root, cas) = fixture();
    let bytes = b"recoverable snapshot state";
    let hash = cas.store(bytes).unwrap();
    let root = selected_root_with_mutable_file(&hash, bytes.len() as u64);
    let debt = GenerationPublication::create_pending(
        &conn,
        None,
        None,
        db_path.to_str().unwrap(),
        runtime_root.root().to_str().unwrap(),
        "fixture",
        &Default::default(),
    )
    .unwrap();
    persist_captured_publication_snapshot(&conn, &debt, &root).unwrap();
    let path = object_path(&runtime_root, &hash);
    make_object_older_than_gc_grace(&path);

    cmd_generation_gc_locked(0, db_path.to_str().unwrap(), &runtime_root).unwrap();

    assert!(
        path.exists(),
        "recoverable publication authority must override object age"
    );
}

#[test]
fn rollback_only_content_survives_generation_gc() {
    let (_temporary, db_path, conn, runtime_root, cas) = fixture();
    let bytes = b"removed package rollback content";
    let hash = cas.store(bytes).unwrap();
    let snapshot = TroveSnapshot::test_package(
        "removed",
        "1",
        vec![FileSnapshot {
            path: "/usr/bin/removed".to_string(),
            node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap(),
            content: Some(PayloadContentAuthority {
                sha256: hash.clone(),
                size: bytes.len() as u64,
            }),
            installed_at: "2026-01-01T00:00:00Z".to_string(),
            component: None,
        }],
    );
    let root = SelectedRootSnapshot::capture(&conn, &empty_selected_root()).unwrap();
    let metadata = metadata_with_removed_troves(
        vec![snapshot],
        Vec::new(),
        root,
        RollbackSystemAuthority::default(),
    )
    .unwrap();
    let mut changeset = Changeset::new("remove fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets
         SET metadata = ?1, rollback_selected_root_snapshot_id = ?2
         WHERE id = ?3",
        rusqlite::params![metadata, root.id(), changeset_id],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    let path = object_path(&runtime_root, &hash);
    make_object_older_than_gc_grace(&path);

    cmd_generation_gc_locked(0, db_path.to_str().unwrap(), &runtime_root).unwrap();

    assert!(
        path.exists(),
        "effective rollback authority must remain live"
    );
}

#[test]
fn every_effective_stack_entry_survives_until_its_lifo_rollback() {
    let (_temporary, db_path, conn, runtime_root, cas) = fixture();
    let c1_bytes = b"first rollback frame";
    let c2_bytes = b"second rollback frame";
    let c1_hash = cas.store(c1_bytes).unwrap();
    let c2_hash = cas.store(c2_bytes).unwrap();
    let c1 = insert_rollback_bearing_mutation(
        &conn,
        "c1",
        "/usr/bin/c1",
        &c1_hash,
        c1_bytes.len() as u64,
    );
    let c2 = insert_rollback_bearing_mutation(
        &conn,
        "c2",
        "/usr/bin/c2",
        &c2_hash,
        c2_bytes.len() as u64,
    );
    let c1_path = object_path(&runtime_root, &c1_hash);
    let c2_path = object_path(&runtime_root, &c2_hash);
    make_object_older_than_gc_grace(&c1_path);
    make_object_older_than_gc_grace(&c2_path);

    cmd_generation_gc_locked(0, db_path.to_str().unwrap(), &runtime_root).unwrap();
    assert!(c1_path.exists(), "C1 must remain below the active C2 frame");
    assert!(c2_path.exists(), "C2 must remain rollback eligible");

    complete_rollback_lineage(&conn, c2);
    cmd_generation_gc_locked(0, db_path.to_str().unwrap(), &runtime_root).unwrap();
    assert!(
        c1_path.exists(),
        "C1 must still be usable after C2 has been rolled back"
    );
    assert_eq!(cas.retrieve(&c1_hash).unwrap(), c1_bytes);

    complete_rollback_lineage(&conn, c1);
}

#[test]
fn missing_surviving_manifest_aborts_before_any_deletion() {
    let (_temporary, db_path, _conn, runtime_root, cas) = fixture();
    let kept = runtime_root.generation_path(2);
    let removable = runtime_root.generation_path(1);
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::create_dir_all(&removable).unwrap();
    let dead_hash = cas.store(b"would otherwise be collected").unwrap();
    let dead_path = object_path(&runtime_root, &dead_hash);
    make_object_older_than_gc_grace(&dead_path);

    let error = cmd_generation_gc_locked(1, db_path.to_str().unwrap(), &runtime_root)
        .expect_err("missing surviving authority must abort");

    assert!(error.to_string().contains("refusing generation GC"));
    assert!(kept.exists());
    assert!(removable.exists());
    assert!(dead_path.exists());
}

#[test]
fn missing_live_cas_object_aborts_before_any_deletion() {
    let (_temporary, db_path, _conn, runtime_root, cas) = fixture();
    let kept = runtime_root.generation_path(2);
    let removable = runtime_root.generation_path(1);
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::create_dir_all(&removable).unwrap();
    let missing_hash = "d".repeat(64);
    let root = selected_root_with_mutable_file(&missing_hash, 12);
    root.generation.write_to(&kept).unwrap();
    root.state.write_to(&kept).unwrap();
    let dead_hash = cas.store(b"would otherwise be collected").unwrap();
    let dead_path = object_path(&runtime_root, &dead_hash);
    make_object_older_than_gc_grace(&dead_path);

    let error = cmd_generation_gc_locked(1, db_path.to_str().unwrap(), &runtime_root)
        .expect_err("missing live object must abort");

    assert!(format!("{error:#}").contains("missing object"));
    assert!(kept.exists());
    assert!(removable.exists());
    assert!(dead_path.exists());
}

#[test]
fn malformed_effective_changeset_metadata_aborts_before_any_deletion() {
    let (_temporary, db_path, conn, runtime_root, cas) = fixture();
    let removable = runtime_root.generation_path(1);
    std::fs::create_dir_all(&removable).unwrap();
    let dead_hash = cas.store(b"would otherwise be collected").unwrap();
    let dead_path = object_path(&runtime_root, &dead_hash);
    make_object_older_than_gc_grace(&dead_path);
    let mut changeset = Changeset::new("corrupt fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = 'not-json' WHERE id = ?1",
        [changeset_id],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let error = cmd_generation_gc_locked(0, db_path.to_str().unwrap(), &runtime_root)
        .expect_err("malformed current metadata must abort");

    assert!(
        error
            .to_string()
            .contains("invalid current rollback authority")
    );
    assert!(removable.exists());
    assert!(dead_path.exists());
}
