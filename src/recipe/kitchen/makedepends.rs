// src/recipe/kitchen/makedepends.rs

//! Makedepends resolution for recipe builds

use crate::error::Result;

/// Trait for resolving and installing makedepends before building
///
/// This allows the Kitchen to remain decoupled from the package installation
/// logic while still being able to ensure build dependencies are available.
pub trait MakedependsResolver: Send + Sync {
    /// Check which makedepends are missing
    ///
    /// Returns a list of package names that are not currently installed.
    fn check_missing(&self, deps: &[&str]) -> Result<Vec<String>>;

    /// Install the specified makedepends
    ///
    /// Should install the packages and return the list of packages that
    /// were actually installed (for later cleanup).
    fn install(&self, deps: &[String]) -> Result<Vec<String>>;

    /// Uninstall packages that were installed as makedepends
    ///
    /// Called after build completes to clean up temporary dependencies.
    /// Only removes packages that were installed by this build.
    fn cleanup(&self, installed: &[String]) -> Result<()>;
}

/// A no-op resolver that assumes all dependencies are satisfied
///
/// Use this when you want to skip makedepends resolution entirely
/// (e.g., in a pre-configured build container).
pub struct NoopResolver;

impl MakedependsResolver for NoopResolver {
    fn check_missing(&self, _deps: &[&str]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn install(&self, _deps: &[String]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn cleanup(&self, _installed: &[String]) -> Result<()> {
        Ok(())
    }
}

/// Result of makedepends resolution
#[derive(Debug, Default, Clone)]
pub struct MakedependsResult {
    /// Packages that were already installed
    pub already_installed: Vec<String>,
    /// Packages that were installed for this build
    pub newly_installed: Vec<String>,
    /// Packages that could not be resolved
    pub unresolved: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// A mock resolver for testing makedepends resolution
    pub struct MockResolver {
        installed: Mutex<HashSet<String>>,
        install_calls: Mutex<Vec<Vec<String>>>,
        cleanup_calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockResolver {
        pub fn new(initially_installed: &[&str]) -> Self {
            Self {
                installed: Mutex::new(
                    initially_installed.iter().map(|s| s.to_string()).collect(),
                ),
                install_calls: Mutex::new(Vec::new()),
                cleanup_calls: Mutex::new(Vec::new()),
            }
        }

    }

    impl MakedependsResolver for MockResolver {
        fn check_missing(&self, deps: &[&str]) -> Result<Vec<String>> {
            let installed = self.installed.lock().unwrap();
            Ok(deps
                .iter()
                .filter(|d| !installed.contains(&d.to_string()))
                .map(|s| s.to_string())
                .collect())
        }

        fn install(&self, deps: &[String]) -> Result<Vec<String>> {
            self.install_calls.lock().unwrap().push(deps.to_vec());
            let mut installed = self.installed.lock().unwrap();
            for dep in deps {
                installed.insert(dep.clone());
            }
            Ok(deps.to_vec())
        }

        fn cleanup(&self, deps: &[String]) -> Result<()> {
            self.cleanup_calls.lock().unwrap().push(deps.to_vec());
            let mut installed = self.installed.lock().unwrap();
            for dep in deps {
                installed.remove(dep);
            }
            Ok(())
        }
    }

    #[test]
    fn test_noop_resolver() {
        let resolver = NoopResolver;
        assert!(resolver.check_missing(&["foo", "bar"]).unwrap().is_empty());
        assert!(resolver.install(&["foo".to_string()]).unwrap().is_empty());
        assert!(resolver.cleanup(&["foo".to_string()]).is_ok());
    }

    #[test]
    fn test_mock_resolver_missing() {
        let resolver = MockResolver::new(&["installed"]);
        let missing = resolver.check_missing(&["installed", "missing"]).unwrap();
        assert_eq!(missing, vec!["missing"]);
    }

    #[test]
    fn test_mock_resolver_install() {
        let resolver = MockResolver::new(&[]);
        let installed = resolver.install(&["pkg1".to_string(), "pkg2".to_string()]).unwrap();
        assert_eq!(installed.len(), 2);

        // Now they should not be missing
        let missing = resolver.check_missing(&["pkg1", "pkg2"]).unwrap();
        assert!(missing.is_empty());
    }
}
