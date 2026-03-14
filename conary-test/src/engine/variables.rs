// conary-test/src/engine/variables.rs

use std::collections::HashMap;

use crate::config::distro::GlobalConfig;
use crate::config::manifest::{Assertion, FileChecksum, QemuBoot, TestManifest};

/// Build the base variable map from global config and distro selection.
///
/// Populates variables from the Remi endpoint, paths, fixture config, and
/// distro-specific test packages. These variables are available to all tests
/// via `${VAR}` substitution in manifest fields.
pub fn build_variables(config: &GlobalConfig, distro: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    vars.insert("REMI_ENDPOINT".to_string(), config.remi.endpoint.clone());
    vars.insert("DB_PATH".to_string(), config.paths.db.clone());
    vars.insert("CONARY_BIN".to_string(), config.paths.conary_bin.clone());
    if let Some(fixture_dir) = &config.paths.fixture_dir {
        vars.insert("FIXTURE_DIR".to_string(), fixture_dir.clone());
    }

    if let Some(fixtures) = &config.fixtures {
        if let Some(value) = &fixtures.package {
            vars.insert("FIXTURE_PKG_NAME".to_string(), value.clone());
        }
        if let Some(value) = &fixtures.file {
            vars.insert("FIXTURE_FILE".to_string(), value.clone());
        }
        if let Some(value) = &fixtures.added_file {
            vars.insert("FIXTURE_ADDED_FILE".to_string(), value.clone());
        }
        if let Some(value) = &fixtures.marker {
            vars.insert("FIXTURE_MARKER".to_string(), value.clone());
        }
        if let Some(fixture_dir) = &config.paths.fixture_dir {
            if let Some(value) = &fixtures.v1_ccs_file {
                vars.insert(
                    "FIXTURE_V1_CCS".to_string(),
                    format!("{fixture_dir}/conary-test-fixture/v1/output/{value}"),
                );
            }
            if let Some(value) = &fixtures.v2_ccs_file {
                vars.insert(
                    "FIXTURE_V2_CCS".to_string(),
                    format!("{fixture_dir}/conary-test-fixture/v2/output/{value}"),
                );
            }
        }
        if let Some(value) = &fixtures.v1_hello_sha256 {
            vars.insert("FIXTURE_V1_HELLO_SHA256".to_string(), value.clone());
        }
        if let Some(value) = &fixtures.v2_hello_sha256 {
            vars.insert("FIXTURE_V2_HELLO_SHA256".to_string(), value.clone());
        }
        if let Some(value) = &fixtures.v2_added_sha256 {
            vars.insert("FIXTURE_V2_ADDED_SHA256".to_string(), value.clone());
        }
    }

    // Add distro-specific variables if present.
    if let Some(dc) = config.distros.get(distro) {
        vars.insert("REMI_DISTRO".to_string(), dc.remi_distro.clone());
        vars.insert("REPO_NAME".to_string(), dc.repo_name.clone());
        for (i, tp) in dc.test_packages.iter().enumerate() {
            let n = i + 1;
            vars.insert(format!("TEST_PACKAGE_{n}"), tp.package.clone());
            vars.insert(format!("TEST_BINARY_{n}"), tp.binary.clone());
        }
    }

    vars
}

/// Load distro-specific manifest overrides into an existing variable map.
pub fn load_manifest_overrides(
    vars: &mut HashMap<String, String>,
    manifest: &TestManifest,
    distro: &str,
) {
    if let Some(overrides) = manifest.distro_overrides.get(distro) {
        vars.extend(overrides.clone());
    }
}

/// Replace `${VAR}` patterns in a string with values from the variable map.
///
/// Variables that are not present in the map are left as-is (the `${VAR}`
/// placeholder remains in the output).
pub fn expand_variables(input: &str, vars: &HashMap<String, String>) -> String {
    if !input.contains("${") {
        return input.to_string();
    }
    let mut result = input.to_string();
    for (key, value) in vars {
        let pattern = format!("${{{key}}}");
        result = result.replace(&pattern, value);
    }
    result
}

