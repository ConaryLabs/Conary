// apps/conary/tests/packaging_m1b.rs

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::builder::write_signed_current_ccs_package;
use conary_core::ccs::verify::verify_package;
use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry as CcsFileEntry};
use conary_core::ccs::{CcsPackage, SigningKeyPair, TrustPolicy};
use conary_core::db::models::{TrySession, TrySessionStatus};
use conary_core::payload::{PayloadContentAuthority, PayloadIdentity, PayloadNode};
use conary_core::runtime_root::ConaryRuntimeRoot;

const PACKAGE_NAME: &str = "hello-m1b";
const PACKAGE_VERSION: &str = "0.1.0";
const PACKAGE_RELEASE: &str = "1";

#[test]
fn new_from_and_explain_are_removed_without_compatibility_aliases() {
    let fixture = CargoFixture::new();
    let from = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["new", "hello", "--from"])
        .arg(fixture.source_dir())
        .output()
        .expect("failed to run conary new");
    assert_failure(&from);
    assert!(output_text(&from).contains("unexpected argument '--from'"));

    let explain = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["new", "hello", "--explain"])
        .output()
        .expect("failed to run conary new");
    assert_failure(&explain);
    assert!(output_text(&explain).contains("unexpected argument '--explain'"));
}

#[test]
fn cook_rejects_bare_source_tree_and_url_without_inference() {
    let fixture = CargoFixture::new();
    let source = cook_validate_only(fixture.source_dir());
    assert_failure(&source);
    assert!(output_text(&source).contains("does not contain recipe.toml"));

    let remote = cook_validate_only(Path::new("https://example.invalid/source.tar.gz"));
    assert_failure(&remote);
    assert!(output_text(&remote).contains("requires an explicit recipe file"));
}

#[test]
fn try_package_creates_session() {
    let fixture = try_fixture_package();
    let (_db_temp, db_path) = common::setup_command_test_db();
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path));
    create_current_generation(&db_path);
    let before_current = fs::read_link(runtime_root.current_link()).unwrap();

    let output = try_package(
        &fixture.package_path(),
        &db_path,
        Some("/usr/bin/hello-m1b"),
    );
    assert_success(&output);
    let stdout = stdout_text(&output);
    let session_id = extract_try_session_id(&stdout);

    assert_eq!(
        fs::read_link(runtime_root.current_link()).unwrap(),
        before_current
    );
    let session = active_try_session(&db_path).expect("active try session");
    assert_eq!(session.id, session_id);
    assert_eq!(session.status, TrySessionStatus::Active);
    assert_eq!(session.package_name.as_deref(), Some(PACKAGE_NAME));
    assert_eq!(session.package_version.as_deref(), Some(PACKAGE_VERSION));
    let generation_id = session.try_generation_id.expect("try generation id");

    let second = try_package(&fixture.package_path(), &db_path, None);
    assert_failure(&second);
    assert!(
        output_text(&second).contains(&session_id),
        "second try should name active session\n{}",
        output_text(&second)
    );

    let status = try_action("status", &db_path);
    assert_success(&status);
    let status_stdout = stdout_text(&status);
    assert!(
        status_stdout.contains(&format!("Try session: {session_id}")),
        "{status_stdout}"
    );
    assert!(status_stdout.contains("Status: active"), "{status_stdout}");
    assert!(
        status_stdout.contains("Package: hello-m1b"),
        "{status_stdout}"
    );
    assert!(
        status_stdout.contains(&format!("Generation: {generation_id}")),
        "{status_stdout}"
    );
}

#[test]
fn try_rollback_clears_session() {
    let fixture = try_fixture_package();
    let (_db_temp, db_path) = common::setup_command_test_db();
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path));
    create_current_generation(&db_path);
    let before_current = fs::read_link(runtime_root.current_link()).unwrap();

    let output = try_package(&fixture.package_path(), &db_path, None);
    assert_success(&output);
    let session_id = extract_try_session_id(&stdout_text(&output));

    let rollback = try_action("rollback", &db_path);
    assert_success(&rollback);

    let session = try_session_by_id(&db_path, &session_id).expect("rolled back session");
    assert_eq!(session.status, TrySessionStatus::RolledBack);
    assert_eq!(active_try_session(&db_path), None);
    assert_eq!(
        fs::read_link(runtime_root.current_link()).unwrap(),
        before_current
    );

    let status = try_action("status", &db_path);
    assert_success(&status);
    assert!(stdout_text(&status).contains("No active try session"));
}

#[test]
fn try_keep_promotes_generation() {
    let fixture = try_fixture_package();
    let (_db_temp, db_path) = common::setup_command_test_db();
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path));
    create_current_generation(&db_path);

    let output = try_package(&fixture.package_path(), &db_path, None);
    assert_success(&output);
    let session_id = extract_try_session_id(&stdout_text(&output));
    let try_generation_id = active_try_session(&db_path)
        .expect("active try session")
        .try_generation_id
        .expect("try generation id");

    let keep = try_action("keep", &db_path);
    assert_success(&keep);

    let session = try_session_by_id(&db_path, &session_id).expect("kept session");
    assert_eq!(session.status, TrySessionStatus::Kept);
    assert_eq!(
        conary_core::generation::mount::current_generation(runtime_root.root()).unwrap(),
        Some(try_generation_id)
    );
}

struct CargoFixture {
    _work: tempfile::TempDir,
    work_dir: PathBuf,
    source_dir: PathBuf,
    output_dir: PathBuf,
}

