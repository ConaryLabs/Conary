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

#[derive(Debug, Clone, Deserialize)]
pub struct DistroConfig {
    pub remi_distro: String,
    pub repo_name: String,
    #[serde(default)]
    pub containerfile: Option<String>,
    #[serde(default)]
    pub test_package_1: Option<String>,
    #[serde(default)]
    pub test_binary_1: Option<String>,
    #[serde(default)]
    pub test_package_2: Option<String>,
    #[serde(default)]
    pub test_binary_2: Option<String>,
    #[serde(default)]
    pub test_package_3: Option<String>,
    #[serde(default)]
    pub test_binary_3: Option<String>,
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
