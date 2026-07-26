// conary-core/src/components/mod.rs

//! Component model for Conary packages
//!
//! This module implements first-class components - independently installable
//! units declared by package metadata with their own dependency relationships.
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
//! use conary_core::components::ComponentType;
//!
//! // Parse an exact component name declared by package metadata.
//! let comp = ComponentType::parse("lib");
//! assert_eq!(comp, Some(ComponentType::Lib));
//! assert!(ComponentType::Runtime.is_default());
//! assert!(!ComponentType::Doc.is_default());
//! ```
//!
mod types;

pub use types::ComponentType;

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
    let (package, component) = spec.split_once(':')?;
    if package.is_empty() || component.is_empty() {
        return None;
    }
    Some((package.to_string(), component.to_string()))
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
}
