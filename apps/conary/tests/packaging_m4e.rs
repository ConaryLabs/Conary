// apps/conary/tests/packaging_m4e.rs

use std::process::{Command, Output};

#[test]
fn config_noreplace_template_lints_builds_verifies_and_tests_without_target_profile() {
    let fixture = M4eFixture::new("config-noreplace");

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev(&[]);
    assert!(package.exists());

    let verify = fixture
        .conary()
        .arg("ccs")
        .arg("verify")
        .arg(&package)
        .output()
        .expect("run conary ccs verify");
    assert_success(&verify);

    let test = fixture
        .conary()
        .arg("ccs")
        .arg("test")
        .arg(&package)
        .arg("--dry-run")
        .output()
        .expect("run conary ccs test");
    assert_success(&test);
}

#[test]
fn service_template_lints_builds_verifies_and_tests_with_supported_profile() {
    let fixture = M4eFixture::new("service");

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .arg("--target-profile")
        .arg("fedora-44")
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev(&["--target-profile", "fedora-44"]);
    assert!(package.exists());

    let verify = fixture
        .conary()
        .arg("ccs")
        .arg("verify")
        .arg(&package)
        .output()
        .expect("run conary ccs verify");
    assert_success(&verify);

    let test = fixture
        .conary()
        .arg("ccs")
        .arg("test")
        .arg(&package)
        .arg("--dry-run")
        .arg("--target-profile")
        .arg("fedora-44")
        .output()
        .expect("run conary ccs test");
    assert_success(&test);
}

#[test]
fn service_template_requires_target_profile_for_v2_build() {
    let fixture = M4eFixture::new("service");
    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["target-profile"]);
}

#[test]
fn service_template_rejects_route_slug_as_target_profile() {
    let fixture = M4eFixture::new("service");
    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--target-profile")
        .arg("fedora")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["unsupported target profile", "fedora-44"]);
}

#[test]
fn service_template_rejects_unsupported_target_profile_ids() {
    for target in ["debian", "linux-mint", "fedora-45", "ubuntu-noble"] {
        let fixture = M4eFixture::new("service");
        let output = fixture
            .conary()
            .arg("ccs")
            .arg("build")
            .arg(fixture.project_dir())
            .arg("--format")
            .arg("v2")
            .arg("--local-dev")
            .arg("--target-profile")
            .arg(target)
            .arg("--output")
            .arg(fixture.output_dir())
            .output()
            .expect("run conary ccs build");

        assert_failure_contains(&output, &[target, "target profile"]);
    }
}

#[test]
fn unsupported_lifecycle_entry_fails_with_profile_diagnostic() {
    let fixture = M4eFixture::new("service");
    let manifest_path = fixture.project_dir().join("ccs.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("conary-example.service", "other.service");
    std::fs::write(&manifest_path, text).unwrap();

    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--target-profile")
        .arg("fedora-44")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["lifecycle-unsupported", "other.service"]);
}

struct M4eFixture {
    _work: tempfile::TempDir,
    project: std::path::PathBuf,
    output: std::path::PathBuf,
    home: std::path::PathBuf,
    xdg_data: std::path::PathBuf,
    xdg_config: std::path::PathBuf,
    package_name: String,
}

impl M4eFixture {
    fn new(template: &str) -> Self {
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("project");
        let output = work.path().join("out");
        let home = work.path().join("home");
        let xdg_data = work.path().join("xdg-data");
        let xdg_config = work.path().join("xdg-config");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg_data).unwrap();
        std::fs::create_dir_all(&xdg_config).unwrap();
        let package_name = format!("m4e-{template}");
        let fixture = Self {
            _work: work,
            project,
            output,
            home,
            xdg_data,
            xdg_config,
            package_name,
        };
        let init = fixture
            .conary()
            .arg("ccs")
            .arg("init")
            .arg(&fixture.project)
            .arg("--template")
            .arg(template)
            .arg("--name")
            .arg(&fixture.package_name)
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

    fn build_v2_local_dev(&self, extra_args: &[&str]) -> std::path::PathBuf {
        let mut command = self.conary();
        command
            .arg("ccs")
            .arg("build")
            .arg(&self.project)
            .arg("--format")
            .arg("v2")
            .arg("--local-dev")
            .arg("--output")
            .arg(&self.output);
        for arg in extra_args {
            command.arg(arg);
        }
        let output = command.output().expect("run conary ccs build");
        assert_success(&output);
        self.output
            .join(format!("{}-0.1.0-1.ccs", self.package_name))
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

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\n{}",
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
