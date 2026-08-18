// conary-test/src/config/tests/native_corpus/rejection.rs

use super::conary_fixture_path;

fn authority_digest(database: &std::path::Path) -> String {
    let output = std::process::Command::new("python3")
        .arg(conary_fixture_path("native/database-authority-digest.py"))
        .arg(database)
        .output()
        .expect("run database authority digest");
    assert!(
        output.status.success(),
        "database authority digest failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn rejection_digest_ignores_only_derived_snapshot_cache_mutation() {
    let directory = tempfile::tempdir().expect("create rejection test directory");
    let database = directory.path().join("conary.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "
            CREATE TABLE troves (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE generation_db_mutation_epoch (
                singleton INTEGER PRIMARY KEY,
                token TEXT NOT NULL
            );
            INSERT INTO generation_db_mutation_epoch VALUES (1, 'before');
            CREATE TABLE selected_root_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                root_node_json TEXT NOT NULL
            );
            CREATE TABLE selected_root_snapshot_entries (
                snapshot_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                entry_json TEXT,
                PRIMARY KEY (snapshot_id, path)
            );
            ",
        )
        .unwrap();

    let before = authority_digest(&database);
    connection
        .execute_batch(
            "
            INSERT INTO selected_root_snapshots(root_node_json) VALUES ('{}');
            INSERT INTO selected_root_snapshot_entries VALUES (1, '/', '{}');
            UPDATE generation_db_mutation_epoch SET token = 'after';
            ",
        )
        .unwrap();
    assert_eq!(
        authority_digest(&database),
        before,
        "derived selected-root preparation cache must not impersonate transaction mutation"
    );

    connection
        .execute("INSERT INTO troves(name) VALUES ('mutated')", [])
        .unwrap();
    assert_ne!(
        authority_digest(&database),
        before,
        "package authority mutation must change the rejection digest"
    );
}
