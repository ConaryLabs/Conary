// crates/conary-core/src/db/models/trigger/tests.rs

use super::super::persisted_value::PersistedValueCorruption;
use super::super::trigger_engine::TriggerEngine;
use super::*;
use crate::trigger::TriggerExecutor;
use tempfile::NamedTempFile;

fn create_test_db() -> (NamedTempFile, Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp_file.path()).unwrap();

    // Create tables
    conn.execute_batch(
        "
            CREATE TABLE triggers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT,
                pattern TEXT NOT NULL,
                handler TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 50,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE trigger_dependencies (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trigger_id INTEGER NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
                depends_on TEXT NOT NULL,
                UNIQUE(trigger_id, depends_on)
            );

            CREATE TABLE changesets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                description TEXT
            );

            CREATE TABLE changeset_triggers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                changeset_id INTEGER NOT NULL,
                trigger_id INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped')),
                matched_files INTEGER NOT NULL DEFAULT 0,
                started_at TEXT,
                completed_at TEXT,
                output TEXT,
                UNIQUE(changeset_id, trigger_id)
            );
            ",
    )
    .unwrap();

    (temp_file, conn)
}

#[test]
fn test_trigger_crud() {
    let (_temp, conn) = create_test_db();

    // Create trigger
    let mut trigger = Trigger::new(
        "ldconfig".to_string(),
        "/usr/lib/*.so*".to_string(),
        "/sbin/ldconfig".to_string(),
    )
    .unwrap();
    trigger.description = Some("Update shared library cache".to_string());
    let id = trigger.insert(&conn).unwrap();

    // Find by ID
    let found = Trigger::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(found.name, "ldconfig");
    assert_eq!(found.handler, "/sbin/ldconfig");

    // Find by name
    let found = Trigger::find_by_name(&conn, "ldconfig").unwrap().unwrap();
    assert_eq!(found.id, Some(id));

    // List all
    let triggers = Trigger::list_all(&conn).unwrap();
    assert_eq!(triggers.len(), 1);

    // Disable
    Trigger::disable(&conn, id).unwrap();
    let found = Trigger::find_by_id(&conn, id).unwrap().unwrap();
    assert!(!found.enabled);

    // Enable
    Trigger::enable(&conn, id).unwrap();
    let found = Trigger::find_by_id(&conn, id).unwrap().unwrap();
    assert!(found.enabled);

    assert!(Trigger::delete(&conn, id).unwrap());
    assert!(Trigger::find_by_id(&conn, id).unwrap().is_none());
}

#[test]
fn test_trigger_pattern_matching() {
    let trigger = Trigger::new(
        "ldconfig".to_string(),
        "/usr/lib/*.so*,/usr/lib64/*.so*".to_string(),
        "/sbin/ldconfig".to_string(),
    )
    .unwrap();

    // Should match
    assert!(trigger.matches("/usr/lib/libssl.so.3").unwrap());
    assert!(trigger.matches("/usr/lib64/libc.so.6").unwrap());
    assert!(trigger.matches("/usr/lib/libfoo.so").unwrap());

    // Should not match
    assert!(!trigger.matches("/usr/bin/ls").unwrap());
    assert!(!trigger.matches("/etc/passwd").unwrap());
    assert!(!trigger.matches("/usr/lib/pkgconfig/foo.pc").unwrap());
}

