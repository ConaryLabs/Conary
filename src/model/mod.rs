// src/model/mod.rs

//! System Model - Declarative OS State Management
//!
//! The system model provides a declarative way to specify the desired state
//! of a system. Instead of running individual install/remove commands, users
//! define a model file that describes what packages should be installed.
//!
//! Conary then computes the diff between current state and desired state,
//! and applies the necessary changes.
//!
//! # Example system.toml
//!
//! ```toml
//! [model]
//! version = 1
//!
//! # Package search path (checked in order)
//! search = [
//!     "fedora@f41:stable",
//!     "conary@extras:stable",
//! ]
//!
//! # Packages to install (and keep installed)
//! install = [
//!     "nginx",
//!     "postgresql",
//!     "redis",
//! ]
//!
//! # Packages to exclude (never install, even as dependencies)
//! exclude = [
//!     "sendmail",
//!     "postfix",
//! ]
//!
//! # Pinned versions (glob patterns supported)
//! [pin]
//! openssl = "3.0.*"
//! kernel = "6.12.*"
//!
//! # Optional packages (install if available, no error if missing)
//! [optional]
//! packages = ["nginx-module-geoip"]
//! ```

mod parser;
mod diff;
mod state;

pub use parser::{SystemModel, ModelConfig, parse_model_file};
pub use diff::{ModelDiff, DiffAction, compute_diff, ApplyOptions};
pub use state::{SystemState, capture_current_state, snapshot_to_model};

use std::path::Path;
use thiserror::Error;

/// Default path for the system model file
pub const DEFAULT_MODEL_PATH: &str = "/etc/conary/system.toml";

/// Errors that can occur when working with system models
#[derive(Debug, Error)]
pub enum ModelError {
    #[error("Failed to read model file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse model file: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Invalid model version: expected {expected}, found {found}")]
    VersionMismatch { expected: u32, found: u32 },

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Invalid search path: {0}")]
    InvalidSearchPath(String),

    #[error("Conflicting package specifications: {0}")]
    ConflictingSpecs(String),

    #[error("Pin pattern invalid: {0}")]
    InvalidPinPattern(String),
}

/// Result type for model operations
pub type ModelResult<T> = Result<T, ModelError>;

/// Load a system model from the default or specified path
pub fn load_model(path: Option<&Path>) -> ModelResult<SystemModel> {
    let path = path.unwrap_or_else(|| Path::new(DEFAULT_MODEL_PATH));
    parse_model_file(path)
}

/// Check if a system model file exists
pub fn model_exists(path: Option<&Path>) -> bool {
    let path = path.unwrap_or_else(|| Path::new(DEFAULT_MODEL_PATH));
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_parse_minimal_model() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"
[model]
version = 1
install = ["nginx"]
"#).unwrap();

        let model = parse_model_file(file.path()).unwrap();
        assert_eq!(model.config.version, 1);
        assert_eq!(model.config.install, vec!["nginx"]);
    }

    #[test]
    fn test_parse_full_model() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"
[model]
version = 1
search = ["fedora@f41:stable", "extras@local:dev"]
install = ["nginx", "postgresql", "redis"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"
kernel = "6.12.*"

[optional]
packages = ["nginx-module-geoip"]
"#).unwrap();

        let model = parse_model_file(file.path()).unwrap();
        assert_eq!(model.config.version, 1);
        assert_eq!(model.config.search.len(), 2);
        assert_eq!(model.config.install.len(), 3);
        assert_eq!(model.config.exclude.len(), 1);
        assert_eq!(model.pin.get("openssl"), Some(&"3.0.*".to_string()));
        assert_eq!(model.optional.packages.len(), 1);
    }
}
