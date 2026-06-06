// apps/conary/tests/native_pm_live_root.rs

use conary_core::db::models::{Repository, RepositoryPackage, SecurityAdvisorySupport};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE_NAME: &str = "conary-test-fixture";

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn run_conary_owned(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn fixture_dir(version: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conary-test-fixture")
        .join(version)
}

fn find_ccs_package(output_dir: &Path) -> PathBuf {
    fs::read_dir(output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension() == Some(OsStr::new("ccs")))
        .expect("ccs build should create a .ccs package")
}

fn build_ccs_fixture(work_dir: &Path, version: &str) -> PathBuf {
    let fixture = fixture_dir(version);
    let output_dir = work_dir.join(format!("ccs-{version}"));
    fs::create_dir_all(&output_dir).unwrap();

    let output = run_conary_owned(&[
        "ccs".to_string(),
        "build".to_string(),
        fixture.to_string_lossy().into_owned(),
        "--source".to_string(),
        fixture.join("stage").to_string_lossy().into_owned(),
        "--output".to_string(),
        output_dir.to_string_lossy().into_owned(),
        "--target".to_string(),
        "ccs".to_string(),
    ]);
    assert_success(&output);

    find_ccs_package(&output_dir)
}

fn install_ccs_into_root(root: &Path, db_path: &Path, package_path: &Path) {
    let output = run_conary_owned(&[
        "ccs".to_string(),
        "install".to_string(),
        package_path.to_string_lossy().into_owned(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "--allow-unsigned".to_string(),
        "--sandbox".to_string(),
        "never".to_string(),
        "--no-deps".to_string(),
        "--yes".to_string(),
    ]);
    assert_success(&output);
}

fn seed_fixture_update(
    db_path: &Path,
    package_path: &Path,
    download_url: String,
    security_advisory_support: SecurityAdvisorySupport,
    is_security_update: bool,
) -> i64 {
    let conn = conary_core::db::open(db_path).unwrap();
    let mut repo = Repository::new(
        "fedora-slice-b-fixture".to_string(),
        "https://example.invalid/fedora/repodata/".to_string(),
    );
    repo.gpg_check = false;
    repo.gpg_strict = false;
    repo.default_strategy = Some("binary".to_string());
    repo.security_advisory_support = security_advisory_support;
    let repo_id = repo.insert(&conn).unwrap();

    let package_bytes = fs::read(package_path).unwrap();
    let mut repo_package = RepositoryPackage::new(
        repo_id,
        FIXTURE_NAME.to_string(),
        "2.0.0".to_string(),
        conary_core::hash::sha256(&package_bytes),
        package_bytes.len() as i64,
        download_url,
    );
    repo_package.architecture = Some(std::env::consts::ARCH.to_string());
    repo_package.description = Some("Slice B update fixture".to_string());
    repo_package.distro = Some("fedora".to_string());
    repo_package.version_scheme = Some("rpm".to_string());
    repo_package.is_security_update = is_security_update;
    if is_security_update {
        repo_package.severity = Some("critical".to_string());
        repo_package.advisory_id = Some("TEST-2026-0001".to_string());
    }
    repo_package.insert(&conn).unwrap();

    conn.execute(
        "UPDATE troves
         SET installed_from_repository_id = ?1, source_distro = 'fedora', version_scheme = 'rpm'
         WHERE name = ?2",
        (repo_id, FIXTURE_NAME),
    )
    .unwrap();

    repo_id
}

fn installed_versions(db_path: &Path) -> Vec<String> {
    let conn = conary_core::db::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT version FROM troves WHERE name = ?1 ORDER BY version")
        .unwrap();
    stmt.query_map([FIXTURE_NAME], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn serve_static_package(package_path: &Path) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let package_path = package_path.to_path_buf();
    let filename = package_path.file_name().unwrap().to_string_lossy();
    let url = format!("http://{address}/{filename}");

    let handle = thread::spawn(move || {
        let started = Instant::now();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request);
                    let body = fs::read(&package_path).unwrap();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() > Duration::from_secs(10) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (url, handle)
}

fn serve_security_json_repository(package_path: &Path) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let package_path = package_path.to_path_buf();
    let package_bytes = fs::read(&package_path).unwrap();
    let filename = package_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let base_url = format!("http://{address}");
    let package_url = format!("{base_url}/{filename}");
    let metadata_body = serde_json::json!({
        "name": "security-json-fixture",
        "version": "1",
        "security_advisory_source": {
            "name": "conary-json",
            "trust": "trusted"
        },
        "packages": [
            {
                "name": FIXTURE_NAME,
                "version": "2.0.0",
                "architecture": std::env::consts::ARCH,
                "description": "Trusted security update fixture",
                "checksum": conary_core::hash::sha256(&package_bytes),
                "size": package_bytes.len() as i64,
                "download_url": package_url,
                "dependencies": [],
                "security_advisory": {
                    "id": "TEST-2026-0001",
                    "severity": "critical",
                    "cves": ["CVE-2026-0001"],
                    "fixed_version": "2.0.0",
                    "url": "https://security.example.test/TEST-2026-0001"
                }
            }
        ]
    })
    .to_string()
    .into_bytes();

    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut served_metadata = false;
        let mut served_package = false;

        while started.elapsed() <= Duration::from_secs(20) && !(served_metadata && served_package) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 2048];
                    let bytes_read = stream.read(&mut request).unwrap_or(0);
                    let request_text = String::from_utf8_lossy(&request[..bytes_read]);
                    let request_path = request_text
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");

                    let (status, content_type, body): (&str, &str, &[u8]) =
                        if request_path == "/metadata.json" {
                            served_metadata = true;
                            ("200 OK", "application/json", metadata_body.as_slice())
                        } else if request_path.trim_start_matches('/') == filename {
                            served_package = true;
                            (
                                "200 OK",
                                "application/octet-stream",
                                package_bytes.as_slice(),
                            )
                        } else {
                            ("404 Not Found", "text/plain", b"not found".as_slice())
                        };

                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (base_url, handle)
}

#[test]
fn no_generation_remove_deletes_file_and_history_records_apply() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let payload = root.path().join("usr/bin/fixture");
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    std::fs::write(&payload, "fixture").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "fixture".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
    );
    let trove_id = trove.insert(&conn).unwrap();
    conary_core::db::models::FileEntry::new(
        "/usr/bin/fixture".to_string(),
        "0".repeat(64),
        7,
        0o100755,
        trove_id,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let output = run_conary(&[
        "remove",
        "fixture",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!payload.exists());

    let history = run_conary(&["system", "history", "--db-path", db_path.to_str().unwrap()]);
    assert!(
        history.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&history.stdout),
        String::from_utf8_lossy(&history.stderr)
    );
    let stdout = String::from_utf8_lossy(&history.stdout);
    assert!(stdout.contains("Remove fixture-1.0.0"), "{stdout}");
    assert!(stdout.contains("Applied"), "{stdout}");
}

#[test]
fn remove_arch_selector_targets_one_variant() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    for arch in ["x86_64", "aarch64"] {
        let payload = root.path().join(format!("usr/bin/remove-demo-{arch}"));
        fs::create_dir_all(payload.parent().unwrap()).unwrap();
        fs::write(&payload, arch).unwrap();

        let mut trove = conary_core::db::models::Trove::new_with_source(
            "remove-demo".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        trove.architecture = Some(arch.to_string());
        let trove_id = trove.insert(&conn).unwrap();
        conary_core::db::models::FileEntry::new(
            format!("/usr/bin/remove-demo-{arch}"),
            "0".repeat(64),
            arch.len() as i64,
            0o100755,
            trove_id,
        )
        .insert(&conn)
        .unwrap();
    }
    drop(conn);

    let ambiguous = run_conary(&[
        "remove",
        "remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
        "--yes",
    ]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));

    let selected = run_conary(&[
        "remove",
        "remove-demo",
        "--version",
        "1.0.0",
        "--arch",
        "aarch64",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
        "--yes",
    ]);
    assert_success(&selected);
    assert!(root.path().join("usr/bin/remove-demo-x86_64").exists());
    assert!(!root.path().join("usr/bin/remove-demo-aarch64").exists());
}

