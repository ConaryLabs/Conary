// apps/conary/tests/packaging_m1b.rs

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::builder::write_ccs_package;
use conary_core::ccs::{
    BuildResult, CcsManifest, CcsPackage, ComponentData, FileEntry as CcsFileEntry, FileType,
};
use conary_core::db::models::{TrySession, TrySessionStatus};
use conary_core::packages::PackageFormat;
use conary_core::recipe::parse_recipe_file;
use conary_core::runtime_root::ConaryRuntimeRoot;

const PACKAGE_NAME: &str = "hello-m1b";
const PACKAGE_VERSION: &str = "0.1.0";
const PACKAGE_RELEASE: &str = "1";

#[test]
fn cook_local_cargo_tree_from_inference_builds_ccs() {
    let fixture = CargoFixture::new();

    let output = cook(
        fixture.source_dir(),
        fixture.output_dir(),
        fixture.source_cache(),
    );
    assert_success(&output);

    assert_cooked_hello_m1b(&fixture.package_path());
}

#[test]
fn cook_local_tarball_from_inference_builds_ccs() {
    let fixture = CargoFixture::new();
    let archive = fixture.work_dir().join("hello-m1b.tar");
    write_tarball(fixture.source_dir(), &archive);

    let output = cook(&archive, fixture.output_dir(), fixture.source_cache());
    assert_success(&output);

    assert_cooked_hello_m1b(&fixture.package_path());
}

#[test]
fn new_from_local_tree_then_cook_recipe_builds_same_package() {
    let fixture = CargoFixture::new();
    let recipe = fixture.work_dir().join("recipe.toml");

    let output = new_from(fixture.source_dir(), &recipe);
    assert_success(&output);
    assert_recipe_uses_local_source_without_locked_build(&recipe);

    let output = cook(&recipe, fixture.output_dir(), fixture.source_cache());
    assert_success(&output);

    assert_cooked_hello_m1b(&fixture.package_path());
}

#[test]
fn new_from_local_tarball_materializes_recipe() {
    let fixture = CargoFixture::new();
    let archive = fixture.work_dir().join("hello-m1b.tar");
    let recipe = fixture.work_dir().join("recipe.toml");
    write_tarball(fixture.source_dir(), &archive);

    let output = new_from(&archive, &recipe);
    assert_success(&output);

    fs::remove_file(&archive).unwrap();
    let materialized_archive = fixture.work_dir().join("sources/hello-m1b.tar");
    assert!(
        materialized_archive.is_file(),
        "expected local archive to be materialized at {}",
        materialized_archive.display()
    );
    assert_recipe_uses_archive_source_without_locked_build(&recipe, "sources/hello-m1b.tar");
}

#[test]
fn new_from_local_tarball_then_cook_recipe_builds_same_package() {
    let fixture = CargoFixture::new();
    let archive = fixture.work_dir().join("hello-m1b.tar");
    let recipe = fixture.work_dir().join("recipe.toml");
    write_tarball(fixture.source_dir(), &archive);

    let output = new_from(&archive, &recipe);
    assert_success(&output);
    fs::remove_file(&archive).unwrap();

    let output = cook(&recipe, fixture.output_dir(), fixture.source_cache());
    assert_success(&output);

    assert_cooked_hello_m1b(&fixture.package_path());
}

#[test]
fn new_from_git_target_materializes_persistent_source_then_cook_builds() {
    let fixture = CargoFixture::new();
    let repo = fixture.work_dir().join("hello-m1b.git");
    let recipe = fixture.work_dir().join("recipe.toml");
    create_git_repo(&repo);

    let output = new_from(&repo, &recipe);
    assert_success(&output);
    fs::remove_dir_all(&repo).unwrap();

    let stable_source = fixture.work_dir().join("sources/hello-m1b.git");
    assert!(
        stable_source.join("Cargo.toml").is_file(),
        "expected git source to be materialized under {}",
        stable_source.display()
    );
    assert!(
        !stable_source.join(".git").exists(),
        "materialized git source must not persist clone metadata"
    );
    assert_recipe_uses_local_source_without_locked_build(&recipe);

    let output = cook(&recipe, fixture.output_dir(), fixture.source_cache());
    assert_success(&output);

    assert_cooked_hello_m1b(&fixture.package_path());
}

#[test]
fn try_package_creates_session() {
    let fixture = try_fixture_package();
    let (_db_temp, db_path) = common::setup_command_test_db();
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path));
    create_current_generation_link(&runtime_root, 7);
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
    create_current_generation_link(&runtime_root, 7);
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
    create_current_generation_link(&runtime_root, 7);

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
    source_cache: PathBuf,
}

