// apps/conary/src/commands/generation/selected_root/tests.rs

use super::*;
use conary_core::db::models::{FileEntry, GenerationPublicationStatus, Trove, TroveType};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};

fn resolved_regular(mode: u32) -> ResolvedPayloadNode {
    let mut source = PayloadNode::regular(mode & 0o7777);
    source.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    source.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(source).unwrap()
}

fn resolved_directory(mode: u32) -> ResolvedPayloadNode {
    let mut source = PayloadNode::regular(mode & 0o7777);
    source.kind = PayloadNodeKind::Directory;
    source.mode = libc::S_IFDIR | (mode & 0o7777);
    source.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    source.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    ResolvedPayloadNode::from_numeric_source(source).unwrap()
}

fn live_regular(path: &str, content: &[u8], mode: u32) -> LiveRootFile {
    LiveRootFile {
        path: path.to_string(),
        content: crate::commands::LiveRootContent::from_in_memory_bytes(content),
        node: resolved_regular(mode),
    }
}

#[test]
fn no_current_generation_materializes_authoritative_db_state_into_selected_root() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
    let cas = conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap();
    let hash = cas.store(b"package-a").unwrap();
    let mut trove = Trove::new(
        "package-a".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    for path in ["/usr", "/usr/lib"] {
        FileEntry::new(path.to_string(), resolved_directory(0o755), None, trove_id)
            .insert(&conn)
            .unwrap();
    }
    FileEntry::new(
        "/usr/lib/package-a".to_string(),
        resolved_regular(0o644),
        Some(PayloadContentAuthority {
            sha256: hash,
            size: 9,
        }),
        trove_id,
    )
    .insert(&conn)
    .unwrap();
    assert!(
        conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .is_none()
    );

    let mut session =
        SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "batch graph").unwrap();
    assert_eq!(
        fs::read_to_string(session.selected_root().join("usr/lib/package-a")).unwrap(),
        "package-a"
    );
    session
        .apply_install_files(&[live_regular("/usr/lib/package-b", b"package-b", 0o644)])
        .unwrap();
    assert_eq!(
        fs::read_to_string(session.selected_root().join("usr/lib/package-b")).unwrap(),
        "package-b"
    );
    let (_, captured) = session.capture_preserving_root(&runtime_root).unwrap();
    assert_eq!(
        captured
            .generation
            .entries
            .iter()
            .find(|entry| entry.path == "/usr/lib/package-b")
            .and_then(|entry| entry.content.as_ref())
            .map(|content| content.size),
        Some(9)
    );
}

#[test]
fn rollback_authority_is_the_immutable_prior_not_a_mutated_root_scan() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut session =
        SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "rollback prior").unwrap();
    let before = session.capture_rollback_authority().unwrap();
    session
        .apply_install_files(&[live_regular("/opt/new", b"new", 0o644)])
        .unwrap();

    assert_eq!(session.capture_rollback_authority().unwrap(), before);
    assert!(
        !before
            .materialize(&conn)
            .unwrap()
            .generation
            .entries
            .iter()
            .any(|entry| entry.path == "/opt/new")
    );
}

#[test]
fn selected_root_snapshot_is_retryable_after_publication_becomes_terminal() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
    let mut session =
        SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "selected root").unwrap();
    let debt = GenerationPublication::create_pending(
        &conn,
        None,
        None,
        db_path.to_str().unwrap(),
        &runtime_root.root().display().to_string(),
        "selected root",
        &Default::default(),
    )
    .unwrap();
    session
        .apply_install_files(&[live_regular(
            "/opt/lifecycle-created",
            b"exact lifecycle output",
            0o640,
        )])
        .unwrap();

    session
        .persist_for_publication(&conn, &runtime_root, &debt)
        .unwrap();
    let captured = load_publication_selected_root(&conn, &debt).unwrap();
    let entry = captured
        .generation
        .entries
        .iter()
        .find(|entry| entry.path == "/opt/lifecycle-created")
        .unwrap();
    assert_eq!(entry.content.as_ref().map(|content| content.size), Some(22));

    debt.set_phase(
        &conn,
        conary_core::db::models::GenerationPublicationPhase::DatabaseBackedUp,
        GenerationPublicationStatus::Running,
        Some(1),
        Some(1),
    )
    .unwrap();
    debt.mark_complete_through(&conn, None, 1, 1).unwrap();
    assert!(load_publication_selected_root(&conn, &debt).is_ok());
}

