// apps/conary/tests/native_pm_live_root.rs

use conary_core::db::models::{
    FileEntry, InstallSource, Repository, RepositoryPackage, RepositoryPackageKey,
    RepositoryPackageKeyStatus, SecurityAdvisorySupport, Trove, TroveType,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE_NAME: &str = "conary-test-fixture";

fn regular_file_entry(
    db_path: &Path,
    path: impl Into<String>,
    content: &[u8],
    permissions: u32,
    trove_id: i64,
) -> FileEntry {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let sha256 = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        .unwrap()
        .store(content)
        .unwrap();
    let mut node = PayloadNode::regular(permissions);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    FileEntry::new(
        path.into(),
        ResolvedPayloadNode::from_numeric_source(node).unwrap(),
        Some(PayloadContentAuthority {
            sha256,
            size: content.len() as u64,
        }),
        trove_id,
    )
}

fn directory_entry(path: &str, permissions: u32, trove_id: i64) -> FileEntry {
    let mut node = PayloadNode::regular(permissions);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | permissions;
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(node).unwrap(),
        None,
        trove_id,
    )
}

fn seed_selected_root_layout(db_path: &Path, immutable_directories: &[&str]) {
    stage_test_boot_assets(db_path.parent().unwrap());
    let conn = conary_core::db::open(db_path).unwrap();
    let mut layout = Trove::new_with_source(
        "test-filesystem-layout".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let layout_id = layout.insert(&conn).unwrap();

    directory_entry("/etc", 0o755, layout_id)
        .insert(&conn)
        .unwrap();
    directory_entry("/sbin", 0o755, layout_id)
        .insert(&conn)
        .unwrap();
    for path in immutable_directories {
        directory_entry(path, 0o755, layout_id)
            .insert(&conn)
            .unwrap();
    }
    regular_file_entry(
        db_path,
        "/etc/passwd",
        b"root:x:0:0:root:/root:/bin/sh\n",
        0o644,
        layout_id,
    )
    .insert(&conn)
    .unwrap();
    regular_file_entry(db_path, "/etc/group", b"root:x:0:\n", 0o644, layout_id)
        .insert(&conn)
        .unwrap();
    regular_file_entry(
        db_path,
        "/sbin/init",
        b"#!/bin/sh\nexec true\n",
        0o755,
        layout_id,
    )
    .insert(&conn)
    .unwrap();
}

fn stage_test_boot_assets(runtime_root: &Path) {
    let kernel_version = "test-kernel";
    let boot_root = runtime_root.join("boot");
    fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    fs::write(
        boot_root.join(format!("vmlinuz-{kernel_version}")),
        b"test-kernel",
    )
    .unwrap();
    fs::write(
        boot_root.join(format!("initramfs-{kernel_version}.img")),
        b"test-initramfs",
    )
    .unwrap();
    fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
}

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
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

fn run_conary_owned_in_test_home(args: &[String], test_home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .env("HOME", test_home.join("home"))
        .env("XDG_DATA_HOME", test_home.join("xdg-data"))
        .env("XDG_CONFIG_HOME", test_home.join("xdg-config"))
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

fn active_generation_artifact(
    db_path: &Path,
) -> conary_core::generation::artifact::GenerationArtifact {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .expect("package mutation should publish an active test generation");
    conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .expect("active generation artifact should load")
}

fn active_generation_has_path(db_path: &Path, path: &str) -> bool {
    let artifact = active_generation_artifact(db_path);
    artifact
        .generation_root
        .entries
        .iter()
        .chain(&artifact.mutable_state.entries)
        .any(|entry| entry.path == path)
}

fn active_generation_file_contents(db_path: &Path, path: &str) -> Vec<u8> {
    let artifact = active_generation_artifact(db_path);
    let content = artifact
        .generation_root
        .entries
        .iter()
        .chain(&artifact.mutable_state.entries)
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("active generation has no {path}"))
        .content
        .as_ref()
        .unwrap_or_else(|| panic!("active generation path {path} has no content authority"));
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        .unwrap()
        .retrieve(&content.sha256)
        .unwrap()
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

    let output = run_conary_owned_in_test_home(
        &[
            "ccs".to_string(),
            "build".to_string(),
            fixture
                .join("live-root/ccs.toml")
                .to_string_lossy()
                .into_owned(),
            "--source".to_string(),
            fixture.join("stage").to_string_lossy().into_owned(),
            "--output".to_string(),
            output_dir.to_string_lossy().into_owned(),
            "--target".to_string(),
            "ccs".to_string(),
            "--local-dev".to_string(),
        ],
        work_dir,
    );
    assert_success(&output);

    find_ccs_package(&output_dir)
}

fn install_ccs_into_root(root: &Path, db_path: &Path, package_path: &Path) {
    let test_home = db_path.parent().unwrap();
    let output = run_conary_owned_in_test_home(
        &[
            "ccs".to_string(),
            "install".to_string(),
            package_path.to_string_lossy().into_owned(),
            "--db-path".to_string(),
            db_path.to_string_lossy().into_owned(),
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "--sandbox".to_string(),
            "always".to_string(),
            "--no-deps".to_string(),
            "--yes".to_string(),
        ],
        test_home,
    );
    assert_success(&output);
}

fn local_dev_signing_key(test_home: &Path) -> conary_core::ccs::SigningKeyPair {
    conary_core::ccs::SigningKeyPair::load_from_file(
        &test_home.join("xdg-data/conary/ccs/local-dev/local-dev-key.private.toml"),
    )
    .expect("local-dev CCS signing key should exist after fixture build")
}

fn authorize_static_ccs_repository(
    conn: &rusqlite::Connection,
    repository: &mut Repository,
    test_home: &Path,
) {
    repository.default_strategy = Some("static".to_string());
    repository.update(conn).unwrap();
    let repository_id = repository.id.unwrap();
    let signer = local_dev_signing_key(test_home);
    RepositoryPackageKey::replace_for_repository(
        conn,
        repository_id,
        &[RepositoryPackageKey {
            repository_id,
            public_key: signer.public_key_base64(),
            key_id: signer.key_id().map(str::to_string),
            status: RepositoryPackageKeyStatus::Active,
            synced_at: None,
        }],
    )
    .unwrap();
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
    repo.set_parser_config(conary_core::repository::RepositoryParserConfig::Json)
        .unwrap();
    repo.default_strategy = Some("static".to_string());
    repo.source_profile = Some("fedora-44".to_string());
    repo.security_advisory_support = security_advisory_support;
    let repo_id = repo.insert(&conn).unwrap();
    authorize_static_ccs_repository(&conn, &mut repo, db_path.parent().unwrap());

    let package_bytes = fs::read(package_path).unwrap();
    let mut repo_package = RepositoryPackage::new(
        repo_id,
        FIXTURE_NAME.to_string(),
        "2.0.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        conary_core::hash::sha256(&package_bytes),
        package_bytes.len() as i64,
        download_url,
    );
    repo_package.architecture = Some(std::env::consts::ARCH.to_string());
    repo_package.description = Some("Slice B update fixture".to_string());
    repo_package.source_profile = Some("fedora-44".to_string());
    repo_package.is_security_update = is_security_update;
    if is_security_update {
        repo_package.severity = Some("critical".to_string());
        repo_package.advisory_id = Some("TEST-2026-0001".to_string());
    }
    repo_package.insert(&conn).unwrap();

    conn.execute(
        "UPDATE troves
         SET installed_from_repository_id = ?1, source_profile = 'fedora-44', version_scheme = 'rpm'
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
                "version_scheme": "rpm",
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
fn selected_generation_remove_deletes_file_and_records_applied_history() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    seed_selected_root_layout(&db_path, &["/usr", "/usr/bin"]);

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "fixture".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    regular_file_entry(&db_path, "/usr/bin/fixture", b"fixture", 0o755, trove_id)
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
        "--sandbox",
        "always",
        "--yes",
    ]);

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!active_generation_has_path(&db_path, "/usr/bin/fixture"));

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
    seed_selected_root_layout(&db_path, &["/usr", "/usr/bin"]);

    let conn = conary_core::db::open(&db_path).unwrap();
    for arch in ["x86_64", "aarch64"] {
        let mut trove = conary_core::db::models::Trove::new_with_source(
            "remove-demo".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some(arch.to_string());
        let trove_id = trove.insert(&conn).unwrap();
        regular_file_entry(
            &db_path,
            format!("/usr/bin/remove-demo-{arch}"),
            arch.as_bytes(),
            0o755,
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
        "--sandbox",
        "always",
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
        "--sandbox",
        "always",
        "--yes",
    ]);
    assert_success(&selected);
    assert!(active_generation_has_path(
        &db_path,
        "/usr/bin/remove-demo-x86_64"
    ));
    assert!(!active_generation_has_path(
        &db_path,
        "/usr/bin/remove-demo-aarch64"
    ));
}