impl CargoFixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let work_dir = work.path().to_path_buf();
        let source_dir = work_dir.join("source");
        let output_dir = work_dir.join("dist");
        write_cargo_project(&source_dir);

        Self {
            _work: work,
            work_dir,
            source_dir,
            output_dir,
        }
    }

    fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    fn package_path(&self) -> PathBuf {
        self.output_dir.join(format!(
            "{PACKAGE_NAME}-{PACKAGE_VERSION}-{PACKAGE_RELEASE}.ccs"
        ))
    }
}

fn write_cargo_project(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "hello-m1b"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rs"),
        r#"fn main() {
    println!("hello m1b");
}
"#,
    )
    .unwrap();
}

fn cook_validate_only(target: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg(target)
        .arg("--validate-only")
        .output()
        .expect("failed to run conary cook")
}

fn try_fixture_package() -> CargoFixture {
    let fixture = CargoFixture::new();
    let target_dir = fixture.work_dir().join("cargo-target");
    let output = Command::new("cargo")
        .args(["build", "--release", "--target-dir"])
        .arg(&target_dir)
        .current_dir(fixture.source_dir())
        .output()
        .expect("failed to build try fixture with cargo");
    assert_success(&output);

    let binary = target_dir.join("release").join(PACKAGE_NAME);
    let content = fs::read(&binary).unwrap_or_else(|error| {
        panic!(
            "failed to read built try fixture binary {}: {error}",
            binary.display()
        )
    });
    write_single_binary_ccs(&fixture.package_path(), content);
    fixture
}

fn write_single_binary_ccs(package_path: &Path, content: Vec<u8>) {
    fs::create_dir_all(package_path.parent().expect("package parent")).unwrap();
    let total_size = content.len() as u64;
    let hash = conary_core::hash::sha256(&content);
    let mut node = PayloadNode::regular(0o755);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    let file = CcsFileEntry {
        path: format!("/usr/bin/{PACKAGE_NAME}"),
        node,
        content: Some(PayloadContentAuthority {
            sha256: hash.clone(),
            size: total_size,
        }),
        component: "runtime".to_string(),
        chunks: None,
    };
    let files = vec![file.clone()];
    let result = BuildResult {
        manifest: CcsManifest::new_minimal(PACKAGE_NAME, PACKAGE_VERSION),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: vec![file.clone()],
                hash: "runtime".to_string(),
                size: total_size,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(hash, content)]),
        )
        .unwrap(),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    let signer = SigningKeyPair::generate().with_key_id("packaging-m1b");
    write_signed_current_ccs_package(&result, package_path, &signer, false).unwrap();
    fs::write(
        package_path.with_extension("policy.toml"),
        format!(
            "trusted_keys = [\"{}\"]\nrequire_timestamp = false\n",
            signer.public_key_base64()
        ),
    )
    .unwrap();
    let verified = verify_package(
        package_path,
        &TrustPolicy::strict(vec![signer.public_key_base64()]),
    )
    .unwrap();
    CcsPackage::from_verified_archive(&package_path.to_string_lossy(), &verified).unwrap();
}

fn try_package(package_path: &Path, db_path: &str, command: Option<&str>) -> Output {
    let mut conary = Command::new(env!("CARGO_BIN_EXE_conary"));
    conary
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .env("CONARY_TEST_TRY_LAUNCHER", "echo")
        .arg("try")
        .arg(package_path)
        .arg("--db-path")
        .arg(db_path)
        .arg("--policy")
        .arg(package_path.with_extension("policy.toml"));
    if let Some(command) = command {
        conary.arg("--").arg(command);
    }
    conary.output().expect("failed to run conary try")
}

fn try_action(action: &str, db_path: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .env("CONARY_TEST_TRY_LAUNCHER", "echo")
        .args(["try", action, "--db-path", db_path])
        .output()
        .expect("failed to run conary try action")
}

fn active_try_session(db_path: &str) -> Option<TrySession> {
    let conn = conary_core::db::open(db_path).unwrap();
    TrySession::find_active_or_orphaned(&conn).unwrap()
}

fn try_session_by_id(db_path: &str, session_id: &str) -> Option<TrySession> {
    let conn = conary_core::db::open(db_path).unwrap();
    TrySession::find_by_id(&conn, session_id).unwrap()
}

fn create_current_generation(db_path: &str) {
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let conn = conary_core::db::open(db_path).unwrap();
    let selected_root = tempfile::tempdir().unwrap();
    conary_core::generation::builder::materialize_selected_root_from_db(
        &conn,
        &runtime_root.objects_dir(),
        selected_root.path(),
    )
    .unwrap();
    let captured = conary_core::generation::root_manifest::scan_selected_root(
        selected_root.path(),
        &conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap(),
    )
    .unwrap();
    let (generation, _) =
        conary_core::generation::builder::build_generation_from_captured_root_with_boot_root_and_activation(
        &conn,
        &runtime_root.generations_dir(),
        "packaging-m1b try-session base",
        &runtime_root.root().join("boot"),
        conary_core::generation::builder::GenerationActivation::Active,
        captured,
    )
    .unwrap();
    conary_core::generation::mount::update_current_symlink(runtime_root.root(), generation)
        .unwrap();
}

fn extract_try_session_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("Try session ")
                .and_then(|rest| rest.strip_suffix(" is active"))
        })
        .unwrap_or_else(|| panic!("missing try session id in stdout:\n{stdout}"))
        .to_string()
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success(), "{}", output_text(output));
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
