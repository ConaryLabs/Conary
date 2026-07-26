// apps/conary/tests/packaging_m2a.rs

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::verify::verify_package;
use conary_core::ccs::{CcsPackage, SigningKeyPair, TrustPolicy, VerifiedCcsArchive};

const HASH: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn cook_isolated_fails_without_hermetic_config() {
    let fixture = RecipeFixture::new(false);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg(&fixture.recipe_path)
        .arg("--isolated")
        .arg("--output")
        .arg(&fixture.output_dir)
        .arg("--source-cache")
        .arg(&fixture.source_cache)
        .arg("--key")
        .arg(&fixture.cook_signing_key_path)
        .env_remove("CONARY_HERMETIC_CONFIG")
        .env(
            "XDG_CONFIG_HOME",
            fixture.work.path().join("missing-config"),
        )
        .output()
        .expect("run conary cook --isolated");

    assert_failure_contains(&output, &["hermetic config"]);
    assert!(!fixture.package_path().exists());
}

#[test]
fn cook_isolated_fails_when_build_dependencies_are_declared() {
    let fixture = RecipeFixture::new(true);
    let config_path = fixture.write_hermetic_config();

    let output = fixture.cook_isolated(&config_path);

    assert_failure_contains(&output, &["build dependencies", "content locks"]);
    assert!(!fixture.package_path().exists());
}

#[test]
fn publish_project_form_fails_without_hermetic_config() {
    let fixture = RecipeFixture::new(false);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(fixture.repo_dir())
        .arg("--recipe")
        .arg(&fixture.recipe_path)
        .arg("--key-dir")
        .arg(fixture.key_dir())
        .arg("--state-file")
        .arg(fixture.state_file())
        .arg("--yes")
        .env_remove("CONARY_HERMETIC_CONFIG")
        .env(
            "XDG_CONFIG_HOME",
            fixture.work.path().join("missing-config"),
        )
        .output()
        .expect("run conary publish");

    assert_failure_contains(&output, &["hermetic config"]);
    assert!(!fixture.repo_dir().exists());
}