#[test]
fn later_selected_root_snapshot_cumulates_prior_pending_effects() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);

    let mut first = SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "first").unwrap();
    first
        .apply_install_files(&[live_regular("/opt/first-effect", b"first", 0o644)])
        .unwrap();
    let first_debt = GenerationPublication::create_pending(
        &conn,
        None,
        None,
        db_path.to_str().unwrap(),
        &runtime_root.root().display().to_string(),
        "first",
        &Default::default(),
    )
    .unwrap();
    first
        .persist_for_publication(&conn, &runtime_root, &first_debt)
        .unwrap();
    first_debt
        .mark_failed(&conn, "forced first publication failure")
        .unwrap();
    drop(first);

    let mut second =
        SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "second").unwrap();
    assert_eq!(
        fs::read(second.selected_root().join("opt/first-effect")).unwrap(),
        b"first"
    );
    second
        .apply_install_files(&[live_regular("/opt/second-effect", b"second", 0o644)])
        .unwrap();
    let second_debt = GenerationPublication::create_pending(
        &conn,
        None,
        None,
        db_path.to_str().unwrap(),
        &runtime_root.root().display().to_string(),
        "second",
        &Default::default(),
    )
    .unwrap();
    second
        .persist_for_publication(&conn, &runtime_root, &second_debt)
        .unwrap();

    let latest = load_publication_selected_root(&conn, &second_debt).unwrap();
    let paths = latest
        .generation
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(paths.contains("/opt/first-effect"));
    assert!(paths.contains("/opt/second-effect"));

    let outcome = crate::commands::generation::publication::retry_pending_publication(
        &conn,
        db_path.to_str().unwrap(),
        "retry cumulative selected root",
    )
    .unwrap();
    assert!(outcome.needs_publication);
    let retried = GenerationPublication::find_by_id(&conn, second_debt.id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(retried.status, GenerationPublicationStatus::Failed);
    assert_eq!(retried.retry_count, 1);
    let still_retryable = load_publication_selected_root(&conn, &retried).unwrap();
    let retry_paths = still_retryable
        .generation
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(retry_paths.contains("/opt/first-effect"));
    assert!(retry_paths.contains("/opt/second-effect"));
}

#[test]
fn mutation_lock_prevents_stale_root_and_preserves_root_db_parity() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
    let mut first =
        SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "first writer").unwrap();
    first
        .apply_install_files(&[live_regular("/opt/serialized-effect", b"serialized", 0o644)])
        .unwrap();

    let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
    let thread_db_path = db_path.clone();
    let waiter = std::thread::spawn(move || {
        let waiter_conn = conary_core::db::open(&thread_db_path).unwrap();
        attempt_tx.send(()).unwrap();
        let session = SelectedRootSession::begin(
            &waiter_conn,
            thread_db_path.to_str().unwrap(),
            "second writer",
        );
        let result = session
            .and_then(|session| {
                let root_bytes = fs::read(session.selected_root().join("opt/serialized-effect"))?;
                let db_entry = FileEntry::find_by_path(&waiter_conn, "/opt/serialized-effect")?
                    .context("serialized root effect has no committed DB authority")?;
                Ok((root_bytes, db_entry.content))
            })
            .map_err(|error| error.to_string());
        result_tx.send(result).unwrap();
    });
    attempt_rx.recv().unwrap();
    assert!(
        matches!(
            result_rx.recv_timeout(std::time::Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "second writer materialized a root before the first writer committed"
    );

    let cas_hash = first.cas().store(b"serialized").unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let mut trove = Trove::new(
        "serialized-fixture".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&tx).unwrap();
    FileEntry::new(
        "/opt/serialized-effect".to_string(),
        resolved_regular(0o644),
        Some(PayloadContentAuthority {
            sha256: cas_hash,
            size: 10,
        }),
        trove_id,
    )
    .insert(&tx)
    .unwrap();
    let debt = GenerationPublication::create_pending(
        &tx,
        None,
        None,
        db_path.to_str().unwrap(),
        &runtime_root.root().display().to_string(),
        "first writer",
        &Default::default(),
    )
    .unwrap();
    first
        .persist_for_publication(&tx, &runtime_root, &debt)
        .unwrap();
    tx.commit().unwrap();
    drop(first);

    let (root_bytes, db_content) = result_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    waiter.join().unwrap();
    assert_eq!(root_bytes, b"serialized");
    let db_content = db_content.expect("serialized DB file has no content authority");
    assert_eq!(db_content.size, root_bytes.len() as u64);
    assert_eq!(db_content.sha256, CasStore::compute_sha256(&root_bytes));
}

#[test]
fn abandoned_selected_root_snapshot_remains_valid_for_history() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('forward', 'applied')",
        [],
    )
    .unwrap();
    let changeset_id = conn.last_insert_rowid();
    let mut session =
        SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "forward").unwrap();
    let debt = GenerationPublication::create_pending(
        &conn,
        Some(changeset_id),
        None,
        db_path.to_str().unwrap(),
        &runtime_root.root().display().to_string(),
        "forward",
        &Default::default(),
    )
    .unwrap();
    session
        .apply_install_files(&[live_regular("/opt/forward", b"forward", 0o644)])
        .unwrap();
    session
        .persist_for_publication(&conn, &runtime_root, &debt)
        .unwrap();
    let snapshot_id = GenerationPublication::find_by_id(&conn, debt.id.unwrap())
        .unwrap()
        .unwrap()
        .selected_root_snapshot_id
        .unwrap();

    assert_eq!(
        GenerationPublication::abandon_recoverable_for_changeset(&conn, changeset_id).unwrap(),
        1
    );
    assert!(
        SelectedRootSnapshot::find(&conn, snapshot_id)
            .unwrap()
            .is_some()
    );
}
