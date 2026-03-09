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

    #[test]
    fn test_load_phase2_group_a_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase2-group-a.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert_eq!(manifest.suite.phase, 2);
            assert!(
                manifest.test.len() >= 13,
                "Expected at least 13 tests (T38-T50), got {}",
                manifest.test.len()
            );

            // Verify T38 is fatal
            let t38 = manifest.test.iter().find(|t| t.id == "T38").unwrap();
            assert_eq!(t38.name, "install_fixture_v1_with_deps");
            assert_eq!(t38.fatal, Some(true));
            assert_eq!(t38.group.as_deref(), Some("A"));

            // Verify T39 has dir_exists step
            let t39 = manifest.test.iter().find(|t| t.id == "T39").unwrap();
            assert!(
                t39.step.iter().any(|s| s.dir_exists.is_some()),
                "T39 should have a dir_exists step"
            );

            // Verify T40 has file_checksum step
            let t40 = manifest.test.iter().find(|t| t.id == "T40").unwrap();
            assert!(
                t40.step.iter().any(|s| s.file_checksum.is_some()),
                "T40 should have a file_checksum step"
            );

            // Verify T42 has file_not_exists steps
            let t42 = manifest.test.iter().find(|t| t.id == "T42").unwrap();
            let not_exists_count = t42.step.iter().filter(|s| s.file_not_exists.is_some()).count();
            assert_eq!(not_exists_count, 2, "T42 should have 2 file_not_exists steps");

            // Verify T48 depends on T47
            let t48 = manifest.test.iter().find(|t| t.id == "T48").unwrap();
            assert_eq!(t48.depends_on.as_ref().unwrap(), &["T47"]);
        }
    }

    #[test]
    fn test_load_phase2_group_b_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase2-group-b.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert_eq!(manifest.suite.phase, 2);
            assert!(
                manifest.test.len() >= 7,
                "Expected at least 7 tests (T51-T57), got {}",
                manifest.test.len()
            );

            // Verify setup step installs fixture
            assert!(!manifest.suite.setup.is_empty(), "Expected setup steps");

            // Verify T51 is fatal and uses run (no_db)
            let t51 = manifest.test.iter().find(|t| t.id == "T51").unwrap();
            assert_eq!(t51.fatal, Some(true));
            assert_eq!(t51.group.as_deref(), Some("B"));
            assert!(t51.step[0].run.is_some(), "T51 should use run (no_db)");

            // Verify T57 checks for no panic
            let t57 = manifest.test.iter().find(|t| t.id == "T57").unwrap();
            let a57 = t57.step[0].assert.as_ref().unwrap();
            assert_eq!(a57.stdout_not_contains.as_deref(), Some("panic"));
        }
    }

    #[test]
    fn test_load_phase2_group_c_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase2-group-c.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert_eq!(manifest.suite.phase, 2);
            assert!(
                manifest.test.len() >= 4,
                "Expected at least 4 tests (T58-T61), got {}",
                manifest.test.len()
            );

            // Verify setup creates directories
            assert!(!manifest.suite.setup.is_empty(), "Expected setup steps");

            // Verify T58 uses stdout_contains_if_success
            let t58 = manifest.test.iter().find(|t| t.id == "T58").unwrap();
            assert_eq!(t58.group.as_deref(), Some("C"));
            let a58 = t58.step[0].assert.as_ref().unwrap();
            assert!(
                a58.stdout_contains_if_success.is_some(),
                "T58 should use stdout_contains_if_success"
            );

            // Verify T61 uses run (shell command with timeout)
            let t61 = manifest.test.iter().find(|t| t.id == "T61").unwrap();
            assert!(t61.step[0].run.is_some(), "T61 should use run");
        }
    }

    #[test]
    fn test_load_phase2_group_d_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase2-group-d.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert_eq!(manifest.suite.phase, 2);
            assert!(
                manifest.test.len() >= 5,
                "Expected at least 5 tests (T62-T66), got {}",
                manifest.test.len()
            );

            // Verify T62 is fatal
            let t62 = manifest.test.iter().find(|t| t.id == "T62").unwrap();
            assert_eq!(t62.fatal, Some(true));
            assert_eq!(t62.group.as_deref(), Some("D"));

            // Verify T66 uses exit_code_not and stdout_contains_any
            let t66 = manifest.test.iter().find(|t| t.id == "T66").unwrap();
            let a66 = t66.step[0].assert.as_ref().unwrap();
            assert_eq!(a66.exit_code_not, Some(0), "T66 should require non-zero exit");
            assert!(
                a66.stdout_contains_any.is_some(),
                "T66 should use stdout_contains_any"
            );
            let any66 = a66.stdout_contains_any.as_ref().unwrap();
            assert!(
                any66.len() >= 5,
                "T66 should check for at least 5 keywords"
            );
        }
    }

    #[test]
    fn test_load_phase2_group_e_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase2-group-e.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert_eq!(manifest.suite.phase, 2);
            assert!(
                manifest.test.len() >= 5,
                "Expected at least 5 tests (T67-T71), got {}",
                manifest.test.len()
            );

            // Verify T67 uses run (curl command)
            let t67 = manifest.test.iter().find(|t| t.id == "T67").unwrap();
            assert_eq!(t67.group.as_deref(), Some("E"));
            assert!(t67.step[0].run.is_some(), "T67 should use run (curl)");

            // Verify T68 has file_exists step
            let t68 = manifest.test.iter().find(|t| t.id == "T68").unwrap();
            assert!(
                t68.step.iter().any(|s| s.file_exists.is_some()),
                "T68 should verify file exists after install"
            );

            // Verify T69 uses stdout_contains_any for HTTP code check
            let t69 = manifest.test.iter().find(|t| t.id == "T69").unwrap();
            let a69 = t69.step[0].assert.as_ref().unwrap();
            assert!(
                a69.stdout_contains_any.is_some(),
                "T69 should use stdout_contains_any"
            );

            // Verify T71 checks for "packages"
            let t71 = manifest.test.iter().find(|t| t.id == "T71").unwrap();
            let a71 = t71.step[0].assert.as_ref().unwrap();
            assert_eq!(a71.stdout_contains.as_deref(), Some("packages"));
        }
    }

    #[test]
    fn test_load_phase2_group_f_manifest() {
        let path =
            std::path::Path::new("../tests/integration/remi/manifests/phase2-group-f.toml");
        if path.exists() {
            let manifest = load_manifest(path).unwrap();
            assert_eq!(manifest.suite.phase, 2);
            assert!(
                manifest.test.len() >= 5,
                "Expected at least 5 tests (T72-T76), got {}",
                manifest.test.len()
            );

            // Verify T72 uses stdout_contains_all
            let t72 = manifest.test.iter().find(|t| t.id == "T72").unwrap();
            assert_eq!(t72.group.as_deref(), Some("F"));
            let a72 = t72.step[0].assert.as_ref().unwrap();
            let all72 = a72.stdout_contains_all.as_ref().unwrap();
            assert_eq!(all72.len(), 2);
            assert!(all72.contains(&"packages.conary.io".to_string()));
            assert!(all72.contains(&"(default)".to_string()));

            // Verify T73 sets custom channel and checks for it
            let t73 = manifest.test.iter().find(|t| t.id == "T73").unwrap();
            assert_eq!(t73.step.len(), 2, "T73 should have 2 steps (set + verify)");

            // Verify T75 checks for version info
            let t75 = manifest.test.iter().find(|t| t.id == "T75").unwrap();
            let a75 = t75.step[0].assert.as_ref().unwrap();
            let all75 = a75.stdout_contains_all.as_ref().unwrap();
            assert!(all75.contains(&"Current version:".to_string()));
            assert!(all75.contains(&"Update channel:".to_string()));

            // Verify T76 uses run (complex mock server script)
            let t76 = manifest.test.iter().find(|t| t.id == "T76").unwrap();
            assert!(t76.step[0].run.is_some(), "T76 should use run");
            let a76 = t76.step[0].assert.as_ref().unwrap();
            let all76 = a76.stdout_contains_all.as_ref().unwrap();
            assert!(all76.contains(&"Update available".to_string()));
            assert!(all76.contains(&"99.0.0".to_string()));
        }
    }
}
