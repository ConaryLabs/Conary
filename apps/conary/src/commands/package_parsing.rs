// src/commands/package_parsing.rs

//! Shared package spec parsing utilities.
//!
//! Provides a canonical `parse_package_spec` that splits a user-provided
//! package specifier (e.g. `"nginx@1.24.2"`) into a name and optional
//! version. Previously duplicated in individual command modules.

/// Parse a `name@version` package specifier.
///
/// Uses `rfind` so that package names containing `@` (unlikely but
/// possible) still parse correctly -- only the *last* `@` is treated
/// as the separator.
///
/// # Examples
///
/// ```ignore
/// let (name, ver) = parse_package_spec("nginx@1.24.2");
/// assert_eq!(name, "nginx");
/// assert_eq!(ver, Some("1.24.2".to_string()));
///
/// let (name, ver) = parse_package_spec("nginx");
/// assert_eq!(name, "nginx");
/// assert_eq!(ver, None);
/// ```
pub(crate) fn parse_package_spec(spec: &str) -> (String, Option<String>) {
    if let Some(idx) = spec.rfind('@') {
        let name = spec[..idx].to_string();
        let version = spec[idx + 1..].to_string();
        (name, Some(version))
    } else {
        (spec.to_string(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_name_only() {
        let (name, version) = parse_package_spec("nginx");
        assert_eq!(name, "nginx");
        assert!(version.is_none());
    }

    #[test]
    fn test_parse_name_and_version() {
        let (name, version) = parse_package_spec("nginx@1.24.2");
        assert_eq!(name, "nginx");
        assert_eq!(version.unwrap(), "1.24.2");
    }

    #[test]
    fn test_parse_scoped_name_with_version() {
        let (name, version) = parse_package_spec("scope@pkg@2.0");
        assert_eq!(name, "scope@pkg");
        assert_eq!(version.unwrap(), "2.0");
    }

    #[test]
    fn test_parse_empty_string() {
        let (name, version) = parse_package_spec("");
        assert_eq!(name, "");
        assert!(version.is_none());
    }
}