/// Expand all variable references in an `Assertion`.
pub fn expand_assertion(assertion: &Assertion, vars: &HashMap<String, String>) -> Assertion {
    Assertion {
        exit_code: assertion.exit_code,
        exit_code_not: assertion.exit_code_not,
        stdout_contains: assertion
            .stdout_contains
            .as_ref()
            .map(|value| expand_variables(value, vars)),
        stdout_not_contains: assertion
            .stdout_not_contains
            .as_ref()
            .map(|value| expand_variables(value, vars)),
        stdout_contains_all: assertion.stdout_contains_all.as_ref().map(|values| {
            values
                .iter()
                .map(|value| expand_variables(value, vars))
                .collect()
        }),
        stdout_contains_any: assertion.stdout_contains_any.as_ref().map(|values| {
            values
                .iter()
                .map(|value| expand_variables(value, vars))
                .collect()
        }),
        stdout_contains_if_success: assertion
            .stdout_contains_if_success
            .as_ref()
            .map(|value| expand_variables(value, vars)),
        stdout_contains_any_if_success: assertion.stdout_contains_any_if_success.as_ref().map(
            |values| {
                values
                    .iter()
                    .map(|value| expand_variables(value, vars))
                    .collect()
            },
        ),
        stderr_contains: assertion
            .stderr_contains
            .as_ref()
            .map(|value| expand_variables(value, vars)),
        file_exists: assertion
            .file_exists
            .as_ref()
            .map(|value| expand_variables(value, vars)),
        file_not_exists: assertion
            .file_not_exists
            .as_ref()
            .map(|value| expand_variables(value, vars)),
        file_checksum: assertion
            .file_checksum
            .as_ref()
            .map(|checksum| FileChecksum {
                path: expand_variables(&checksum.path, vars),
                sha256: expand_variables(&checksum.sha256, vars),
            }),
    }
}

