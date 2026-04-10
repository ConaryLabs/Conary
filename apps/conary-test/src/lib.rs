// conary-test/src/lib.rs

pub mod build_info;
pub mod config;
pub mod container;
pub mod deploy;
pub mod engine;
pub mod error;
pub mod error_taxonomy;
pub mod paths;
pub mod report;
pub mod server;

#[cfg(test)]
pub mod test_fixtures {
    use crate::config::distro::*;
    use crate::server::state::AppState;
    use std::collections::HashMap;

    /// Minimal GlobalConfig for unit tests (server modules).
    pub fn test_global_config() -> GlobalConfig {
        GlobalConfig {
            remi: RemiConfig {
                endpoint: "https://localhost".to_string(),
            },
            paths: PathsConfig {
                db: "/tmp/test.db".to_string(),
                conary_bin: "/usr/bin/conary".to_string(),
                results_dir: "/tmp/results".to_string(),
                fixture_dir: None,
            },
            setup: SetupConfig::default(),
            distros: HashMap::new(),
            fixtures: None,
        }
    }

    /// GlobalConfig with a fedora43 distro entry.
    pub fn test_global_config_with_fedora() -> GlobalConfig {
        let mut config = test_global_config();
        config.distros.insert(
            "fedora43".to_string(),
            DistroConfig {
                remi_distro: "fedora43".to_string(),
                repo_name: "conary-fedora43".to_string(),
                containerfile: None,
                test_packages: Vec::new(),
            },
        );
        config
    }

    /// AppState with fedora43 distro and a temp manifest dir.
    pub fn test_app_state() -> AppState {
        AppState::new(
            test_global_config_with_fedora(),
            "/tmp/manifests".to_string(),
        )
    }
}
