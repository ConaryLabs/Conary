// conary-core/src/repository/sync/tests/native/canonical.rs

use super::*;

#[test]
fn test_link_canonical_ids_populates_from_implementations() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('firefox-web', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
             VALUES (?1, 'fedora-44', 'firefox', 'contract')",
        [canonical_id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
            VALUES ('fedora-44', 'https://example.com', 1, 10, 'fedora-44')",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'firefox', '125.0', 'sha256:abc', 1024, 'https://example.com/firefox.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();
    let pkg_id = conn.last_insert_rowid();

    let count = link_canonical_ids(&conn, repo_id).unwrap();
    assert_eq!(count, 1);

    let cid: Option<i64> = conn
        .query_row(
            "SELECT canonical_id FROM repository_packages WHERE id = ?1",
            [pkg_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cid, Some(canonical_id));
}

#[test]
fn test_link_canonical_ids_skips_already_linked() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('test', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('test-repo', 'https://example.com', 1, 10)",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'test-pkg', '1.0', 'sha256:x', 100, 'https://example.com/x', 'rpm', ?2)",
            rusqlite::params![repo_id, canonical_id],
        )
        .unwrap();

    let count = link_canonical_ids(&conn, repo_id).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn repository_name_never_substitutes_for_source_profile_authority() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('firefox-web', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
         VALUES (?1, 'fedora-44', 'firefox', 'contract')",
        [canonical_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority)
         VALUES ('fedora-44', 'https://example.com', 1, 10)",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, checksum, size, download_url, version_scheme)
         VALUES (?1, 'firefox', '125.0', 'sha256:abc', 1024,
                 'https://example.com/firefox.rpm', 'rpm')",
        [repo_id],
    )
    .unwrap();
    let package_id = conn.last_insert_rowid();

    assert_eq!(link_canonical_ids(&conn, repo_id).unwrap(), 0);
    let linked: Option<i64> = conn
        .query_row(
            "SELECT canonical_id FROM repository_packages WHERE id = ?1",
            [package_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linked, None);
}