#[test]
fn publish_project_form_records_hermetic_evidence_with_build_attestation() {
    let fixture = RecipeFixture::new(false);
    let config_path = fixture.write_hermetic_config();

    let output = fixture.publish_project_form(&config_path);

    if !assert_success_or_skip_pristine_container_unavailable(&output) {
        return;
    }
    assert_stdout_contains(&output, "Cooking and attesting");

    let package_path = fixture.published_package_path();
    let package = read_package(&package_path, &fixture.publish_signing_key_path());
    let authority = package.v2_authority().expect("v2 authority");
    assert_eq!(
        authority.provenance.hardening_level.as_deref(),
        Some("hermetic")
    );
    assert!(
        authority
            .provenance
            .hermetic_evidence_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    let provenance = package.manifest().provenance.as_ref().expect("provenance");
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    let attestation = provenance
        .build_attestation
        .as_ref()
        .expect("build attestation");
    assert_eq!(attestation.payload.hardening_level, "hermetic");
    assert_eq!(attestation.payload.origin_class, "native-built");
    assert_eq!(
        attestation.payload.publish_policy_digest,
        "m2-static-publish-policy-v1"
    );
    assert_eq!(attestation.signer_key_id, "publish");

    let verified = verify_package_with_key(&package_path, &fixture.publish_signing_key_path());
    assert_eq!(verified.signature().key_id.as_deref(), Some("publish"));
}

#[test]
fn cook_isolated_records_hermetic_evidence() {
    let fixture = RecipeFixture::new(false);
    let config_path = fixture.write_hermetic_config();

    let output = fixture.cook_isolated(&config_path);

    if !assert_success_or_skip_pristine_container_unavailable(&output) {
        return;
    }
    let package = read_package(&fixture.package_path(), &fixture.cook_signing_key_path);
    let authority = package.v2_authority().expect("v2 authority");
    assert_eq!(
        authority.provenance.hardening_level.as_deref(),
        Some("hermetic")
    );
    assert_eq!(
        authority.identity.architecture.as_deref(),
        Some(std::env::consts::ARCH)
    );
    assert!(
        authority
            .provenance
            .hermetic_evidence_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
}

#[test]
fn cook_isolated_executes_npm_under_runtime_enforcement() {
    let fixture = RecipeFixture::new_npm_fetch();
    let config_path = fixture.write_hermetic_config();

    let output = fixture.cook_isolated(&config_path);

    assert_failure_contains(&output, &["npm runtime reached under sandbox enforcement"]);
    assert!(!output_text(&output).contains("M2a hermetic support"));
    assert!(!fixture.package_path().exists());
}

#[test]
fn publish_artifact_form_accepts_attested_project_artifact() {
    let fixture = RecipeFixture::new(false);
    let config_path = fixture.write_hermetic_config();
    let project_output = fixture.publish_project_form(&config_path);
    if !assert_success_or_skip_pristine_container_unavailable(&project_output) {
        return;
    }

    let artifact_path = fixture.published_package_path();
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(&artifact_path)
        .arg(fixture.artifact_repo_dir())
        .arg("--key-dir")
        .arg(fixture.key_dir())
        .arg("--state-file")
        .arg(fixture.artifact_state_file())
        .output()
        .expect("run conary publish artifact form");

    assert_success(&output);
    assert_stdout_contains(&output, "Published attested artifact");
    let republished = fixture.published_artifact_package_path();
    let package = read_package(&republished, &fixture.publish_signing_key_path());
    assert!(package.v2_build_attestation().is_some());
}

struct RecipeFixture {
    work: tempfile::TempDir,
    recipe_path: PathBuf,
    output_dir: PathBuf,
    source_cache: PathBuf,
    sysroot: PathBuf,
    cook_signing_key_path: PathBuf,
    install_fake_npm: bool,
}

impl RecipeFixture {
    fn new(with_build_dependencies: bool) -> Self {
        Self::from_recipe(
            |project_dir, recipe_path| {
                write_recipe(recipe_path, with_build_dependencies);
                fs::write(project_dir.join("source.txt"), "hello from m2a\n").unwrap();
            },
            false,
        )
    }

    fn new_npm_fetch() -> Self {
        Self::from_recipe(
            |project_dir, recipe_path| {
                fs::write(project_dir.join("package.json"), r#"{"name":"m2a-npm"}"#).unwrap();
                fs::write(
                    recipe_path,
                    r#"
[package]
name = "m2a-fixture"
version = "1.0.0"

[source]
path = "."

[build]
install = "npm install atomic-lockfile"
"#,
                )
                .unwrap();
            },
            true,
        )
    }

    fn from_recipe(write: impl FnOnce(&Path, &Path), install_fake_npm: bool) -> Self {
        let work = tempfile::tempdir().unwrap();
        let project_dir = work.path().join("project");
        let recipe_path = project_dir.join("recipe.toml");
        let output_dir = work.path().join("dist");
        let source_cache = work.path().join("source-cache");
        let sysroot = work.path().join("sysroot");
        let cook_signing_key_path = work.path().join("cook.private");
        SigningKeyPair::generate()
            .with_key_id("cook-test")
            .save_to_files(&cook_signing_key_path, &work.path().join("cook.public"))
            .unwrap();

        fs::create_dir_all(&project_dir).unwrap();
        write(&project_dir, &recipe_path);

        Self {
            work,
            recipe_path,
            output_dir,
            source_cache,
            sysroot,
            cook_signing_key_path,
            install_fake_npm,
        }
    }

    fn write_hermetic_config(&self) -> PathBuf {
        write_shell_sysroot(&self.sysroot);
        if self.install_fake_npm {
            write_executable(
                &self.sysroot.join("bin/npm"),
                "#!/bin/sh\nprintf 'npm runtime reached under sandbox enforcement\\n' >&2\nexit 73\n",
            );
        }
        let config_path = self.work.path().join("hermetic.toml");
        fs::write(
            &config_path,
            format!(
                r#"
default_builder = "test"

[builders.test]
kind = "pristine"
sysroot_path = "{}"
sysroot_hash = "{HASH}"
"#,
                self.sysroot.display()
            ),
        )
        .unwrap();
        config_path
    }

    fn cook_isolated(&self, config_path: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_conary"))
            .arg("cook")
            .arg(&self.recipe_path)
            .arg("--isolated")
            .arg("--output")
            .arg(&self.output_dir)
            .arg("--source-cache")
            .arg(&self.source_cache)
            .arg("--key")
            .arg(&self.cook_signing_key_path)
            .env("CONARY_HERMETIC_CONFIG", config_path)
            .output()
            .expect("run conary cook --isolated")
    }

    fn publish_project_form(&self, config_path: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_conary"))
            .arg("publish")
            .arg(self.repo_dir())
            .arg("--recipe")
            .arg(&self.recipe_path)
            .arg("--key-dir")
            .arg(self.key_dir())
            .arg("--state-file")
            .arg(self.state_file())
            .arg("--yes")
            .env("CONARY_HERMETIC_CONFIG", config_path)
            .output()
            .expect("run conary publish")
    }

    fn package_path(&self) -> PathBuf {
        self.output_dir.join("m2a-fixture-1.0.0-1.ccs")
    }

    fn repo_dir(&self) -> PathBuf {
        self.work.path().join("repo")
    }

    fn artifact_repo_dir(&self) -> PathBuf {
        self.work.path().join("artifact-repo")
    }

    fn key_dir(&self) -> PathBuf {
        self.work.path().join("keys")
    }

    fn state_file(&self) -> PathBuf {
        self.work.path().join("publish-state.toml")
    }

    fn publish_signing_key_path(&self) -> PathBuf {
        self.key_dir().join("publish.private")
    }

    fn artifact_state_file(&self) -> PathBuf {
        self.work.path().join("artifact-publish-state.toml")
    }

    fn published_package_path(&self) -> PathBuf {
        self.published_package_path_in(&self.repo_dir())
    }

    fn published_artifact_package_path(&self) -> PathBuf {
        self.published_package_path_in(&self.artifact_repo_dir())
    }

    fn published_package_path_in(&self, repo_dir: &Path) -> PathBuf {
        let package_dir = repo_dir.join("packages").join("m2a-fixture");
        fs::read_dir(&package_dir)
            .unwrap_or_else(|error| panic!("read published package dir {package_dir:?}: {error}"))
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "ccs"))
            .expect("published package")
    }
}

fn write_recipe(recipe_path: &Path, with_build_dependencies: bool) {
    let deps = if with_build_dependencies {
        r#"requires = ["make"]
makedepends = ["gcc"]
"#
    } else {
        ""
    };
    fs::write(
        recipe_path,
        format!(
            r#"
[package]
name = "m2a-fixture"
version = "1.0.0"

[source]
path = "."

[build]
{deps}install = "printf 'hello from m2a\n' > %(destdir)s/hello.txt"
"#
        ),
    )
    .unwrap();
}

fn write_shell_sysroot(sysroot: &Path) {
    copy_tool_with_runtime_deps(Path::new("/bin/sh"), sysroot, Path::new("bin/sh"));
}

fn copy_tool_with_runtime_deps(tool: &Path, sysroot: &Path, target_relative: &Path) {
    copy_host_file_into_sysroot(tool, sysroot, target_relative);
    for dependency in ldd_paths(tool) {
        copy_host_file_into_sysroot(&dependency, sysroot, dependency.strip_prefix("/").unwrap());
    }
}

fn copy_host_file_into_sysroot(source: &Path, sysroot: &Path, target_relative: &Path) {
    let destination = sysroot.join(target_relative);
    fs::create_dir_all(destination.parent().expect("sysroot file parent")).unwrap();
    fs::copy(source, &destination)
        .unwrap_or_else(|error| panic!("copy {source:?} to {destination:?}: {error}"));
    let mut permissions = fs::metadata(&destination).unwrap().permissions();
    permissions.set_mode(fs::metadata(source).unwrap().permissions().mode() | 0o555);
    fs::set_permissions(&destination, permissions).unwrap();
}

fn write_executable(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("executable parent")).unwrap();
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn ldd_paths(binary: &Path) -> Vec<PathBuf> {
    let output = Command::new("ldd")
        .arg(binary)
        .output()
        .unwrap_or_else(|error| panic!("run ldd {binary:?}: {error}"));
    if !output.status.success() {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut paths = Vec::new();
    for line in text.lines() {
        for token in line.split_whitespace() {
            let token = token.trim_end_matches(':');
            if token.starts_with('/') {
                let path = PathBuf::from(token);
                if path.exists() && !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

fn verify_package_with_key(package_path: &Path, signing_key_path: &Path) -> VerifiedCcsArchive {
    let signer = SigningKeyPair::load_from_file(signing_key_path).unwrap();
    verify_package(
        package_path,
        &TrustPolicy::strict(vec![signer.public_key_base64()]),
    )
    .unwrap()
}

fn read_package(package_path: &Path, signing_key_path: &Path) -> CcsPackage {
    let verified = verify_package_with_key(package_path, signing_key_path);
    CcsPackage::from_verified_archive(&package_path.to_string_lossy(), &verified).unwrap()
}

#[test]
fn pristine_container_unavailable_text_detects_ci_mount_failure() {
    let ci_failure = "Error: install phase failed with exit code 127\nstderr:\n\
        RTNETLINK answers: Operation not permitted\n\
        warning: sethostname failed: Operation not permitted\n\
        Scriptlet error: mount --make-rprivate failed: EACCES: Permission denied";

    assert!(pristine_container_unavailable_text(ci_failure));
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_success_or_skip_pristine_container_unavailable(output: &Output) -> bool {
    if output.status.success() {
        return true;
    }

    let combined = output_text(output);
    if pristine_container_unavailable_text(&combined) {
        eprintln!(
            "skipping M2a pristine builder assertion on a host without mount namespace privileges"
        );
        return false;
    }

    panic!("{combined}");
}

fn assert_stdout_contains(output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "expected stdout to contain {needle:?}\n{}",
        output_text(output)
    );
}

fn assert_failure_contains(output: &Output, needles: &[&str]) {
    assert!(
        !output.status.success(),
        "expected command to fail\n{}",
        output_text(output)
    );
    let combined = output_text(output);
    for needle in needles {
        assert!(
            combined.contains(needle),
            "expected output to contain {needle:?}\n{combined}"
        );
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn pristine_container_unavailable_text(combined: &str) -> bool {
    combined.contains("mount --make-rprivate failed: EACCES")
        || combined.contains("mount --make-rprivate failed: EPERM")
        || (combined.contains("mount --make-rprivate failed")
            && (combined.contains("Operation not permitted")
                || combined.contains("Permission denied")))
}
