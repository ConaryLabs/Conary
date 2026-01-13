// src/dependencies/detection.rs

//! Language-specific dependency detection
//!
//! This module provides automatic detection of language-specific dependencies
//! by analyzing file paths and package contents.

use super::{DependencyClass, LanguageDep};
use std::path::Path;

/// Detector for language-specific dependencies
///
/// Analyzes file paths to determine what language-specific dependencies
/// a package provides and requires.
pub struct LanguageDepDetector;

impl LanguageDepDetector {
    /// Detect what language-specific capabilities a file provides
    ///
    /// Returns a list of dependencies that this file provides
    /// (what others can depend on).
    pub fn detect_provides(path: &str) -> Vec<LanguageDep> {
        let mut provides = Vec::new();

        // Python modules
        if let Some(module) = Self::detect_python_module(path) {
            provides.push(LanguageDep::new(DependencyClass::Python, module));
        }

        // Perl modules
        if let Some(module) = Self::detect_perl_module(path) {
            provides.push(LanguageDep::new(DependencyClass::Perl, module));
        }

        // Ruby gems
        if let Some(gem) = Self::detect_ruby_gem(path) {
            provides.push(LanguageDep::new(DependencyClass::Ruby, gem));
        }

        // Java packages (from class files or jars)
        if let Some(pkg) = Self::detect_java_package(path) {
            provides.push(LanguageDep::new(DependencyClass::Java, pkg));
        }

        // Shared libraries (soname)
        if let Some(soname) = Self::detect_soname(path) {
            provides.push(LanguageDep::new(DependencyClass::Soname, soname));
        }

        // File provides (everything provides itself)
        if path.starts_with('/') {
            provides.push(LanguageDep::new(DependencyClass::File, path));
        }

        provides
    }

    /// Detect Python module from file path
    ///
    /// Python modules are found in:
    /// - /usr/lib/pythonX.Y/site-packages/module.py
    /// - /usr/lib/pythonX.Y/site-packages/module/__init__.py
    /// - /usr/lib64/pythonX.Y/site-packages/...
    fn detect_python_module(path: &str) -> Option<String> {
        // Check if this is a Python site-packages path
        let site_packages_patterns = [
            "site-packages/",
            "dist-packages/", // Debian-style
        ];

        for pattern in &site_packages_patterns {
            if let Some(pos) = path.find(pattern) {
                let after_pattern = &path[pos + pattern.len()..];
                return Self::extract_python_module_name(after_pattern);
            }
        }

        None
    }

    /// Extract Python module name from path after site-packages/
    fn extract_python_module_name(path: &str) -> Option<String> {
        let path = Path::new(path);

        // Get the first component as the top-level module
        let mut components = path.components();
        let first = components.next()?;
        let first_str = first.as_os_str().to_str()?;

        // Skip common non-module directories
        if first_str.starts_with('_')
            || first_str == "bin"
            || first_str.ends_with(".dist-info")
            || first_str.ends_with(".egg-info")
        {
            return None;
        }

        // If it's a .py file, use the stem
        if first_str.ends_with(".py") {
            let stem = first_str.strip_suffix(".py")?;
            if stem != "__init__" && stem != "__main__" && !stem.starts_with('_') {
                return Some(stem.to_string());
            }
            return None;
        }

        // If it's a .so file (compiled module)
        if first_str.contains(".cpython-") || first_str.ends_with(".so") {
            // Extract module name from e.g., "module.cpython-311-x86_64-linux-gnu.so"
            let module_name = first_str.split('.').next()?;
            if !module_name.starts_with('_') {
                return Some(module_name.to_string());
            }
            return None;
        }

        // Otherwise, it's a package directory
        if !first_str.contains('.') && !first_str.starts_with('_') {
            return Some(first_str.to_string());
        }

        None
    }

    /// Detect Perl module from file path
    ///
    /// Perl modules are found in:
    /// - /usr/share/perl5/Module/Name.pm -> perl(Module::Name)
    /// - /usr/lib/perl5/auto/...
    /// - /usr/lib64/perl5/...
    fn detect_perl_module(path: &str) -> Option<String> {
        // Check for Perl module paths
        if !path.contains("/perl") {
            return None;
        }

        // Only .pm files are modules
        if !path.ends_with(".pm") {
            return None;
        }

        // Find the module path part
        // Common patterns: /perl5/, /perl5/vendor_perl/, /perl5/site_perl/
        let perl_patterns = [
            "/perl5/vendor_perl/",
            "/perl5/site_perl/",
            "/perl5/",
            "/share/perl5/",
            "/share/perl/",
        ];

        for pattern in &perl_patterns {
            if let Some(pos) = path.find(pattern) {
                let module_path = &path[pos + pattern.len()..];
                return Self::path_to_perl_module(module_path);
            }
        }

        None
    }

