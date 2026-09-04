// apps/conary-test/src/engine/variables.rs

use std::collections::HashMap;

use crate::config::corpus::{CorpusCaseDef, CorpusTargetDef};
use crate::config::distro::GlobalConfig;
use crate::config::manifest::{Assertion, FileChecksum, QemuBoot, QemuGuestCopy, TestManifest};

/// Build the base variable map from global config and distro selection.
///
/// Populates variables from the Remi endpoint, paths, fixture config, and
/// distro-specific test packages. These variables are available to all tests
/// via `${VAR}` substitution in manifest fields.
pub fn build_variables(config: &GlobalConfig, distro: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let conary_binaries = config.paths.resolve_conary_binaries();
    vars.insert("DISTRO".to_string(), distro.to_string());
    vars.insert("REMI_ENDPOINT".to_string(), config.remi.endpoint.clone());
    vars.insert("DB_PATH".to_string(), config.paths.db.clone());
    vars.insert("CONARY_BIN".to_string(), conary_binaries.ordinary);
    vars.insert("CONARY_HOOKS_BIN".to_string(), conary_binaries.test_hooks);
    if let Some(fixture_dir) = &config.paths.fixture_dir {
        vars.insert("FIXTURE_DIR".to_string(), fixture_dir.clone());
        let fixture_root = std::path::Path::new(fixture_dir);
        vars.insert(
            "FIXTURE_CCS_KEY".to_string(),
            crate::paths::fixture_ccs_key_path_for(fixture_root)
                .to_string_lossy()
                .into_owned(),
        );
        vars.insert(
            "FIXTURE_CCS_PUBLIC_KEY".to_string(),
            crate::paths::fixture_ccs_public_key_path_for(fixture_root)
                .to_string_lossy()
                .into_owned(),
        );
        vars.insert(
            "FIXTURE_CCS_POLICY".to_string(),
            crate::paths::fixture_ccs_policy_path_for(fixture_root)
                .to_string_lossy()
                .into_owned(),
        );
        vars.insert(
            "FIXTURE_CCS_EXPIRED_POLICY".to_string(),
            crate::paths::fixture_ccs_expired_policy_path_for(fixture_root)
                .to_string_lossy()
                .into_owned(),
        );
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

/// Resolve manifest variables in the string-bearing corpus authority before
/// recording target evidence.
pub fn expand_corpus_case(
    definition: &CorpusCaseDef,
    vars: &HashMap<String, String>,
) -> CorpusCaseDef {
    CorpusCaseDef {
        evidence_path: expand_variables(&definition.evidence_path, vars),
        source_profile: expand_variables(&definition.source_profile, vars),
        source_format: crate::config::corpus::CorpusSourceFormat::from_value(expand_variables(
            definition.source_format.as_str(),
            vars,
        )),
        digest_source: definition.digest_source,
        target: CorpusTargetDef {
            architecture: expand_variables(&definition.target.architecture, vars),
            init_system: expand_variables(&definition.target.init_system, vars),
            capabilities: definition
                .target
                .capabilities
                .iter()
                .map(|value| expand_variables(value, vars))
                .collect(),
        },
        coverage: definition.coverage.clone(),
        stages: definition.stages.clone(),
    }
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
        stderr_not_contains: assertion
            .stderr_not_contains
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
        local_image_path: config
            .local_image_path
            .as_ref()
            .map(|path| expand_variables(path, vars)),
        image_format: config.image_format,
        stage_conary: config.stage_conary,
        scratch_disk_mb: config.scratch_disk_mb,
        copy_to_guest: config
            .copy_to_guest
            .iter()
            .map(|copy| QemuGuestCopy {
                source: expand_variables(&copy.source, vars),
                dest: expand_variables(&copy.dest, vars),
            })
            .collect(),
        copy_from_guest: config
            .copy_from_guest
            .iter()
            .map(|copy| QemuGuestCopy {
                source: expand_variables(&copy.source, vars),
                dest: expand_variables(&copy.dest, vars),
            })
            .collect(),
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
#[path = "variables/tests.rs"]
mod tests;