#[test]
fn remove_ignores_non_package_trove_with_same_name() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let payload = root.path().join("usr/bin/remove-shared-selector");
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    fs::write(&payload, "fixture").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut package = conary_core::db::models::Trove::new_with_source(
        "remove-shared-selector".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
    );
    let trove_id = package.insert(&conn).unwrap();
    conary_core::db::models::FileEntry::new(
        "/usr/bin/remove-shared-selector".to_string(),
        "0".repeat(64),
        7,
        0o100755,
        trove_id,
    )
    .insert(&conn)
    .unwrap();

    let mut collection = conary_core::db::models::Trove::new(
        "remove-shared-selector".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Collection,
    );
    collection.insert(&conn).unwrap();
    drop(conn);

    let output = run_conary(&[
        "remove",
        "remove-shared-selector",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert_success(&output);
    assert!(!payload.exists());
    let conn = conary_core::db::open(&db_path).unwrap();
    let remaining =
        conary_core::db::models::Trove::find_by_name(&conn, "remove-shared-selector").unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].trove_type,
        conary_core::db::models::TroveType::Collection
    );
}

#[test]
fn no_generation_update_installs_repository_ccs_into_live_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let v1_package = build_ccs_fixture(temp.path(), "v1");
    let v2_package = build_ccs_fixture(temp.path(), "v2");
    install_ccs_into_root(&root, &db_path, &v1_package);

    assert_eq!(installed_versions(&db_path), vec!["1.0.0".to_string()]);
    assert_eq!(
        fs::read_to_string(root.join("usr/share/conary-test/hello.txt")).unwrap(),
        "hello-v1\n"
    );

    let (download_url, server) = serve_static_package(&v2_package);
    seed_fixture_update(
        &db_path,
        &v2_package,
        download_url,
        SecurityAdvisorySupport::Supported,
        false,
    );

    let output = run_conary_owned(&[
        "update".to_string(),
        FIXTURE_NAME.to_string(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "--sandbox".to_string(),
        "never".to_string(),
        "--yes".to_string(),
    ]);
    server.join().unwrap();
    assert_success(&output);

    assert_eq!(installed_versions(&db_path), vec!["2.0.0".to_string()]);
    assert_eq!(
        fs::read_to_string(root.join("usr/share/conary-test/hello.txt")).unwrap(),
        "hello-v2\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("usr/share/conary-test/added.txt")).unwrap(),
        "added-in-v2\n"
    );

    let list = run_conary_owned(&[
        "list".to_string(),
        FIXTURE_NAME.to_string(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--info".to_string(),
    ]);
    assert_success(&list);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("Version     : 2.0.0"), "{stdout}");
    assert!(stdout.contains("Files       : 2"), "{stdout}");
}

