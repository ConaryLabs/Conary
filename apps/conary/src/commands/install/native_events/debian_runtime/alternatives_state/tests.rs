// apps/conary/src/commands/install/native_events/debian_runtime/alternatives_state/tests.rs

use super::*;
use std::os::unix::fs::symlink;
use std::process::Command;

const EDITOR_STATE: &str = "\
auto
/usr/bin/editor
editor.1.gz
/usr/share/man/man1/editor.1.gz

/usr/bin/nano
40
/usr/share/man/man1/nano.1.gz
/usr/bin/vim.basic
30
/usr/share/man/man1/vim.1.gz

";

fn current_connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    conn
}

fn captured_fixture() -> (tempfile::TempDir, Connection) {
    let root = tempfile::tempdir().unwrap();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    fs::create_dir_all(admin.join("alternatives")).unwrap();
    fs::write(admin.join("alternatives/editor"), EDITOR_STATE).unwrap();
    fs::create_dir_all(root.path().join("etc/alternatives")).unwrap();
    symlink("/usr/bin/nano", root.path().join("etc/alternatives/editor")).unwrap();
    let conn = current_connection();
    capture(&conn, &admin, root.path()).unwrap();
    (root, conn)
}

#[test]
fn upstream_admin_grammar_round_trips_canonically() {
    let mut group = parse_group("editor", EDITOR_STATE).unwrap();
    group.selected_path = "/usr/bin/nano".to_string();
    assert_eq!(serialize_group(&group).unwrap(), EDITOR_STATE);
    assert_eq!(group.mode, AlternativeMode::Auto);
    assert_eq!(group.slaves.len(), 1);
    assert_eq!(group.choices["/usr/bin/nano"].priority, 40);
}

#[test]
fn helper_state_is_normalized_then_reprojected_with_links() {
    let (root, conn) = captured_fixture();
    assert_eq!(
        conn.query_row(
            "SELECT mode || ':' || selected_path
             FROM debian_alternative_groups
             WHERE name = 'editor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "auto:/usr/bin/nano"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM debian_alternative_choices
             WHERE group_name = 'editor'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2
    );

    let admin = root.path().join("var/lib/conary/dpkg-compat");
    fs::remove_dir_all(&admin).unwrap();
    fs::create_dir_all(&admin).unwrap();
    materialize(&conn, &admin, root.path()).unwrap();

    assert_eq!(
        fs::read_to_string(admin.join("alternatives/editor")).unwrap(),
        EDITOR_STATE
    );
    assert_eq!(
        fs::read_link(root.path().join("etc/alternatives/editor")).unwrap(),
        Path::new("/usr/bin/nano")
    );
    assert_eq!(
        fs::read_link(root.path().join("usr/bin/editor")).unwrap(),
        Path::new("/etc/alternatives/editor")
    );
    assert_eq!(
        fs::read_link(root.path().join("etc/alternatives/editor.1.gz")).unwrap(),
        Path::new("/usr/share/man/man1/nano.1.gz")
    );
}

#[test]
fn parser_rejects_incomplete_or_noncanonical_state() {
    assert!(parse_group("editor", "auto\n/usr/bin/editor\n").is_err());
    assert!(
        parse_group(
            "editor",
            "auto\n/usr/bin/editor\n\n/usr/bin/../bin/nano\n40\n\n"
        )
        .is_err()
    );
    assert!(
        parse_group(
            "editor",
            "auto\n/usr/bin/editor\nslave\n/usr/bin/slave\n\n/usr/bin/nano\n40\n"
        )
        .is_err()
    );
}

#[test]
fn capture_rejects_symlinked_administrative_records() {
    let root = tempfile::tempdir().unwrap();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    fs::create_dir_all(admin.join("alternatives")).unwrap();
    fs::write(root.path().join("foreign"), EDITOR_STATE).unwrap();
    symlink(
        root.path().join("foreign"),
        admin.join("alternatives/editor"),
    )
    .unwrap();
    let conn = current_connection();
    assert!(capture(&conn, &admin, root.path()).is_err());
}

#[test]
fn failed_capture_preserves_prior_normalized_state() {
    let (root, conn) = captured_fixture();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    fs::write(admin.join("alternatives/editor"), "corrupt\n").unwrap();
    assert!(capture(&conn, &admin, root.path()).is_err());
    assert_eq!(
        conn.query_row(
            "SELECT selected_path FROM debian_alternative_groups
             WHERE name = 'editor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "/usr/bin/nano"
    );
}

#[test]
fn empty_helper_state_removes_normalized_groups() {
    let (root, conn) = captured_fixture();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    fs::remove_file(admin.join("alternatives/editor")).unwrap();
    fs::remove_file(root.path().join("etc/alternatives/editor")).unwrap();

    capture(&conn, &admin, root.path()).unwrap();

    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM debian_alternative_groups",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}

#[test]
fn real_update_alternatives_state_is_accepted_when_installed() {
    if Command::new("update-alternatives")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("usr/bin")).unwrap();
    fs::create_dir_all(root.path().join("var/log")).unwrap();
    fs::write(root.path().join("usr/bin/nano"), b"fixture").unwrap();
    let admin = root.path().join("var/lib/conary/dpkg-compat");
    fs::create_dir_all(&admin).unwrap();
    let status = Command::new("update-alternatives")
        .arg("--root")
        .arg(root.path())
        .arg("--admindir")
        .arg(admin.join("alternatives"))
        .args([
            "--install",
            "/usr/bin/editor",
            "editor",
            "/usr/bin/nano",
            "40",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let conn = current_connection();
    capture(&conn, &admin, root.path()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT selected_path FROM debian_alternative_groups
             WHERE name = 'editor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "/usr/bin/nano"
    );
}