    /// Convert a Perl file path to module name
    fn path_to_perl_module(path: &str) -> Option<String> {
        // Remove .pm extension and convert slashes to ::
        let module_path = path.strip_suffix(".pm")?;
        let module_name = module_path.replace('/', "::");
        if !module_name.is_empty() {
            Some(module_name)
        } else {
            None
        }
    }

    /// Detect Ruby gem from file path
    ///
    /// Ruby gems are found in:
    /// - /usr/share/gems/gems/gemname-version/...
    /// - /usr/lib/ruby/gems/X.Y.Z/gems/gemname-version/...
    fn detect_ruby_gem(path: &str) -> Option<String> {
        if !path.contains("/ruby") && !path.contains("/gems/") {
            return None;
        }

        // Look for the gems directory
        let gems_patterns = ["/gems/gems/", "/share/gems/gems/"];

        for pattern in &gems_patterns {
            if let Some(pos) = path.find(pattern) {
                let after_gems = &path[pos + pattern.len()..];
                // Extract gem name (everything before the version)
                if let Some(first_slash) = after_gems.find('/') {
                    let gem_with_version = &after_gems[..first_slash];
                    // Split on last hyphen to get gem name
                    if let Some(last_hyphen) = gem_with_version.rfind('-') {
                        let gem_name = &gem_with_version[..last_hyphen];
                        if !gem_name.is_empty() {
                            return Some(gem_name.to_string());
                        }
                    }
                }
            }
        }

        None
    }

    /// Detect Java package from file path
    ///
    /// Java packages are found in:
    /// - /usr/share/java/package.jar
    /// - Compiled .class files
    fn detect_java_package(path: &str) -> Option<String> {
        // JAR files
        if path.ends_with(".jar") && path.contains("/java/") {
            let file_name = Path::new(path).file_stem()?.to_str()?;
            // Remove version suffix if present
            let name = Self::strip_java_version(file_name);
            return Some(name.to_string());
        }

        // .class files (would need to parse to get actual package name)
        // For now, we just detect them but don't parse the package
        if path.ends_with(".class") {
            // Would need bytecode parsing for accurate package detection
            return None;
        }

        None
    }

    /// Strip version suffix from Java package name
    fn strip_java_version(name: &str) -> &str {
        // Common patterns: package-1.0.0, package_1.0.0
        // Find the first digit after a separator
        for (i, c) in name.char_indices() {
            if (c == '-' || c == '_')
                && name.len() > i + 1
                && let Some(next_char) = name.chars().nth(i + 1)
                && next_char.is_ascii_digit()
            {
                return &name[..i];
            }
        }
        name
    }

    /// Detect shared library soname from file path
    fn detect_soname(path: &str) -> Option<String> {
        // Must be in a lib directory and end with .so or .so.X
        if !path.contains("/lib") {
            return None;
        }

        let filename = Path::new(path).file_name()?.to_str()?;

        // Check for .so files
        if !filename.contains(".so") {
            return None;
        }

        // Return the soname (the filename is usually the soname or close to it)
        Some(filename.to_string())
    }

    /// Detect all language-specific capabilities from a list of file paths
    pub fn detect_all_provides(paths: &[String]) -> Vec<LanguageDep> {
        let mut all_provides = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for path in paths {
            for dep in Self::detect_provides(path) {
                // Deduplicate by dep string
                let key = dep.to_dep_string();
                if seen.insert(key) {
                    all_provides.push(dep);
                }
            }
        }

        all_provides
    }

    /// Analyze file paths and return capabilities grouped by class
    pub fn analyze_provides(
        paths: &[String],
    ) -> std::collections::HashMap<DependencyClass, Vec<String>> {
        let mut result = std::collections::HashMap::new();

        for path in paths {
            for dep in Self::detect_provides(path) {
                result
                    .entry(dep.class)
                    .or_insert_with(Vec::new)
                    .push(dep.name);
            }
        }

        // Deduplicate within each class
        for names in result.values_mut() {
            names.sort();
            names.dedup();
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================
    // Python detection
    // ====================

    #[test]
    fn test_detect_python_module_simple() {
        let provides =
            LanguageDepDetector::detect_provides("/usr/lib/python3.11/site-packages/requests.py");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Python && d.name == "requests"));
    }

