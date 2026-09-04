// apps/conary-test/src/config/distro.rs

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

use super::release_root::{AuthenticatedTargetRoot, DistroReleaseRoot};

#[derive(Debug, Clone, Deserialize)]
pub struct GlobalConfig {
    pub remi: RemiConfig,
    pub paths: PathsConfig,
    #[serde(default)]
    pub setup: SetupConfig,
    #[serde(default)]
    pub distros: HashMap<String, DistroConfig>,
    #[serde(default)]
    pub fixtures: Option<FixtureConfig>,
}

impl GlobalConfig {
    pub fn apply_env_overrides(mut self) -> Result<Self> {
        self.apply_env_overrides_with(|name| std::env::var(name).ok());
        Ok(self)
    }

    fn apply_env_overrides_with(&mut self, mut lookup: impl FnMut(&str) -> Option<String>) {
        if let Some(val) = lookup("REMI_ENDPOINT") {
            self.remi.endpoint = val;
        }
        if let Some(val) = lookup("DB_PATH") {
            self.paths.db = val;
        }
        if let Some(val) = lookup("CONARY_BIN") {
            self.paths.conary_bin = val;
        }
        if let Some(val) = lookup("CONARY_HOOKS_BIN") {
            self.paths.test_hooks_conary_bin = Some(val);
        }
        if let Some(val) = lookup("RESULTS_DIR") {
            self.paths.results_dir = val;
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemiConfig {
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsConfig {
    pub db: String,
    pub conary_bin: String,
    pub test_hooks_conary_bin: Option<String>,
    pub results_dir: String,
    #[serde(default)]
    pub fixture_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedConaryBinaries {
    pub ordinary: String,
    pub test_hooks: String,
}

impl PathsConfig {
    /// Resolve binary authority only after environment overrides have been
    /// applied to the deserialized optional configuration.
    pub(crate) fn resolve_conary_binaries(&self) -> ResolvedConaryBinaries {
        ResolvedConaryBinaries {
            ordinary: self.conary_bin.clone(),
            test_hooks: self
                .test_hooks_conary_bin
                .clone()
                .unwrap_or_else(|| self.conary_bin.clone()),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SetupConfig {
    #[serde(default)]
    pub remove_default_repos: Vec<String>,
}

/// A test package with its name and the expected binary path.
#[derive(Debug, Clone, Deserialize)]
pub struct TestPackage {
    pub package: String,
    pub binary: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistroConfig {
    pub remi_distro: String,
    pub repo_name: String,
    pub build_context: DistroBuildContext,
    #[serde(default)]
    pub containerfile: Option<String>,
    #[serde(default)]
    pub test_packages: Vec<TestPackage>,
    #[serde(default)]
    pub release_root: Option<DistroReleaseRoot>,
    #[serde(default)]
    pub target_root: Option<AuthenticatedTargetRoot>,
}

/// Which Conary binary a distro image build context stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DistroBuildContext {
    /// Stage the host-built Conary binary and integration fixtures.
    ///
    /// The staged binary keeps the host's glibc and `libseccomp.so.2`
    /// couplings, so it only runs in an image whose userland matches the
    /// build host.
    Binary,
    /// Stage the static `x86_64-unknown-linux-musl` Conary artifact.
    ///
    /// Built by `scripts/build-static-conary.sh`, this artifact carries no
    /// runtime library couplings and therefore runs in any image regardless
    /// of the build host.
    StaticBinary,
}

#[derive(Debug, Clone)]
pub struct FixtureConfig {
    pub package: Option<String>,
    pub file: Option<String>,
    pub added_file: Option<String>,
    pub marker: Option<String>,
    pub v1_version: Option<String>,
    pub v1_ccs_file: Option<String>,
    pub v1_hello_sha256: Option<String>,
    pub v2_version: Option<String>,
    pub v2_ccs_file: Option<String>,
    pub v2_hello_sha256: Option<String>,
    pub v2_added_sha256: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FixtureVersionRaw {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    ccs_file: Option<String>,
    #[serde(default)]
    hello_sha256: Option<String>,
    #[serde(default)]
    added_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureConfigRaw {
    #[serde(default, alias = "test_package_name")]
    package: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    added_file: Option<String>,
    #[serde(default, alias = "marker_file_v1", alias = "marker_file_v2")]
    marker: Option<String>,
    #[serde(default)]
    v1: Option<FixtureVersionRaw>,
    #[serde(default)]
    v2: Option<FixtureVersionRaw>,
    #[serde(default)]
    v1_version: Option<String>,
    #[serde(default)]
    v2_version: Option<String>,
}

impl From<FixtureConfigRaw> for FixtureConfig {
    fn from(raw: FixtureConfigRaw) -> Self {
        let v1 = raw.v1.unwrap_or_default();
        let v2 = raw.v2.unwrap_or_default();

        Self {
            package: raw.package,
            file: raw.file,
            added_file: raw.added_file,
            marker: raw.marker,
            v1_version: raw.v1_version.or(v1.version),
            v1_ccs_file: v1.ccs_file,
            v1_hello_sha256: v1.hello_sha256,
            v2_version: raw.v2_version.or(v2.version),
            v2_ccs_file: v2.ccs_file,
            v2_hello_sha256: v2.hello_sha256,
            v2_added_sha256: v2.added_sha256,
        }
    }
}

impl<'de> Deserialize<'de> for FixtureConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = FixtureConfigRaw::deserialize(deserializer)?;
        Ok(Self::from(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_manifest;
    use crate::engine::variables::{build_variables, expand_variables};
    use std::path::PathBuf;

    fn test_config(test_hooks_conary_bin: Option<&str>) -> GlobalConfig {
        GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://remi.example.test".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/conary-test.db".to_string(),
                conary_bin: "/usr/bin/conary".to_string(),
                test_hooks_conary_bin: test_hooks_conary_bin.map(str::to_string),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: None,
            },
            setup: SetupConfig::default(),
            distros: HashMap::new(),
            fixtures: None,
        }
    }

    fn apply_binary_overrides(
        mut config: GlobalConfig,
        conary_bin: Option<&str>,
        test_hooks_conary_bin: Option<&str>,
    ) -> GlobalConfig {
        config.apply_env_overrides_with(|name| match name {
            "CONARY_BIN" => conary_bin.map(str::to_string),
            "CONARY_HOOKS_BIN" => test_hooks_conary_bin.map(str::to_string),
            _ => None,
        });
        config
    }

    #[test]
    fn conary_bin_override_is_the_implicit_hook_binary_and_reaches_manifests() {
        let config =
            apply_binary_overrides(test_config(None), Some("/opt/conary-under-test"), None);
        let resolved = config.paths.resolve_conary_binaries();
        assert_eq!(resolved.ordinary, "/opt/conary-under-test");
        assert_eq!(resolved.test_hooks, "/opt/conary-under-test");

        let vars = build_variables(&config, "fedora44");
        let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../conary/tests/integration/remi/manifests/native-cross-source-lifecycle.toml");
        let manifest = load_manifest(&manifest_path).expect("load hook-dependent manifest");
        let mut hook_dependent_commands = 0;
        for test in &manifest.test {
            for step in &test.step {
                let Some(command) = &step.run else {
                    continue;
                };
                if !command.contains("${CONARY_HOOKS_BIN}") {
                    continue;
                }
                hook_dependent_commands += 1;
                let expanded = expand_variables(command, &vars);
                assert!(
                    expanded.contains("--conary-bin /opt/conary-under-test"),
                    "ordinary manifest input did not receive CONARY_BIN override: {expanded}"
                );
                assert!(
                    expanded.contains("--test-hooks-conary-bin /opt/conary-under-test"),
                    "implicit hook manifest input did not receive CONARY_BIN override: {expanded}"
                );
            }
        }
        assert!(
            hook_dependent_commands > 0,
            "fixture must contain hook-dependent commands"
        );
    }

    #[test]
    fn explicit_hook_override_wins_over_config_and_conary_override() {
        let config = apply_binary_overrides(
            test_config(Some("/configured/hooks-conary")),
            Some("/opt/conary-under-test"),
            Some("/opt/hooks-conary-under-test"),
        );

        assert_eq!(
            config.paths.resolve_conary_binaries(),
            ResolvedConaryBinaries {
                ordinary: "/opt/conary-under-test".to_string(),
                test_hooks: "/opt/hooks-conary-under-test".to_string(),
            }
        );
    }

    #[test]
    fn configured_hook_binary_wins_when_environment_has_no_binary_overrides() {
        let config =
            apply_binary_overrides(test_config(Some("/configured/hooks-conary")), None, None);

        assert_eq!(
            config.paths.resolve_conary_binaries(),
            ResolvedConaryBinaries {
                ordinary: "/usr/bin/conary".to_string(),
                test_hooks: "/configured/hooks-conary".to_string(),
            }
        );
    }
}
