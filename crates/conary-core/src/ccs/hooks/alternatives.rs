// conary-core/src/ccs/hooks/alternatives.rs

//! Alternatives integration for CCS hooks
//!
//! Handles update-alternatives for managing multiple versions
//! of programs that provide the same functionality.

use super::HookExecutor;
use anyhow::Result;
use tracing::info;

/// Validate alternative name - only allow `[a-zA-Z0-9_-]` characters.
fn validate_alternative_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow::anyhow!("Alternative name cannot be empty"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow::anyhow!(
            "Alternative name contains invalid characters: {}",
            name
        ));
    }
    Ok(())
}

/// Validate path is absolute and contains no `..` components.
fn validate_alternative_path(path: &str) -> Result<()> {
    if !path.starts_with('/') {
        return Err(anyhow::anyhow!(
            "Alternative path must be absolute: {}",
            path
        ));
    }
    if path.split('/').any(|component| component == "..") {
        return Err(anyhow::anyhow!(
            "Alternative path contains traversal: {}",
            path
        ));
    }
    Ok(())
}

impl HookExecutor {
    pub(super) fn preflight_alternative(&self, link: &str, name: &str, path: &str) -> Result<()> {
        validate_alternative_path(link)?;
        validate_alternative_name(name)?;
        validate_alternative_path(path)?;
        crate::scriptlet::ScriptletExecutor::new(
            &self.root,
            "ccs-alternatives",
            "signed-v2",
            crate::scriptlet::PackageFormat::Conary,
        )
        .preflight_native_command(&["/usr/bin/update-alternatives".to_string()])?;
        Ok(())
    }

    /// Update alternatives
    ///
    /// Apply the target-owned alternatives grammar to the selected root.
    pub(super) fn update_alternatives(
        &self,
        link: &str,
        name: &str,
        path: &str,
        priority: i32,
    ) -> Result<()> {
        self.preflight_alternative(link, name, path)?;

        let argv = vec![
            "/usr/bin/update-alternatives".to_string(),
            "--install".to_string(),
            link.to_string(),
            name.to_string(),
            path.to_string(),
            priority.to_string(),
        ];
        crate::scriptlet::ScriptletExecutor::new(
            &self.root,
            "ccs-alternatives",
            "signed-v2",
            crate::scriptlet::PackageFormat::Conary,
        )
        .execute_native_command("alternatives", &argv, &[])?;
        info!(
            "Updated alternative '{}' -> '{}' (priority {}) in selected root {}",
            name,
            path,
            priority,
            self.root.display()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_alternative_names() {
        assert!(validate_alternative_name("gcc").is_ok());
        assert!(validate_alternative_name("g-plus-plus").is_ok());
        assert!(validate_alternative_name("python3_11").is_ok());
    }

    #[test]
    fn test_invalid_alternative_names() {
        assert!(validate_alternative_name("").is_err());
        assert!(validate_alternative_name("name;rm -rf /").is_err());
        assert!(validate_alternative_name("../etc/passwd").is_err());
        assert!(validate_alternative_name("name with spaces").is_err());
        assert!(validate_alternative_name("name.with.dots").is_err());
    }

    #[test]
    fn test_valid_alternative_paths() {
        assert!(validate_alternative_path("/usr/bin/gcc-12").is_ok());
        assert!(validate_alternative_path("/usr/local/bin/python3").is_ok());
    }

    #[test]
    fn test_invalid_alternative_paths() {
        assert!(validate_alternative_path("relative/path").is_err());
        assert!(validate_alternative_path("/usr/../etc/passwd").is_err());
        assert!(validate_alternative_path("").is_err());
    }
}