    #[test]
    fn test_detect_python_package() {
        let provides = LanguageDepDetector::detect_provides(
            "/usr/lib/python3.11/site-packages/flask/__init__.py",
        );
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Python && d.name == "flask"));
    }

    #[test]
    fn test_detect_python_compiled() {
        let provides = LanguageDepDetector::detect_provides(
            "/usr/lib64/python3.11/site-packages/numpy.cpython-311-x86_64-linux-gnu.so",
        );
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Python && d.name == "numpy"));
    }

    #[test]
    fn test_detect_python_dist_packages() {
        // Debian-style
        let provides = LanguageDepDetector::detect_provides(
            "/usr/lib/python3/dist-packages/apt_pkg.cpython-311-x86_64-linux-gnu.so",
        );
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Python && d.name == "apt_pkg"));
    }

    #[test]
    fn test_detect_python_skip_private() {
        let provides = LanguageDepDetector::detect_provides(
            "/usr/lib/python3.11/site-packages/_internal/__init__.py",
        );
        assert!(!provides
            .iter()
            .any(|d| d.class == DependencyClass::Python && d.name == "_internal"));
    }

    #[test]
    fn test_detect_python_skip_dist_info() {
        let provides = LanguageDepDetector::detect_provides(
            "/usr/lib/python3.11/site-packages/requests-2.28.0.dist-info/METADATA",
        );
        assert!(!provides
            .iter()
            .any(|d| d.class == DependencyClass::Python && d.name.contains("dist-info")));
    }

    // ====================
    // Perl detection
    // ====================

    #[test]
    fn test_detect_perl_module() {
        let provides = LanguageDepDetector::detect_provides("/usr/share/perl5/DBI.pm");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Perl && d.name == "DBI"));
    }

    #[test]
    fn test_detect_perl_nested_module() {
        let provides =
            LanguageDepDetector::detect_provides("/usr/share/perl5/vendor_perl/XML/Parser.pm");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Perl && d.name == "XML::Parser"));
    }

    #[test]
    fn test_detect_perl_skip_non_pm() {
        let provides =
            LanguageDepDetector::detect_provides("/usr/share/perl5/vendor_perl/auto/DBI/DBI.so");
        assert!(!provides.iter().any(|d| d.class == DependencyClass::Perl));
    }

    // ====================
    // Ruby detection
    // ====================

    #[test]
    fn test_detect_ruby_gem() {
        let provides =
            LanguageDepDetector::detect_provides("/usr/share/gems/gems/bundler-2.4.0/lib/bundler.rb");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Ruby && d.name == "bundler"));
    }

    // ====================
    // Java detection
    // ====================

    #[test]
    fn test_detect_java_jar() {
        let provides =
            LanguageDepDetector::detect_provides("/usr/share/java/commons-lang3-3.12.0.jar");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Java && d.name == "commons-lang3"));
    }

    #[test]
    fn test_detect_java_jar_no_version() {
        let provides = LanguageDepDetector::detect_provides("/usr/share/java/junit.jar");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Java && d.name == "junit"));
    }

    // ====================
    // Soname detection
    // ====================

    #[test]
    fn test_detect_soname() {
        let provides = LanguageDepDetector::detect_provides("/usr/lib64/libssl.so.3");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Soname && d.name == "libssl.so.3"));
    }

    #[test]
    fn test_detect_soname_multiarch() {
        let provides =
            LanguageDepDetector::detect_provides("/usr/lib/x86_64-linux-gnu/libz.so.1.2.13");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Soname && d.name == "libz.so.1.2.13"));
    }

    // ====================
    // File detection
    // ====================

    #[test]
    fn test_detect_file_provides() {
        let provides = LanguageDepDetector::detect_provides("/usr/bin/python3");
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::File && d.name == "/usr/bin/python3"));
    }

    // ====================
    // Batch detection
    // ====================

    #[test]
    fn test_detect_all_provides() {
        let paths = vec![
            "/usr/lib/python3.11/site-packages/requests.py".to_string(),
            "/usr/share/perl5/DBI.pm".to_string(),
            "/usr/lib64/libssl.so.3".to_string(),
        ];

        let provides = LanguageDepDetector::detect_all_provides(&paths);

        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Python));
        assert!(provides.iter().any(|d| d.class == DependencyClass::Perl));
        assert!(provides
            .iter()
            .any(|d| d.class == DependencyClass::Soname));
    }

    #[test]
    fn test_analyze_provides() {
        let paths = vec![
            "/usr/lib/python3.11/site-packages/requests.py".to_string(),
            "/usr/lib/python3.11/site-packages/urllib3.py".to_string(),
            "/usr/share/perl5/DBI.pm".to_string(),
        ];

        let analysis = LanguageDepDetector::analyze_provides(&paths);

        assert_eq!(analysis.get(&DependencyClass::Python).map(|v| v.len()), Some(2));
        assert_eq!(analysis.get(&DependencyClass::Perl).map(|v| v.len()), Some(1));
    }
}
