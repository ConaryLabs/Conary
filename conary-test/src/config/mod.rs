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

    #[test]
    fn test_load_phase1_advanced_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase1-advanced.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert!(
                manifest.test.len() >= 27,
                "Expected at least 27 tests (T11-T37), got {}",
                manifest.test.len()
            );

            // Verify suite metadata
            assert_eq!(manifest.suite.phase, 1);

            // Verify T11 remove_package
            let t11 = manifest.test.iter().find(|t| t.id == "T11").unwrap();
            assert_eq!(t11.name, "remove_package");
            assert_eq!(t11.timeout, 60);

            // Verify T15 uses stdout_contains_all
            let t15 = manifest.test.iter().find(|t| t.id == "T15").unwrap();
            let a15 = t15.step[0].assert.as_ref().unwrap();
            assert!(
                a15.stdout_contains_all.is_some(),
                "T15 should use stdout_contains_all"
            );
            let all = a15.stdout_contains_all.as_ref().unwrap();
            assert_eq!(all.len(), 2);

            // Verify T27 uses stdout_contains_all with 3 entries
            let t27 = manifest.test.iter().find(|t| t.id == "T27").unwrap();
            let a27 = t27.step[0].assert.as_ref().unwrap();
            let all27 = a27.stdout_contains_all.as_ref().unwrap();
            assert_eq!(all27.len(), 3);

            // Verify T33 uses run (no_db generation command)
            let t33 = manifest.test.iter().find(|t| t.id == "T33").unwrap();
            assert!(
                t33.step[0].run.is_some(),
                "T33 should use run (no_db generation command)"
            );

            // Verify T35 uses stdout_contains_any
            let t35 = manifest.test.iter().find(|t| t.id == "T35").unwrap();
            let a35 = t35.step[0].assert.as_ref().unwrap();
            assert!(
                a35.stdout_contains_any.is_some(),
                "T35 should use stdout_contains_any"
            );
            let any35 = a35.stdout_contains_any.as_ref().unwrap();
            assert_eq!(any35.len(), 3);

            // Verify T34 uses stdout_contains_if_success
            let t34 = manifest.test.iter().find(|t| t.id == "T34").unwrap();
            let a34 = t34.step[0].assert.as_ref().unwrap();
            assert!(
                a34.stdout_contains_if_success.is_some(),
                "T34 should use stdout_contains_if_success"
            );

            // Verify T37 uses stdout_contains_any_if_success
            let t37 = manifest.test.iter().find(|t| t.id == "T37").unwrap();
            let a37 = t37.step[0].assert.as_ref().unwrap();
            assert!(
                a37.stdout_contains_any_if_success.is_some(),
                "T37 should use stdout_contains_any_if_success"
            );
        }
    }

    #[test]
    fn test_load_phase1_core_manifest() {
        let path = std::path::Path::new("../tests/integration/remi/manifests/phase1-core.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert!(
                manifest.test.len() >= 10,
                "Expected at least 10 tests, got {}",
                manifest.test.len()
            );

            // Verify suite metadata
            assert_eq!(manifest.suite.phase, 1);
            assert!(!manifest.suite.setup.is_empty(), "Expected setup steps");

            // Verify T01 health check
            let t01 = &manifest.test[0];
            assert_eq!(t01.id, "T01");
            assert_eq!(t01.name, "health_check");
            assert_eq!(t01.fatal, Some(true));

            // Verify T04 repo sync is fatal
            let t04 = manifest.test.iter().find(|t| t.id == "T04").unwrap();
            assert_eq!(t04.fatal, Some(true));
            assert_eq!(t04.timeout, 300);

            // Verify T08 has file_executable step
            let t08 = manifest.test.iter().find(|t| t.id == "T08").unwrap();
            assert!(
                t08.step.iter().any(|s| s.file_executable.is_some()),
                "T08 should have a file_executable step"
            );

            // Verify T10 has exit_code_not assertion
            let t10 = manifest.test.iter().find(|t| t.id == "T10").unwrap();
            let assertion = t10.step[0].assert.as_ref().unwrap();
            assert_eq!(assertion.exit_code_not, Some(0));
        }
    }
}