/// Expand all variable references in a `QemuBoot` configuration.
pub fn expand_qemu_boot(config: &QemuBoot, vars: &HashMap<String, String>) -> QemuBoot {
    QemuBoot {
        image: expand_variables(&config.image, vars),
        memory_mb: config.memory_mb,
        timeout_seconds: config.timeout_seconds,
        ssh_port: config.ssh_port,
        commands: config
            .commands
            .iter()
            .map(|cmd| expand_variables(cmd, vars))
            .collect(),
        expect_output: config
            .expect_output
            .iter()
            .map(|s| expand_variables(s, vars))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::distro::{
        DistroConfig, FixtureConfig, GlobalConfig, PathsConfig, RemiConfig, SetupConfig,
        TestPackage,
    };

    fn test_config() -> GlobalConfig {
        let mut distros = HashMap::new();
        distros.insert(
            "fedora43".to_string(),
            DistroConfig {
                remi_distro: "fedora-43".to_string(),
                repo_name: "fedora-remi".to_string(),
                containerfile: None,
                test_packages: vec![TestPackage {
                    package: "conary-test-fixture".to_string(),
                    binary: "/usr/bin/true".to_string(),
                }],
            },
        );

        GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://packages.conary.io".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/conary-test.db".to_string(),
                conary_bin: "/usr/local/bin/conary".to_string(),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: Some("/opt/remi-tests/fixtures".to_string()),
            },
            setup: SetupConfig::default(),
            distros,
            fixtures: Some(FixtureConfig {
                package: Some("conary-test-fixture".to_string()),
                file: Some("/usr/share/conary-test/hello.txt".to_string()),
                added_file: Some("/usr/share/conary-test/added.txt".to_string()),
                marker: Some("/var/lib/conary-test/installed".to_string()),
                v1_version: Some("1.0.0".to_string()),
                v1_ccs_file: Some("conary-test-fixture-1.0.0.ccs".to_string()),
                v1_hello_sha256: Some(
                    "18933c865fcf7230f8ea99b059747facc14285b7ed649758115f9c9a73f42a53".to_string(),
                ),
                v2_version: Some("2.0.0".to_string()),
                v2_ccs_file: Some("conary-test-fixture-2.0.0.ccs".to_string()),
                v2_hello_sha256: Some(
                    "bd80c5e8a7138bd13d0f10e1358bda6f9727c266b6909d4b6c9293ab141ec1db".to_string(),
                ),
                v2_added_sha256: Some(
                    "9767b0b4d55db9aee6638c9875b5cefea50c952cc77fbc5703ebc866b0daba3c".to_string(),
                ),
            }),
        }
    }

    #[test]
    fn test_basic_expansion() {
        let mut vars = HashMap::new();
        vars.insert("NAME".to_string(), "world".to_string());
        assert_eq!(expand_variables("hello ${NAME}", &vars), "hello world");
    }

    #[test]
    fn test_missing_variable_left_as_is() {
        let vars = HashMap::new();
        assert_eq!(
            expand_variables("hello ${MISSING}", &vars),
            "hello ${MISSING}"
        );
    }

    #[test]
    fn test_empty_template() {
        let vars = HashMap::new();
        assert_eq!(expand_variables("", &vars), "");
    }

    #[test]
    fn test_no_variables_in_input() {
        let mut vars = HashMap::new();
        vars.insert("KEY".to_string(), "value".to_string());
        assert_eq!(expand_variables("no vars here", &vars), "no vars here");
    }

    #[test]
    fn test_multiple_variables() {
        let mut vars = HashMap::new();
        vars.insert("A".to_string(), "1".to_string());
        vars.insert("B".to_string(), "2".to_string());
        assert_eq!(expand_variables("${A} and ${B}", &vars), "1 and 2");
    }

    #[test]
    fn test_build_variables_populates_core_fields() {
        let config = test_config();
        let vars = build_variables(&config, "fedora43");

        assert_eq!(vars["REMI_ENDPOINT"], "https://packages.conary.io");
        assert_eq!(vars["DB_PATH"], "/tmp/conary-test.db");
        assert_eq!(vars["CONARY_BIN"], "/usr/local/bin/conary");
        assert_eq!(vars["REMI_DISTRO"], "fedora-43");
        assert_eq!(vars["REPO_NAME"], "fedora-remi");
        assert_eq!(vars["TEST_PACKAGE_1"], "conary-test-fixture");
        assert_eq!(vars["TEST_BINARY_1"], "/usr/bin/true");
        assert_eq!(vars["FIXTURE_PKG_NAME"], "conary-test-fixture");
    }

    #[test]
    fn test_build_variables_fixture_ccs_paths() {
        let config = test_config();
        let vars = build_variables(&config, "fedora43");

        assert_eq!(
            vars["FIXTURE_V1_CCS"],
            "/opt/remi-tests/fixtures/conary-test-fixture/v1/output/conary-test-fixture-1.0.0.ccs"
        );
        assert_eq!(
            vars["FIXTURE_V2_CCS"],
            "/opt/remi-tests/fixtures/conary-test-fixture/v2/output/conary-test-fixture-2.0.0.ccs"
        );
    }

    #[test]
    fn test_build_variables_unknown_distro() {
        let config = test_config();
        let vars = build_variables(&config, "unknown-distro");

        // Core fields still present.
        assert_eq!(vars["REMI_ENDPOINT"], "https://packages.conary.io");
        // Distro-specific fields absent.
        assert!(!vars.contains_key("REMI_DISTRO"));
        assert!(!vars.contains_key("TEST_PACKAGE_1"));
    }

    #[test]
    fn test_distro_override_precedence() {
        let config = test_config();
        let mut vars = build_variables(&config, "fedora43");

        // Simulate a manifest that overrides REMI_ENDPOINT.
        let mut manifest = TestManifest {
            suite: crate::config::manifest::SuiteDef {
                name: "test".to_string(),
                phase: 1,
                setup: Vec::new(),
                mock_server: None,
            },
            test: Vec::new(),
            distro_overrides: HashMap::new(),
        };
        manifest.distro_overrides.insert(
            "fedora43".to_string(),
            HashMap::from([("REMI_ENDPOINT".to_string(), "http://override".to_string())]),
        );

        load_manifest_overrides(&mut vars, &manifest, "fedora43");
        assert_eq!(vars["REMI_ENDPOINT"], "http://override");
    }

    #[test]
    fn test_expand_assertion_substitutes_vars() {
        let mut vars = HashMap::new();
        vars.insert("PKG".to_string(), "conary-test-fixture".to_string());
        vars.insert("HELLO_SHA".to_string(), "abc123".to_string());

        let assertion = Assertion {
            stdout_contains_all: Some(vec!["${PKG}".to_string(), "Version".to_string()]),
            stderr_contains: Some("${PKG}".to_string()),
            file_checksum: Some(FileChecksum {
                path: "/tmp/${PKG}".to_string(),
                sha256: "${HELLO_SHA}".to_string(),
            }),
            ..Assertion::default()
        };

        let expanded = expand_assertion(&assertion, &vars);
        assert_eq!(
            expanded.stdout_contains_all,
            Some(vec![
                "conary-test-fixture".to_string(),
                "Version".to_string()
            ])
        );
        assert_eq!(
            expanded.stderr_contains.as_deref(),
            Some("conary-test-fixture")
        );
        assert_eq!(
            expanded.file_checksum.as_ref().map(|chk| chk.path.as_str()),
            Some("/tmp/conary-test-fixture")
        );
        assert_eq!(
            expanded
                .file_checksum
                .as_ref()
                .map(|chk| chk.sha256.as_str()),
            Some("abc123")
        );
    }

    #[test]
    fn test_expand_qemu_boot_substitutes_vars() {
        let mut vars = HashMap::new();
        vars.insert("IMG".to_string(), "minimal-boot-v1".to_string());

        let expanded = expand_qemu_boot(
            &QemuBoot {
                image: "${IMG}".to_string(),
                memory_mb: 1024,
                timeout_seconds: 120,
                ssh_port: 2222,
                commands: vec!["echo ${IMG}".to_string()],
                expect_output: vec!["${IMG}".to_string()],
            },
            &vars,
        );

        assert_eq!(expanded.image, "minimal-boot-v1");
        assert_eq!(expanded.commands, vec!["echo minimal-boot-v1"]);
        assert_eq!(expanded.expect_output, vec!["minimal-boot-v1"]);
    }
}
