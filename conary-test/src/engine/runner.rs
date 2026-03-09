// conary-test/src/engine/runner.rs

use crate::config::distro::GlobalConfig;

/// Executes tests from a manifest against a container.
pub struct TestRunner {
    pub config: GlobalConfig,
    pub distro: String,
}

impl TestRunner {
    pub fn new(config: GlobalConfig, distro: String) -> Self {
        Self { config, distro }
    }
}
