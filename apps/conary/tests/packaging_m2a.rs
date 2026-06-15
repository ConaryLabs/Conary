// apps/conary/tests/packaging_m2a.rs

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use conary_core::ccs::archive_reader::read_ccs_archive;
use conary_core::ccs::{CcsManifest, CcsPackage};
use conary_core::packages::PackageFormat;

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

    assert_success(&output);
    assert_stdout_contains(&output, "Cooking and attesting");

    let package_path = fixture.published_package_path();
    let manifest = read_package_manifest(&package_path);
    let provenance = manifest.provenance.expect("provenance");
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    assert!(provenance.hermetic_evidence.is_some());
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

    let archive = read_package_archive(&package_path);
    assert!(
        archive.signature_raw.is_some(),
        "static publish should sign the CCS manifest"
    );

    let manifest_text = read_package_manifest_text(&package_path);
    assert!(manifest_text.contains("build_attestation"));
    assert!(!manifest_text.contains("attested"));
}

#[test]
fn cook_isolated_records_hermetic_evidence() {
    let fixture = RecipeFixture::new(false);
    let config_path = fixture.write_hermetic_config();

    let output = fixture.cook_isolated(&config_path);

    assert_success(&output);
    let manifest = read_package_manifest(&fixture.package_path());
    let provenance = manifest.provenance.expect("provenance");
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    assert!(provenance.hermetic_evidence.is_some());
}

#[test]
fn cook_isolated_blocks_npm_fetch_before_build() {
    let fixture = RecipeFixture::new_npm_fetch();
    let config_path = fixture.write_hermetic_config();

    let output = fixture.cook_isolated(&config_path);

    assert_failure_contains(&output, &["npm", "M2a hermetic support"]);
    assert!(!fixture.package_path().exists());
}

#[test]
fn publish_artifact_form_still_requires_m2b_attestation() {
    let fixture = RecipeFixture::new(false);
    let config_path = fixture.write_hermetic_config();
    let cook_output = fixture.cook_isolated(&config_path);
    assert_success(&cook_output);
    assert!(fixture.package_path().is_file());

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(fixture.package_path())
        .arg(fixture.repo_dir())
        .output()
        .expect("run conary publish artifact form");

    assert_failure_contains(
        &output,
        &["artifact-form publish requires M2 attestation support"],
    );
}

struct RecipeFixture {
    work: tempfile::TempDir,
    recipe_path: PathBuf,
    output_dir: PathBuf,
    source_cache: PathBuf,
    sysroot: PathBuf,
}

impl RecipeFixture {
    fn new(with_build_dependencies: bool) -> Self {
        Self::from_recipe(|project_dir, recipe_path| {
            write_recipe(recipe_path, with_build_dependencies);
            fs::write(project_dir.join("source.txt"), "hello from m2a\n").unwrap();
        })
    }

    fn new_npm_fetch() -> Self {
        Self::from_recipe(|project_dir, recipe_path| {
            fs::write(project_dir.join("package.json"), r#"{"name":"m2a-npm"}"#).unwrap();
            fs::write(
                recipe_path,
                r#"
[package]
name = "m2a-fixture"
version = "1.0"

[source]
path = "."

[build]
install = "npm install atomic-lockfile"
"#,
            )
            .unwrap();
        })
    }

    fn from_recipe(write: impl FnOnce(&Path, &Path)) -> Self {
        let work = tempfile::tempdir().unwrap();
        let project_dir = work.path().join("project");
        let recipe_path = project_dir.join("recipe.toml");
        let output_dir = work.path().join("dist");
        let source_cache = work.path().join("source-cache");
        let sysroot = work.path().join("sysroot");

        fs::create_dir_all(&project_dir).unwrap();
        write(&project_dir, &recipe_path);

        Self {
            work,
            recipe_path,
            output_dir,
            source_cache,
            sysroot,
        }
    }

    fn write_hermetic_config(&self) -> PathBuf {
        write_shell_sysroot(&self.sysroot);
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
        self.output_dir.join("m2a-fixture-1.0-1.ccs")
    }

    fn repo_dir(&self) -> PathBuf {
        self.work.path().join("repo")
    }

    fn key_dir(&self) -> PathBuf {
        self.work.path().join("keys")
    }

    fn state_file(&self) -> PathBuf {
        self.work.path().join("publish-state.toml")
    }

    fn published_package_path(&self) -> PathBuf {
        let package_dir = self.repo_dir().join("packages").join("m2a-fixture");
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
version = "1.0"

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

fn read_package_manifest(package_path: &Path) -> CcsManifest {
    CcsPackage::parse(&package_path.to_string_lossy())
        .unwrap()
        .manifest()
        .clone()
}

fn read_package_archive(
    package_path: &Path,
) -> conary_core::ccs::archive_reader::CcsArchiveContents {
    read_ccs_archive(File::open(package_path).unwrap()).unwrap()
}

fn read_package_manifest_text(package_path: &Path) -> String {
    let archive = read_package_archive(package_path);
    String::from_utf8(archive.toml_raw.expect("MANIFEST.toml")).unwrap()
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
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
