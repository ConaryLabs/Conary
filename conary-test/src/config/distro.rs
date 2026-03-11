// conary-test/src/config/distro.rs

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

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
        if let Ok(val) = std::env::var("REMI_ENDPOINT") {
            self.remi.endpoint = val;
        }
        if let Ok(val) = std::env::var("DB_PATH") {
            self.paths.db = val;
        }
        if let Ok(val) = std::env::var("CONARY_BIN") {
            self.paths.conary_bin = val;
        }
        if let Ok(val) = std::env::var("RESULTS_DIR") {
            self.paths.results_dir = val;
        }
        Ok(self)
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
    pub results_dir: String,
    #[serde(default)]
    pub fixture_dir: Option<String>,
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

/// Intermediate struct for deserializing the legacy numbered-field format
/// (`test_package`, `test_binary`, `test_package_2`, `test_binary_2`, ...).
#[derive(Debug, Deserialize)]
struct DistroConfigRaw {
    remi_distro: String,
    repo_name: String,
    #[serde(default)]
    containerfile: Option<String>,
    // New format: array of tables.
    #[serde(default)]
    test_packages: Option<Vec<TestPackage>>,
    // Legacy numbered fields (first pair uses no suffix or `_1`).
    #[serde(default, alias = "test_package_1")]
    test_package: Option<String>,
    #[serde(default, alias = "test_binary_1")]
    test_binary: Option<String>,
    #[serde(default)]
    test_package_2: Option<String>,
    #[serde(default)]
    test_binary_2: Option<String>,
    #[serde(default)]
    test_package_3: Option<String>,
    #[serde(default)]
    test_binary_3: Option<String>,
}

impl From<DistroConfigRaw> for DistroConfig {
    fn from(raw: DistroConfigRaw) -> Self {
        let test_packages = if let Some(pkgs) = raw.test_packages {
            pkgs
        } else {
            let mut pkgs = Vec::new();
            if let (Some(pkg), Some(bin)) = (raw.test_package, raw.test_binary) {
                pkgs.push(TestPackage {
                    package: pkg,
                    binary: bin,
                });
            }
            if let (Some(pkg), Some(bin)) = (raw.test_package_2, raw.test_binary_2) {
                pkgs.push(TestPackage {
                    package: pkg,
                    binary: bin,
                });
            }
            if let (Some(pkg), Some(bin)) = (raw.test_package_3, raw.test_binary_3) {
                pkgs.push(TestPackage {
                    package: pkg,
                    binary: bin,
                });
            }
            pkgs
        };

        DistroConfig {
            remi_distro: raw.remi_distro,
            repo_name: raw.repo_name,
            containerfile: raw.containerfile,
            test_packages,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DistroConfig {
    pub remi_distro: String,
    pub repo_name: String,
    pub containerfile: Option<String>,
    pub test_packages: Vec<TestPackage>,
}

impl<'de> Deserialize<'de> for DistroConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = DistroConfigRaw::deserialize(deserializer)?;
        Ok(Self::from(raw))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureConfig {
    #[serde(default)]
    pub test_package_name: Option<String>,
    #[serde(default)]
    pub marker_file_v1: Option<String>,
    #[serde(default)]
    pub marker_file_v2: Option<String>,
    #[serde(default)]
    pub v1_version: Option<String>,
    #[serde(default)]
    pub v2_version: Option<String>,
}