#[test]
fn security_update_with_unknown_advisory_support_refuses_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let v1_package = build_ccs_fixture(temp.path(), "v1");
    let v2_package = build_ccs_fixture(temp.path(), "v2");
    install_ccs_into_root(&root, &db_path, &v1_package);
    seed_fixture_update(
        &db_path,
        &v2_package,
        "http://127.0.0.1:9/conary-test-fixture-2.0.0.ccs".to_string(),
        SecurityAdvisorySupport::Unknown,
        true,
    );

    let output = run_conary_owned(&[
        "update".to_string(),
        FIXTURE_NAME.to_string(),
        "--security".to_string(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "--sandbox".to_string(),
        "never".to_string(),
        "--yes".to_string(),
    ]);

    assert!(
        !output.status.success(),
        "security-only update should refuse unverifiable metadata"
    );
    let combined = output_text(&output);
    assert!(combined.contains("security metadata"), "{combined}");
    assert!(combined.contains("fedora-slice-b-fixture"), "{combined}");
    assert_eq!(installed_versions(&db_path), vec!["1.0.0".to_string()]);
    assert_eq!(
        fs::read_to_string(root.join("usr/share/conary-test/hello.txt")).unwrap(),
        "hello-v1\n"
    );
    assert!(!root.join("usr/share/conary-test/added.txt").exists());
}

#[test]
fn security_update_syncs_trusted_json_advisory_and_applies_fix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let v1_package = build_ccs_fixture(temp.path(), "v1");
    let v2_package = build_ccs_fixture(temp.path(), "v2");
    install_ccs_into_root(&root, &db_path, &v1_package);

    let (repo_url, server) = serve_security_json_repository(&v2_package);
    let add_repo = run_conary_owned(&[
        "repo".to_string(),
        "add".to_string(),
        "security-json-fixture".to_string(),
        repo_url.clone(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--no-gpg-check".to_string(),
        "--security-advisories".to_string(),
        "supported".to_string(),
    ]);
    assert_success(&add_repo);

    let sync = run_conary_owned(&[
        "repo".to_string(),
        "sync".to_string(),
        "security-json-fixture".to_string(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--force".to_string(),
    ]);
    assert_success(&sync);

    let conn = conary_core::db::open(&db_path).unwrap();
    let repo = Repository::find_by_name(&conn, "security-json-fixture")
        .unwrap()
        .expect("security fixture repo should exist");
    conn.execute(
        "UPDATE troves
         SET installed_from_repository_id = ?1, source_distro = 'security-json-fixture', version_scheme = 'rpm'
         WHERE name = ?2",
        (repo.id.unwrap(), FIXTURE_NAME),
    )
    .unwrap();
    drop(conn);

    let output = run_conary_owned(&[
        "update".to_string(),
        FIXTURE_NAME.to_string(),
        "--security".to_string(),
        "--db-path".to_string(),
        db_path.to_string_lossy().into_owned(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "--sandbox".to_string(),
        "never".to_string(),
        "--yes".to_string(),
    ]);
    server.join().unwrap();
    assert_success(&output);

    let combined = output_text(&output);
    assert!(combined.contains("TEST-2026-0001"), "{combined}");
    assert!(combined.contains("CVE-2026-0001"), "{combined}");
    assert!(
        combined.contains("trusted source: conary-json"),
        "{combined}"
    );
    assert_eq!(installed_versions(&db_path), vec!["2.0.0".to_string()]);
    assert_eq!(
        fs::read_to_string(root.join("usr/share/conary-test/hello.txt")).unwrap(),
        "hello-v2\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("usr/share/conary-test/added.txt")).unwrap(),
        "added-in-v2\n"
    );
}
