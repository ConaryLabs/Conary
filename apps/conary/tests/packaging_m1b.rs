// apps/conary/tests/packaging_m1b.rs

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::CcsPackage;
use conary_core::packages::PackageFormat;
use conary_core::recipe::parse_recipe_file;

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

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
