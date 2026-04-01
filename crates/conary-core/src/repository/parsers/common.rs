// conary-core/src/repository/parsers/common.rs

//! Shared parser helpers for repository metadata parsers.
//!
//! Eliminates duplication across the Arch, Debian, and Fedora parsers for
//! common operations like version constraint extraction, dependency string
//! splitting, and download URL construction.

/// Maximum allowed package size (5 GB).
///
/// Shared across all parsers to reject unreasonably large packages.
pub const MAX_PACKAGE_SIZE: u64 = 5 * 1024 * 1024 * 1024;

/// Recognized version constraint operators, ordered longest-first so that
/// `>=` is matched before `>`.
const CONSTRAINT_OPS: &[&str] = &[">=", "<=", ">>", "<<", "=", ">", "<"];

/// Extract the bare version string from a constraint like `"= 1.0"`,
/// `">=3.2"`, or `">> 1.5"`.
///
/// Strips the leading operator and any surrounding whitespace. Returns
/// `None` if the input is empty or contains only an operator with no
/// version.
///
/// Used by all three parsers when converting native provide constraints
/// into structured `RepositoryProvide` version fields.
pub fn extract_version_from_constraint(constraint: &str) -> Option<String> {
    let trimmed = constraint.trim();
    if trimmed.is_empty() {
        return None;
    }

    let ver_part = strip_constraint_op(trimmed);
    let ver = ver_part.trim();
    if ver.is_empty() {
        None
    } else {
        Some(ver.to_string())
    }
}

/// Strip a leading constraint operator from a version string.
///
/// Returns the remaining string after the operator (if any). If no
/// recognized operator is found, returns the input unchanged.
fn strip_constraint_op(s: &str) -> &str {
    for op in CONSTRAINT_OPS {
        if let Some(rest) = s.strip_prefix(op) {
            return rest;
        }
    }
    s
}

/// Split a dependency string into (name, constraint).
///
/// Handles two common formats:
/// - **Parenthesized** (Debian style): `"libc6 (>= 2.34)"` -> `("libc6", ">= 2.34")`
/// - **Inline operator** (Arch/RPM style): `"glibc>=2.17"` -> `("glibc", ">=2.17")`
/// - **Name only**: `"bash"` -> `("bash", "")`
///
/// The returned constraint string includes the operator (e.g. `">= 2.34"`).
pub fn split_dependency(dep: &str) -> (String, String) {
    // Parenthesized form: "name (OP version)"
    if let Some(paren_pos) = dep.find('(') {
        let name = dep[..paren_pos].trim().to_string();
        let constraint = dep[paren_pos + 1..]
            .trim_end_matches(')')
            .trim()
            .to_string();
        return (name, constraint);
    }

    // Inline operator form: "name>=version"
    for op in CONSTRAINT_OPS {
        if let Some(pos) = dep.find(op) {
            let name = dep[..pos].to_string();
            let version = dep[pos..].to_string();
            return (name, version);
        }
    }

    // Name only
    (dep.trim().to_string(), String::new())
}

/// Construct a metadata URL by joining a base URL with a relative path.
///
/// Normalizes trailing slashes on the base URL.
pub fn join_repo_url(base_url: &str, path: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), path)
}

/// Validate that a filename is safe (no path traversal, not absolute, no
/// URL schemes).
///
/// Returns `Ok(())` if safe, or an error description if suspicious.
pub fn validate_filename(filename: &str) -> Result<(), String> {
    if filename.contains("..") {
        return Err(format!(
            "Suspicious filename (path traversal): {}",
            filename
        ));
    }
    if filename.starts_with('/') || filename.contains("://") {
        return Err(format!(
            "Suspicious filename (not relative path): {}",
            filename
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version_from_constraint_equals() {
        assert_eq!(
            extract_version_from_constraint("= 1.0"),
            Some("1.0".to_string())
        );
    }

    #[test]
    fn test_extract_version_from_constraint_ge() {
        assert_eq!(
            extract_version_from_constraint(">=3.2"),
            Some("3.2".to_string())
        );
    }

    #[test]
    fn test_extract_version_from_constraint_le() {
        assert_eq!(
            extract_version_from_constraint("<= 2.0-1"),
            Some("2.0-1".to_string())
        );
    }

    #[test]
    fn test_extract_version_from_constraint_deb_gt() {
        assert_eq!(
            extract_version_from_constraint(">> 1.5"),
            Some("1.5".to_string())
        );
    }

    #[test]
    fn test_extract_version_from_constraint_bare_version() {
        // No operator -- returns as-is
        assert_eq!(
            extract_version_from_constraint("3.0"),
            Some("3.0".to_string())
        );
    }

    #[test]
    fn test_extract_version_from_constraint_empty() {
        assert_eq!(extract_version_from_constraint(""), None);
        assert_eq!(extract_version_from_constraint("  "), None);
    }

    #[test]
    fn test_split_dependency_parenthesized() {
        let (name, constraint) = split_dependency("libc6 (>= 2.34)");
        assert_eq!(name, "libc6");
        assert_eq!(constraint, ">= 2.34");
    }

    #[test]
    fn test_split_dependency_inline_operator() {
        let (name, constraint) = split_dependency("glibc>=2.17");
        assert_eq!(name, "glibc");
        assert_eq!(constraint, ">=2.17");
    }

    #[test]
    fn test_split_dependency_equals() {
        let (name, constraint) = split_dependency("zlib=1.3.1-1");
        assert_eq!(name, "zlib");
        assert_eq!(constraint, "=1.3.1-1");
    }

    #[test]
    fn test_split_dependency_name_only() {
        let (name, constraint) = split_dependency("bash");
        assert_eq!(name, "bash");
        assert_eq!(constraint, "");
    }

    #[test]
    fn test_split_dependency_deb_versioned() {
        let (name, constraint) = split_dependency("bash (= 5.2-1)");
        assert_eq!(name, "bash");
        assert_eq!(constraint, "= 5.2-1");
    }

    #[test]
    fn test_join_repo_url() {
        assert_eq!(
            join_repo_url("https://repo.example.com/", "repodata/repomd.xml"),
            "https://repo.example.com/repodata/repomd.xml"
        );
        assert_eq!(
            join_repo_url("https://repo.example.com", "Packages/a/app.rpm"),
            "https://repo.example.com/Packages/a/app.rpm"
        );
    }

    #[test]
    fn test_validate_filename_safe() {
        assert!(validate_filename("Packages/a/app.rpm").is_ok());
        assert!(validate_filename("pool/main/g/glibc.deb").is_ok());
    }

    #[test]
    fn test_validate_filename_traversal() {
        assert!(validate_filename("../../../etc/passwd").is_err());
    }

    #[test]
    fn test_validate_filename_absolute() {
        assert!(validate_filename("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_filename_url_scheme() {
        assert!(validate_filename("https://evil.com/malware.rpm").is_err());
    }
}
