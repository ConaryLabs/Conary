// apps/conary/tests/packaging_m4b.rs

use std::process::{Command, Output};

#[test]
fn ccs_build_v2_requires_key_or_local_dev() {
    let fixture = MinimalPackageFixture::new();

    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["--local-dev", "--key"]);
}

#[test]
fn ccs_build_v1_keeps_legacy_output_name() {
    let fixture = MinimalPackageFixture::new();

    let build = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");
    assert_success(&build);

    assert!(fixture.output_dir().join("hello-0.1.0.ccs").exists());
    assert!(!fixture.output_dir().join("hello-0.1.0-1.ccs").exists());
}

#[test]
fn ccs_build_v2_accepts_explicit_release_key_and_policy_verify() {
    let fixture = MinimalPackageFixture::new();
    let key_base = fixture.work.path().join("release-key");
    let private_key = key_base.with_extension("private");
    let public_key = key_base.with_extension("public");
    let policy_path = fixture.work.path().join("release-policy.toml");

    let keygen = fixture
        .conary()
        .arg("ccs")
        .arg("keygen")
        .arg("--output")
        .arg(&key_base)
        .arg("--key-id")
        .arg("release")
        .output()
        .expect("run conary ccs keygen");
    assert_success(&keygen);
    write_trust_policy_from_public_key(&public_key, &policy_path);

    let build = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--key")
        .arg(&private_key)
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");
    assert_success(&build);

    let package = fixture.output_dir().join("hello-0.1.0-1.ccs");
    let verify = fixture
        .conary()
        .arg("ccs")
        .arg("verify")
        .arg(&package)
        .arg("--policy")
        .arg(&policy_path)
        .output()
        .expect("run conary ccs verify");
    assert_success(&verify);
}

struct MinimalPackageFixture {
    work: tempfile::TempDir,
    project: std::path::PathBuf,
    output: std::path::PathBuf,
    home: std::path::PathBuf,
    xdg_data: std::path::PathBuf,
    xdg_config: std::path::PathBuf,
}

impl MinimalPackageFixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("project");
        let output = work.path().join("out");
        let home = work.path().join("home");
        let xdg_data = work.path().join("xdg-data");
        let xdg_config = work.path().join("xdg-config");
        std::fs::create_dir_all(project.join("bin")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg_data).unwrap();
        std::fs::create_dir_all(&xdg_config).unwrap();
        std::fs::write(project.join("bin/hello"), "#!/bin/sh\necho hello\n").unwrap();
        let fixture = Self {
            work,
            project,
            output,
            home,
            xdg_data,
            xdg_config,
        };
        let init = fixture
            .conary()
            .arg("ccs")
            .arg("init")
            .arg(&fixture.project)
            .arg("--template")
            .arg("minimal-file")
            .arg("--name")
            .arg("hello")
            .arg("--version")
            .arg("0.1.0")
            .output()
            .expect("run conary ccs init");
        assert_success(&init);
        fixture
    }

    fn project_dir(&self) -> &std::path::Path {
        &self.project
    }

    fn output_dir(&self) -> &std::path::Path {
        &self.output
    }

    fn conary(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_conary"));
        command
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_CONFIG_HOME", &self.xdg_config);
        command
    }
}

fn write_trust_policy_from_public_key(
    public_key_path: &std::path::Path,
    policy_path: &std::path::Path,
) {
    #[derive(serde::Deserialize)]
    struct PublicKeyFile {
        key: String,
    }

    let key_text = std::fs::read_to_string(public_key_path).unwrap();
    let key: PublicKeyFile = toml::from_str(&key_text).unwrap();
    std::fs::write(
        policy_path,
        format!(
            "trusted_keys = [\"{}\"]\nallow_unsigned = false\nrequire_timestamp = false\n",
            key.key
        ),
    )
    .unwrap();
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\n{}",
        output_text(output)
    );
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