#[test]
fn remove_ignores_non_package_trove_with_same_name() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    seed_selected_root_layout(&db_path, &["/usr", "/usr/bin"]);

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut package = conary_core::db::models::Trove::new_with_source(
        "remove-shared-selector".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = package.insert(&conn).unwrap();
    regular_file_entry(
        &db_path,
        "/usr/bin/remove-shared-selector",
        b"fixture",
        0o755,
        trove_id,
    )
    .insert(&conn)
    .unwrap();

    let mut collection = conary_core::db::models::Trove::new(
        "remove-shared-selector".to_string(),
        "2.0.0".to_string(),
        conary_core::db::models::TroveType::Collection,
        conary_core::repository::versioning::VersionScheme::Conary,
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
        "--sandbox",
        "always",
        "--yes",
    ]);

    assert_success(&output);
    assert!(!active_generation_has_path(
        &db_path,
        "/usr/bin/remove-shared-selector"
    ));
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
fn selected_generation_update_installs_signed_repository_ccs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    seed_selected_root_layout(&db_path, &[]);

    let v1_package = build_ccs_fixture(temp.path(), "v1");
    let v2_package = build_ccs_fixture(temp.path(), "v2");
    install_ccs_into_root(&root, &db_path, &v1_package);

    assert_eq!(installed_versions(&db_path), vec!["1.0.0".to_string()]);
    assert_eq!(
        active_generation_file_contents(&db_path, "/usr/share/conary-test/hello.txt"),
        b"hello-v1\n"
    );

    let (download_url, server) = serve_static_package(&v2_package);
    seed_fixture_update(
        &db_path,
        &v2_package,
        download_url,
        SecurityAdvisorySupport::Supported,
        false,
    );

    let output = run_conary_owned_in_test_home(
        &[
            "update".to_string(),
            FIXTURE_NAME.to_string(),
            "--db-path".to_string(),
            db_path.to_string_lossy().into_owned(),
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "--sandbox".to_string(),
            "always".to_string(),
            "--yes".to_string(),
        ],
        temp.path(),
    );
    server.join().unwrap();
    assert_success(&output);

    assert_eq!(installed_versions(&db_path), vec!["2.0.0".to_string()]);
    assert_eq!(
        active_generation_file_contents(&db_path, "/usr/share/conary-test/hello.txt"),
        b"hello-v2\n"
    );
    assert_eq!(
        active_generation_file_contents(&db_path, "/usr/share/conary-test/added.txt"),
        b"added-in-v2\n"
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
    assert!(stdout.contains("Files       : 5"), "{stdout}");
}