#[test]
fn invalid_trigger_patterns_fail_before_persistence() {
    let invalid_glob = Trigger::new(
        "invalid-glob".to_string(),
        "/usr/lib/[broken".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap_err();
    assert!(
        invalid_glob
            .to_string()
            .contains("invalid path pattern '/usr/lib/[broken'"),
        "{invalid_glob}"
    );

    let empty_member = Trigger::new(
        "empty-member".to_string(),
        "/usr/lib/*,,/usr/lib64/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap_err();
    assert!(
        empty_member.to_string().contains("empty path pattern"),
        "{empty_member}"
    );
}

#[test]
fn invalid_persisted_trigger_pattern_is_a_planning_error() {
    let (_temp, conn) = create_test_db();
    conn.execute(
        "INSERT INTO triggers (name, pattern, handler, priority, enabled)
             VALUES ('corrupt', '/usr/lib/[broken', '/bin/true', 50, 1)",
        [],
    )
    .unwrap();

    let error = TriggerEngine::new(&conn)
        .find_matching_triggers(&["/usr/lib/libfoo.so".to_string()])
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("corrupt"), "{message}");
    assert!(message.contains("invalid path pattern"), "{message}");
}

#[test]
fn corrupt_persisted_status_stops_execution_before_the_handler_loop() {
    let (_temp, conn) = create_test_db();
    let mut trigger = Trigger::new(
        "must-not-run".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    let trigger_id = trigger.insert(&conn).unwrap();
    conn.execute(
        "INSERT INTO changesets (description) VALUES ('corrupt status')",
        [],
    )
    .unwrap();
    let changeset_id = conn.last_insert_rowid();
    let mut changeset_trigger = ChangesetTrigger::new(changeset_id, trigger_id);
    let row_id = changeset_trigger.upsert(&conn).unwrap();

    conn.pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    conn.execute(
        "UPDATE changeset_triggers SET status = 'already-ran-maybe' WHERE id = ?1",
        [row_id],
    )
    .unwrap();
    conn.pragma_update(None, "ignore_check_constraints", false)
        .unwrap();

    let root = tempfile::tempdir().unwrap();
    let error = TriggerExecutor::new(&conn, root.path())
        .execute_pending(changeset_id)
        .unwrap_err();
    let crate::error::Error::Database(rusqlite::Error::FromSqlConversionFailure(
        3,
        rusqlite::types::Type::Text,
        source,
    )) = error
    else {
        panic!("unexpected error: {error}");
    };
    let corruption = source
        .downcast_ref::<PersistedValueCorruption>()
        .expect("status error must retain typed row corruption");
    assert_eq!(corruption.table(), "changeset_triggers");
    assert_eq!(corruption.row_id(), row_id);
    assert_eq!(corruption.column(), "status");
    assert_eq!(corruption.value(), "already-ran-maybe");

    let stored_status: String = conn
        .query_row(
            "SELECT status FROM changeset_triggers WHERE id = ?1",
            [row_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_status, "already-ran-maybe");
}

#[test]
fn insert_revalidates_public_pattern_field() {
    let (_temp, conn) = create_test_db();
    let mut trigger = Trigger::new(
        "mutated".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    trigger.pattern = "/usr/lib/[broken".to_string();

    let error = trigger.insert(&conn).unwrap_err();
    assert!(
        error.to_string().contains("invalid path pattern"),
        "{error}"
    );
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM triggers", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_trigger_dependencies() {
    let (_temp, conn) = create_test_db();

    // Create triggers
    let mut trigger1 = Trigger::new(
        "sysusers".to_string(),
        "/usr/lib/sysusers.d/*".to_string(),
        "systemd-sysusers".to_string(),
    )
    .unwrap();
    let mut trigger2 = Trigger::new(
        "tmpfiles".to_string(),
        "/usr/lib/tmpfiles.d/*".to_string(),
        "systemd-tmpfiles".to_string(),
    )
    .unwrap();

    trigger1.insert(&conn).unwrap();
    let id2 = trigger2.insert(&conn).unwrap();

    // tmpfiles depends on sysusers
    TriggerDependency::add(&conn, id2, "sysusers").unwrap();

    let deps = TriggerDependency::get_dependencies(&conn, id2).unwrap();
    assert_eq!(deps, vec!["sysusers"]);
}

#[test]
fn test_changeset_trigger_tracking() {
    let (_temp, conn) = create_test_db();

    // Create a changeset
    conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
        .unwrap();
    let changeset_id = conn.last_insert_rowid();

    // Create trigger
    let mut trigger =
        Trigger::new("test".to_string(), "/*".to_string(), "true".to_string()).unwrap();
    let trigger_id = trigger.insert(&conn).unwrap();

    // Track trigger
    let mut ct = ChangesetTrigger::new(changeset_id, trigger_id);
    ct.matched_files = 5;
    ct.upsert(&conn).unwrap();

    // Find pending
    let pending = ChangesetTrigger::find_pending(&conn, changeset_id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].matched_files, 5);

    // Mark running
    ChangesetTrigger::mark_running(&conn, changeset_id, trigger_id).unwrap();
    let all = ChangesetTrigger::find_by_changeset(&conn, changeset_id).unwrap();
    assert_eq!(all[0].status, TriggerStatus::Running);

    // Mark completed
    ChangesetTrigger::mark_completed(&conn, changeset_id, trigger_id, Some("OK")).unwrap();
    let all = ChangesetTrigger::find_by_changeset(&conn, changeset_id).unwrap();
    assert_eq!(all[0].status, TriggerStatus::Completed);
}

#[test]
fn test_trigger_engine_matching() {
    let (_temp, conn) = create_test_db();

    // Create triggers
    let mut t1 = Trigger::new(
        "ldconfig".to_string(),
        "/usr/lib/*.so*".to_string(),
        "/sbin/ldconfig".to_string(),
    )
    .unwrap();
    let mut t2 = Trigger::new(
        "icons".to_string(),
        "/usr/share/icons/*".to_string(),
        "gtk-update-icon-cache".to_string(),
    )
    .unwrap();
    t1.insert(&conn).unwrap();
    t2.insert(&conn).unwrap();

    let engine = TriggerEngine::new(&conn);
    let files = vec![
        "/usr/lib/libssl.so.3".to_string(),
        "/usr/lib/libcrypto.so.3".to_string(),
        "/usr/share/icons/hicolor/48x48/apps/foo.png".to_string(),
    ];

    let matches = engine.find_matching_triggers(&files).unwrap();
    assert_eq!(matches.len(), 2);

    // Check that ldconfig matched 2 files and icons matched 1
    for (trigger, matched) in &matches {
        match trigger.name.as_str() {
            "ldconfig" => assert_eq!(matched.len(), 2),
            "icons" => assert_eq!(matched.len(), 1),
            _ => panic!("Unexpected trigger"),
        }
    }
}

#[test]
fn test_execution_order_preserves_topological_sort() {
    // Regression test: a high-priority trigger that depends on a low-priority
    // trigger must still execute after its dependency. The topological order
    // must not be destroyed by a secondary priority sort.
    let (_temp, conn) = create_test_db();

    // Create trigger B (low priority = runs first if no deps, priority 90)
    let mut trigger_b = Trigger::new(
        "trigger_b".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    trigger_b.priority = 90;
    let id_b = trigger_b.insert(&conn).unwrap();

    // Create trigger A (high priority = would run first by priority alone, priority 10)
    // but A depends on B, so B must run first
    let mut trigger_a = Trigger::new(
        "trigger_a".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    trigger_a.priority = 10;
    let id_a = trigger_a.insert(&conn).unwrap();

    // A depends on B
    TriggerDependency::add(&conn, id_a, "trigger_b").unwrap();

    // Create a changeset with both triggers pending
    conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
        .unwrap();
    let changeset_id = conn.last_insert_rowid();

    let mut ct_b = ChangesetTrigger::new(changeset_id, id_b);
    ct_b.upsert(&conn).unwrap();
    let mut ct_a = ChangesetTrigger::new(changeset_id, id_a);
    ct_a.upsert(&conn).unwrap();

    let engine = TriggerEngine::new(&conn);
    let order = engine.get_execution_order(changeset_id).unwrap();

    assert_eq!(order.len(), 2);
    // B must come before A despite A having higher (lower number) priority
    assert_eq!(order[0].name, "trigger_b", "dependency must execute first");
    assert_eq!(order[1].name, "trigger_a", "dependent must execute second");
}

#[test]
fn test_execution_order_respects_priority_within_level() {
    // Triggers with no dependency relationship should be ordered by priority
    let (_temp, conn) = create_test_db();

    let mut t_low = Trigger::new(
        "zz_low_priority".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    t_low.priority = 90;
    let id_low = t_low.insert(&conn).unwrap();

    let mut t_high = Trigger::new(
        "aa_high_priority".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    t_high.priority = 10;
    let id_high = t_high.insert(&conn).unwrap();

    conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
        .unwrap();
    let changeset_id = conn.last_insert_rowid();

    let mut ct_low = ChangesetTrigger::new(changeset_id, id_low);
    ct_low.upsert(&conn).unwrap();
    let mut ct_high = ChangesetTrigger::new(changeset_id, id_high);
    ct_high.upsert(&conn).unwrap();

    let engine = TriggerEngine::new(&conn);
    let order = engine.get_execution_order(changeset_id).unwrap();

    assert_eq!(order.len(), 2);
    // Both at the same topological level, so priority wins (lower number first)
    assert_eq!(
        order[0].name, "aa_high_priority",
        "higher priority (lower number) should run first within same level"
    );
    assert_eq!(order[1].name, "zz_low_priority");
}

#[test]
fn test_execution_order_rejects_dependency_cycles() {
    let (_temp, conn) = create_test_db();

    let mut trigger_a = Trigger::new(
        "trigger_a".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    let id_a = trigger_a.insert(&conn).unwrap();
    let mut trigger_b = Trigger::new(
        "trigger_b".to_string(),
        "/usr/lib/*".to_string(),
        "/bin/true".to_string(),
    )
    .unwrap();
    let id_b = trigger_b.insert(&conn).unwrap();

    TriggerDependency::add(&conn, id_a, "trigger_b").unwrap();
    TriggerDependency::add(&conn, id_b, "trigger_a").unwrap();

    conn.execute("INSERT INTO changesets (description) VALUES ('test')", [])
        .unwrap();
    let changeset_id = conn.last_insert_rowid();
    ChangesetTrigger::new(changeset_id, id_a)
        .upsert(&conn)
        .unwrap();
    ChangesetTrigger::new(changeset_id, id_b)
        .upsert(&conn)
        .unwrap();

    let error = TriggerEngine::new(&conn)
        .get_execution_order(changeset_id)
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("dependency cycle"));
    assert!(message.contains("trigger_a"));
    assert!(message.contains("trigger_b"));
}