impl CargoFixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let work_dir = work.path().to_path_buf();
        let source_dir = work_dir.join("source");
        let output_dir = work_dir.join("dist");
        let source_cache = work_dir.join("source-cache");

        write_cargo_project(&source_dir);

        Self {
            _work: work,
            work_dir,
            source_dir,
            output_dir,
            source_cache,
        }
    }

    fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    fn source_cache(&self) -> &Path {
        &self.source_cache
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

fn write_tarball(source_root: &Path, archive: &Path) {
    let file = fs::File::create(archive).unwrap();
    let mut builder = tar::Builder::new(file);
    builder
        .append_dir_all(format!("{PACKAGE_NAME}-{PACKAGE_VERSION}"), source_root)
        .unwrap();
    builder.finish().unwrap();
}

fn create_git_repo(root: &Path) {
    write_cargo_project(root);
    git(root, &["init"]);
    git(root, &["config", "user.email", "conary@example.invalid"]);
    git(root, &["config", "user.name", "Conary Test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial"]);
}

fn new_from(source: &Path, recipe: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["new", "--from"])
        .arg(source)
        .args(["--output"])
        .arg(recipe)
        .output()
        .expect("failed to run conary new")
}

fn cook(target: &Path, output_dir: &Path, source_cache: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg(target)
        .args(["--output"])
        .arg(output_dir)
        .args(["--source-cache"])
        .arg(source_cache)
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
    let hash = conary_core::hash::sha256(&content);
    let file = CcsFileEntry {
        path: format!("/usr/bin/{PACKAGE_NAME}"),
        hash: hash.clone(),
        size: content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    let total_size = file.size;
    let result = BuildResult {
        manifest: CcsManifest::new_minimal(PACKAGE_NAME, PACKAGE_VERSION),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: vec![file.clone()],
                hash: "runtime".to_string(),
                size: file.size,
            },
        )]),
        files: vec![file],
        blobs: HashMap::from([(hash, content)]),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, package_path).unwrap();
}

fn try_package(package_path: &Path, db_path: &str, command: Option<&str>) -> Output {
    let mut conary = Command::new(env!("CARGO_BIN_EXE_conary"));
    conary
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .env("CONARY_TEST_TRY_LAUNCHER", "echo")
        .arg("try")
        .arg(package_path)
        .arg("--db-path")
        .arg(db_path);
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

fn create_current_generation_link(runtime_root: &ConaryRuntimeRoot, generation: i64) {
    fs::create_dir_all(runtime_root.generation_path(generation)).unwrap();
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

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {:?} failed\n{}",
        args,
        output_text(&output)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn assert_cooked_hello_m1b(package_path: &Path) {
    assert!(
        package_path.is_file(),
        "expected cooked package at {}",
        package_path.display()
    );

    let package = CcsPackage::parse(&package_path.to_string_lossy()).unwrap();
    assert_eq!(package.name(), PACKAGE_NAME);
    assert_eq!(package.version(), PACKAGE_VERSION);
    assert!(
        package
            .files()
            .iter()
            .any(|file| file.path == "/usr/bin/hello-m1b"),
        "expected cooked package to include /usr/bin/hello-m1b"
    );
}

fn assert_recipe_uses_local_source_without_locked_build(recipe_path: &Path) {
    let recipe = parse_recipe_file(recipe_path).unwrap();
    assert_eq!(recipe.package.name, PACKAGE_NAME);
    assert_eq!(recipe.package.version, PACKAGE_VERSION);
    assert!(
        recipe.local_source().is_some(),
        "expected local source recipe"
    );
    assert_cargo_build_omits_locked(recipe_path);
}

fn assert_recipe_uses_archive_source_without_locked_build(recipe_path: &Path, archive: &str) {
    let recipe = parse_recipe_file(recipe_path).unwrap();
    assert_eq!(recipe.package.name, PACKAGE_NAME);
    assert_eq!(recipe.package.version, PACKAGE_VERSION);
    let source = recipe
        .remote_source()
        .expect("expected archive source recipe");
    assert_eq!(source.archive, archive);
    assert!(source.checksum.starts_with("sha256:"));
    assert_cargo_build_omits_locked(recipe_path);
}

fn assert_cargo_build_omits_locked(recipe_path: &Path) {
    let rendered = fs::read_to_string(recipe_path).unwrap();
    assert!(
        rendered.contains("cargo build --release"),
        "expected generated Cargo recipe to build in release mode\n{}",
        rendered
    );
    assert!(
        !rendered.contains("--locked"),
        "generated Cargo recipe must omit --locked when Cargo.lock is absent\n{}",
        rendered
    );
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