#[test]
fn security_update_with_unknown_advisory_support_refuses_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    seed_selected_root_layout(&db_path, &[]);

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

    let output = run_conary_owned_in_test_home(
        &[
            "update".to_string(),
            FIXTURE_NAME.to_string(),
            "--security".to_string(),
            "--db-path".to_string(),
            db_path.to_string_lossy().into_owned(),
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "--sandbox".to_string(),
            "always".to_string(),
            "--yes".to_string(),
        ],
        temp.path(),
    );

    assert!(
        !output.status.success(),
        "security-only update should refuse unverifiable metadata"
    );
    let combined = output_text(&output);
    assert!(combined.contains("security metadata"), "{combined}");
    assert!(combined.contains("fedora-slice-b-fixture"), "{combined}");
    assert_eq!(installed_versions(&db_path), vec!["1.0.0".to_string()]);
    assert_eq!(
        active_generation_file_contents(&db_path, "/usr/share/conary-test/hello.txt"),
        b"hello-v1\n"
    );
    assert!(!active_generation_has_path(
        &db_path,
        "/usr/share/conary-test/added.txt"
    ));
}

#[test]
fn security_update_syncs_trusted_json_advisory_and_applies_fix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    seed_selected_root_layout(&db_path, &[]);

    let v1_package = build_ccs_fixture(temp.path(), "v1");
    let v2_package = build_ccs_fixture(temp.path(), "v2");
    install_ccs_into_root(&root, &db_path, &v1_package);

    let (repo_url, server) = serve_security_json_repository(&v2_package);
    let add_repo = run_conary_owned_in_test_home(
        &[
            "repo".to_string(),
            "add".to_string(),
            "security-json-fixture".to_string(),
            repo_url.clone(),
            "--db-path".to_string(),
            db_path.to_string_lossy().into_owned(),
            "--package-format".to_string(),
            "json".to_string(),
            "--source-profile".to_string(),
            "fedora-44".to_string(),
            "--security-advisories".to_string(),
            "supported".to_string(),
        ],
        temp.path(),
    );
    assert_success(&add_repo);

    let sync = run_conary_owned_in_test_home(
        &[
            "repo".to_string(),
            "sync".to_string(),
            "security-json-fixture".to_string(),
            "--db-path".to_string(),
            db_path.to_string_lossy().into_owned(),
            "--force".to_string(),
        ],
        temp.path(),
    );
    assert_success(&sync);

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut repo = Repository::find_by_name(&conn, "security-json-fixture")
        .unwrap()
        .expect("security fixture repo should exist");
    repo.source_profile = Some("fedora-44".to_string());
    authorize_static_ccs_repository(&conn, &mut repo, temp.path());
    conn.execute(
        "UPDATE troves
         SET installed_from_repository_id = ?1, source_profile = 'fedora-44', version_scheme = 'rpm'
         WHERE name = ?2",
        (repo.id.unwrap(), FIXTURE_NAME),
    )
    .unwrap();
    drop(conn);

    let output = run_conary_owned_in_test_home(
        &[
            "update".to_string(),
            FIXTURE_NAME.to_string(),
            "--security".to_string(),
            "--db-path".to_string(),
            db_path.to_string_lossy().into_owned(),
            "--root".to_string(),
            root.to_string_lossy().into_owned(),
            "--sandbox".to_string(),
            "always".to_string(),
            "--yes".to_string(),
        ],
        temp.path(),
    );
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
        active_generation_file_contents(&db_path, "/usr/share/conary-test/hello.txt"),
        b"hello-v2\n"
    );
    assert_eq!(
        active_generation_file_contents(&db_path, "/usr/share/conary-test/added.txt"),
        b"added-in-v2\n"
    );
}
