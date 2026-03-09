// conary-test/src/config/mod.rs

pub mod distro;
pub mod manifest;

pub use distro::{DistroConfig, GlobalConfig};
pub use manifest::{Assertion, StepType, TestDef, TestManifest};

use anyhow::Result;
use std::path::Path;

pub fn load_manifest(path: &Path) -> Result<TestManifest> {
    let content = std::fs::read_to_string(path)?;
    let manifest: TestManifest = toml::from_str(&content)?;
    Ok(manifest)
}

pub fn load_global_config(path: &Path) -> Result<GlobalConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: GlobalConfig = toml::from_str(&content)?;
    config.apply_env_overrides()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let toml = r#"
[suite]
name = "smoke"
phase = 1

[[test]]
id = "T01"
name = "health_check"
description = "Verify Remi is reachable"
timeout = 10

[[test.step]]
run = "curl -sf http://localhost:8081/health"

[test.step.assert]
exit_code = 0
stdout_contains = "ok"
"#;
        let manifest: TestManifest = toml::from_str(toml).unwrap();
        assert_eq!(manifest.suite.name, "smoke");
        assert_eq!(manifest.suite.phase, 1);
        assert_eq!(manifest.test.len(), 1);

        let test = &manifest.test[0];
        assert_eq!(test.id, "T01");
        assert_eq!(test.name, "health_check");
        assert_eq!(test.timeout, 10);
        assert_eq!(test.step.len(), 1);

        let step = &test.step[0];
        assert_eq!(step.run.as_deref(), Some("curl -sf http://localhost:8081/health"));
        let assertion = step.assert.as_ref().unwrap();
        assert_eq!(assertion.exit_code, Some(0));
        assert_eq!(assertion.stdout_contains.as_deref(), Some("ok"));
    }

    #[test]
    fn test_parse_multi_step_test() {
        let toml = r#"
[suite]
name = "install"
phase = 1

[[test]]
id = "T05"
name = "install_package"
description = "Install and verify a package"
timeout = 60
fatal = true

[[test.step]]
conary = "install tree"

[test.step.assert]
exit_code = 0

[[test.step]]
file_exists = "/usr/bin/tree"
"#;
        let manifest: TestManifest = toml::from_str(toml).unwrap();
        let test = &manifest.test[0];
        assert_eq!(test.fatal, Some(true));
        assert_eq!(test.step.len(), 2);

        let step0 = &test.step[0];
        assert_eq!(step0.conary.as_deref(), Some("install tree"));
        assert!(matches!(step0.step_type(), Some(StepType::Conary(_))));

        let step1 = &test.step[1];
        assert_eq!(step1.file_exists.as_deref(), Some("/usr/bin/tree"));
        assert!(matches!(step1.step_type(), Some(StepType::FileExists(_))));
    }

    #[test]
    fn test_parse_depends_on() {
        let toml = r#"
[suite]
name = "deps"
phase = 1

[[test]]
id = "T02"
name = "repo_sync"
description = "Sync after adding repo"
timeout = 30
depends_on = ["T01"]

[[test.step]]
conary = "repo sync"
"#;
        let manifest: TestManifest = toml::from_str(toml).unwrap();
        let test = &manifest.test[0];
        let deps = test.depends_on.as_ref().unwrap();
        assert_eq!(deps, &["T01"]);
    }

    #[test]
    fn test_parse_global_config() {
        let toml = r#"
[remi]
endpoint = "https://packages.conary.io"

[paths]
db = "/tmp/conary-test.db"
conary_bin = "/usr/local/bin/conary"
results_dir = "/tmp/results"

[setup]
remove_default_repos = ["fedora", "updates"]

[distros.fedora43]
remi_distro = "fedora43"
repo_name = "conary-fedora43"
containerfile = "Containerfile.fedora43"
test_package_1 = "tree"
test_binary_1 = "/usr/bin/tree"

[distros.ubuntu-noble]
remi_distro = "ubuntu-noble"
repo_name = "conary-ubuntu-noble"
test_package_1 = "tree"

[fixtures]
test_package_name = "conary-test-fixture"
v1_version = "1.0.0"
v2_version = "2.0.0"
"#;
        let config: GlobalConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.remi.endpoint, "https://packages.conary.io");
        assert_eq!(config.paths.db, "/tmp/conary-test.db");
        assert_eq!(config.setup.remove_default_repos, &["fedora", "updates"]);
        assert_eq!(config.distros.len(), 2);

        let fedora = &config.distros["fedora43"];
        assert_eq!(fedora.remi_distro, "fedora43");
        assert_eq!(fedora.test_package_1.as_deref(), Some("tree"));
        assert_eq!(fedora.containerfile.as_deref(), Some("Containerfile.fedora43"));

        let fixtures = config.fixtures.as_ref().unwrap();
        assert_eq!(fixtures.test_package_name.as_deref(), Some("conary-test-fixture"));
        assert_eq!(fixtures.v1_version.as_deref(), Some("1.0.0"));
    }
}
