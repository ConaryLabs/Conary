// apps/conary/tests/packaging_m4e.rs

use std::process::{Command, Output};

#[test]
fn config_noreplace_template_lints_builds_verifies_and_tests() {
    let fixture = M4eFixture::new("config-noreplace");

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev();
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
fn service_template_lints_builds_verifies_and_tests_without_distro_gate() {
    let fixture = M4eFixture::new("service");

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev();
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
fn arbitrary_declarative_lifecycle_is_not_filtered_by_distro_allowlists() {
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
        .arg("--local-dev")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_success(&output);
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

    fn build_v2_local_dev(&self) -> std::path::PathBuf {
        let output = self
            .conary()
            .arg("ccs")
            .arg("build")
            .arg(&self.project)
            .arg("--local-dev")
            .arg("--output")
            .arg(&self.output)
            .output()
            .expect("run conary ccs build");
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

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
