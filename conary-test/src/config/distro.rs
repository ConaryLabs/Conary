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

#[derive(Debug, Deserialize)]
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
        let v1 = raw.v1.unwrap_or(FixtureVersionRaw {
            version: None,
            ccs_file: None,
            hello_sha256: None,
            added_sha256: None,
        });
        let v2 = raw.v2.unwrap_or(FixtureVersionRaw {
            version: None,
            ccs_file: None,
            hello_sha256: None,
            added_sha256: None,
        });

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
