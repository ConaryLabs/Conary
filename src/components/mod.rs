// src/components/mod.rs

//! Component model for Conary packages
//!
//! This module implements first-class components - independently installable
//! units within packages. Components are classified by file paths and have
//! their own dependency relationships.
//!
//! # Component Types
//!
//! | Component | Description | Default? |
//! |-----------|-------------|----------|
//! | `:runtime` | Executables, assets, helpers | Yes |
//! | `:lib` | Shared libraries | Yes |
//! | `:config` | Configuration files | Yes |
//! | `:devel` | Headers, static libs, pkg-config | No |
//! | `:doc` | Documentation, man pages | No |
//! | `:debuginfo` | Debug symbols | No |
//! | `:test` | Test suites | No |
//!
//! # Usage
//!
//! ```ignore
//! use conary::components::{ComponentType, ComponentClassifier};
//!
//! // Classify a single file
//! let comp = ComponentClassifier::classify(Path::new("/usr/lib/libssl.so.3"));
//! assert_eq!(comp, ComponentType::Lib);
//!
//! // Check if component is installed by default
//! assert!(ComponentType::Runtime.is_default());
//! assert!(!ComponentType::Doc.is_default());
//! ```
//!
//! # External Filters
//!
//! Custom classification rules can be loaded from configuration files:
//!
//! ```ignore
//! use conary::components::{FilterSet, FilteredClassifier};
//!
//! let filters = FilterSet::load_from_file(Path::new("/etc/conary/filters.d/custom.conf"))?;
//! let classifier = FilteredClassifier::new(filters);
//! let comp = classifier.classify("/opt/myapp/bin/foo");
//! ```

mod classifier;
mod filters;

pub use classifier::{ComponentClassifier, ComponentType};
pub use filters::{FilterRule, FilterSet, FilteredClassifier};

/// Parse a component spec string like "package:component"
///
/// Returns `Some((package_name, component_name))` if valid, `None` otherwise.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(parse_component_spec("nginx:lib"), Some(("nginx".to_string(), "lib".to_string())));
/// assert_eq!(parse_component_spec("nginx"), None);
/// assert_eq!(parse_component_spec(":lib"), None);
/// ```
pub fn parse_component_spec(spec: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = spec.splitn(2, ':').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

/// Format a component spec from package and component names
///
/// # Examples
///
/// ```ignore
/// assert_eq!(format_component_spec("nginx", "lib"), "nginx:lib");
/// ```
pub fn format_component_spec(package: &str, component: &str) -> String {
    format!("{}:{}", package, component)
}

/// Check if scriptlets should run based on installed components
///
/// Scriptlets only run when `:runtime` or `:lib` is being installed.
/// This prevents crashes when scripts try to run binaries that aren't present.
pub fn should_run_scriptlets(installed_components: &[ComponentType]) -> bool {
    installed_components
        .iter()
        .any(|c| matches!(c, ComponentType::Runtime | ComponentType::Lib))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_component_spec_valid() {
        assert_eq!(
            parse_component_spec("nginx:lib"),
            Some(("nginx".to_string(), "lib".to_string()))
        );
        assert_eq!(
            parse_component_spec("openssl:devel"),
            Some(("openssl".to_string(), "devel".to_string()))
        );
    }

    #[test]
    fn test_parse_component_spec_invalid() {
        assert_eq!(parse_component_spec("nginx"), None);
        assert_eq!(parse_component_spec(":lib"), None);
        assert_eq!(parse_component_spec("nginx:"), None);
        assert_eq!(parse_component_spec(":"), None);
        assert_eq!(parse_component_spec(""), None);
    }

    #[test]
    fn test_format_component_spec() {
        assert_eq!(format_component_spec("nginx", "lib"), "nginx:lib");
        assert_eq!(format_component_spec("openssl", "devel"), "openssl:devel");
    }

    #[test]
    fn test_should_run_scriptlets_with_runtime() {
        assert!(should_run_scriptlets(&[ComponentType::Runtime]));
        assert!(should_run_scriptlets(&[ComponentType::Runtime, ComponentType::Doc]));
    }

    #[test]
    fn test_should_run_scriptlets_with_lib() {
        assert!(should_run_scriptlets(&[ComponentType::Lib]));
        assert!(should_run_scriptlets(&[ComponentType::Lib, ComponentType::Devel]));
    }

    #[test]
    fn test_should_run_scriptlets_without_runtime_or_lib() {
        assert!(!should_run_scriptlets(&[ComponentType::Devel]));
        assert!(!should_run_scriptlets(&[ComponentType::Doc]));
        assert!(!should_run_scriptlets(&[ComponentType::Config]));
        assert!(!should_run_scriptlets(&[ComponentType::Devel, ComponentType::Doc]));
        assert!(!should_run_scriptlets(&[]));
    }
}
