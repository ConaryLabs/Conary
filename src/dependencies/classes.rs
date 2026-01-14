// src/dependencies/classes.rs

//! Dependency class definitions
//!
//! Based on OG Conary's 18 dependency classes, this module provides
//! a type-safe way to represent language-specific dependencies.

use std::fmt;

/// Dependency classes represent different ecosystems/languages
///
/// These correspond to OG Conary's dependency classes but focused on
/// the most commonly used ones. Each class has a specific syntax for
/// expressing dependencies within that ecosystem.
///
/// Inspired by Aeryn OS typed dependencies, this provides explicit
/// type prefixes for all dependency kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyClass {
    /// System/package-level dependency (default)
    /// Format: package-name
    Package,

    /// Shared library dependency
    /// Format: soname(version)
    /// Example: libssl.so.3(OPENSSL_3.0.0)
    Soname,

    /// Python module dependency
    /// Format: python(module-name)
    /// Example: python(requests>=2.0)
    Python,

    /// Perl module dependency
    /// Format: perl(Module::Name)
    /// Example: perl(DBI>=1.600)
    Perl,

    /// Ruby gem dependency
    /// Format: ruby(gem-name)
    /// Example: ruby(bundler>=2.0)
    Ruby,

    /// Java package dependency
    /// Format: java(package.name)
    /// Example: java(org.apache.commons.lang)
    Java,

    /// .NET/Mono CIL dependency
    /// Format: cil(assembly-name)
    /// Example: cil(System.Core)
    Cil,

    /// File-based dependency (specific file must exist)
    /// Format: file(/path/to/file)
    /// Example: file(/usr/bin/python3)
    File,

    /// ELF interpreter dependency
    /// Format: interpreter(/path)
    /// Example: interpreter(/lib64/ld-linux-x86-64.so.2)
    Interpreter,

    /// ABI compatibility tag
    /// Format: abi(name)
    /// Example: abi(x86_64-linux-gnu)
    Abi,

    /// pkg-config dependency
    /// Format: pkgconfig(name)
    /// Example: pkgconfig(zlib>=1.2)
    PkgConfig,

    /// CMake package dependency
    /// Format: cmake(name)
    /// Example: cmake(Qt5Core>=5.15)
    CMake,

    /// Binary/executable dependency
    /// Format: binary(name)
    /// Example: binary(python3)
    Binary,

    /// Kernel module dependency
    /// Format: kmod(name)
    /// Example: kmod(nvidia)
    KernelModule,
}

impl DependencyClass {
    /// Get the string prefix for this dependency class
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Package => "",
            Self::Soname => "soname",
            Self::Python => "python",
            Self::Perl => "perl",
            Self::Ruby => "ruby",
            Self::Java => "java",
            Self::Cil => "cil",
            Self::File => "file",
            Self::Interpreter => "interpreter",
            Self::Abi => "abi",
            Self::PkgConfig => "pkgconfig",
            Self::CMake => "cmake",
            Self::Binary => "binary",
            Self::KernelModule => "kmod",
        }
    }

    /// Parse a dependency class from its prefix
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix.to_lowercase().as_str() {
            "" => Some(Self::Package),
            "soname" => Some(Self::Soname),
            "python" | "python3" | "python3dist" => Some(Self::Python),
            "perl" => Some(Self::Perl),
            "ruby" => Some(Self::Ruby),
            "java" => Some(Self::Java),
            "cil" => Some(Self::Cil),
            "file" => Some(Self::File),
            "interpreter" => Some(Self::Interpreter),
            "abi" => Some(Self::Abi),
            "pkgconfig" | "pkg-config" => Some(Self::PkgConfig),
            "cmake" => Some(Self::CMake),
            "binary" | "bin" => Some(Self::Binary),
            "kmod" | "kernel" => Some(Self::KernelModule),
            _ => None,
        }
    }

    /// Return all dependency classes
    pub fn all() -> &'static [DependencyClass] {
        &[
            Self::Package,
            Self::Soname,
            Self::Python,
            Self::Perl,
            Self::Ruby,
            Self::Java,
            Self::Cil,
            Self::File,
            Self::Interpreter,
            Self::Abi,
            Self::PkgConfig,
            Self::CMake,
            Self::Binary,
            Self::KernelModule,
        ]
    }

    /// Is this a language-specific dependency class?
    pub fn is_language(&self) -> bool {
        matches!(
            self,
            Self::Python | Self::Perl | Self::Ruby | Self::Java | Self::Cil
        )
    }

    /// Is this a build-time dependency class?
    pub fn is_build_time(&self) -> bool {
        matches!(self, Self::PkgConfig | Self::CMake)
    }

    /// Is this a system-level dependency class?
    pub fn is_system(&self) -> bool {
        matches!(
            self,
            Self::Package | Self::Soname | Self::File | Self::Interpreter | Self::Abi | Self::Binary | Self::KernelModule
        )
    }

    /// Get a human-readable description of this dependency class
    pub fn description(&self) -> &'static str {
        match self {
            Self::Package => "Package dependency",
            Self::Soname => "Shared library (soname)",
            Self::Python => "Python module",
            Self::Perl => "Perl module",
            Self::Ruby => "Ruby gem",
            Self::Java => "Java package",
            Self::Cil => ".NET/Mono assembly",
            Self::File => "File path",
            Self::Interpreter => "ELF interpreter",
            Self::Abi => "ABI compatibility",
            Self::PkgConfig => "pkg-config module",
            Self::CMake => "CMake package",
            Self::Binary => "Executable binary",
            Self::KernelModule => "Kernel module",
        }
    }
}

impl fmt::Display for DependencyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.prefix())
    }
}

/// A language-specific dependency
///
/// This struct represents a parsed dependency with its class, name,
/// and optional version constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageDep {
    /// The dependency class (python, perl, ruby, etc.)
    pub class: DependencyClass,

    /// The name of the dependency within its ecosystem
    /// e.g., "requests" for python(requests)
    pub name: String,

    /// Optional version constraint
    /// e.g., ">=2.0" for python(requests>=2.0)
    pub version_constraint: Option<String>,
}

impl LanguageDep {
    /// Create a new language dependency
    pub fn new(class: DependencyClass, name: impl Into<String>) -> Self {
        Self {
            class,
            name: name.into(),
            version_constraint: None,
        }
    }

    /// Create with a version constraint
    pub fn with_version(mut self, constraint: impl Into<String>) -> Self {
        self.version_constraint = Some(constraint.into());
        self
    }

    /// Parse a dependency string like "python(requests>=2.0)"
    ///
    /// Returns None if the string is not a valid language-specific dependency.
    pub fn parse(s: &str) -> Option<Self> {
        // Check for pattern: prefix(name) or prefix(name>=version)
        let open_paren = s.find('(')?;
        let close_paren = s.rfind(')')?;

        if close_paren <= open_paren {
            return None;
        }

        let prefix = &s[..open_paren];
        let inner = &s[open_paren + 1..close_paren];

        if inner.is_empty() {
            return None;
        }

        let class = DependencyClass::from_prefix(prefix)?;

        // Parse name and optional version constraint
        // Look for version operators: >=, <=, >, <, ==, =
        let version_ops = [">=", "<=", "==", ">", "<", "="];

        for op in &version_ops {
            if let Some(pos) = inner.find(op) {
                let name = inner[..pos].trim();
                let version = inner[pos..].trim();
                if !name.is_empty() && !version.is_empty() {
                    return Some(Self {
                        class,
                        name: name.to_string(),
                        version_constraint: Some(version.to_string()),
                    });
                }
            }
        }

        // No version constraint found
        Some(Self {
            class,
            name: inner.trim().to_string(),
            version_constraint: None,
        })
    }

    /// Format as a dependency string
    pub fn to_dep_string(&self) -> String {
        let prefix = self.class.prefix();
        if prefix.is_empty() {
            if let Some(ref ver) = self.version_constraint {
                format!("{}{}", self.name, ver)
            } else {
                self.name.clone()
            }
        } else if let Some(ref ver) = self.version_constraint {
            format!("{}({}{})", prefix, self.name, ver)
        } else {
            format!("{}({})", prefix, self.name)
        }
    }

    /// Check if this dependency is satisfied by an installed version
    pub fn is_satisfied_by(&self, installed_version: Option<&str>) -> bool {
        // If no version constraint, any version satisfies
        let Some(ref constraint) = self.version_constraint else {
            return installed_version.is_some();
        };

        // If we have a constraint but no installed version, not satisfied
        let Some(installed) = installed_version else {
            return false;
        };

        // Parse and check the constraint
        // This is a simplified version - a full implementation would use
        // the version comparison logic from the version module
        Self::check_version_constraint(installed, constraint)
    }

    /// Simple version constraint checker
    fn check_version_constraint(installed: &str, constraint: &str) -> bool {
        // Parse operator from constraint
        let (op, required) = if let Some(ver) = constraint.strip_prefix(">=") {
            (">=", ver.trim())
        } else if let Some(ver) = constraint.strip_prefix("<=") {
            ("<=", ver.trim())
        } else if let Some(ver) = constraint.strip_prefix("==") {
            ("==", ver.trim())
        } else if let Some(ver) = constraint.strip_prefix(">") {
            (">", ver.trim())
        } else if let Some(ver) = constraint.strip_prefix("<") {
            ("<", ver.trim())
        } else if let Some(ver) = constraint.strip_prefix("=") {
            ("=", ver.trim())
        } else {
            // No operator means exact match
            ("=", constraint)
        };

        // Simple string comparison (works for many version schemes)
        // A full implementation would use semantic versioning or distro-specific comparison
        match op {
            ">=" => installed >= required,
            "<=" => installed <= required,
            ">" => installed > required,
            "<" => installed < required,
            "=" | "==" => installed == required,
            _ => false,
        }
    }
}

impl fmt::Display for LanguageDep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_dep_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================
    // DependencyClass tests
    // ====================

    #[test]
    fn test_dependency_class_prefix() {
        assert_eq!(DependencyClass::Package.prefix(), "");
        assert_eq!(DependencyClass::Python.prefix(), "python");
        assert_eq!(DependencyClass::Perl.prefix(), "perl");
        assert_eq!(DependencyClass::Ruby.prefix(), "ruby");
        assert_eq!(DependencyClass::Java.prefix(), "java");
    }

    #[test]
    fn test_dependency_class_from_prefix() {
        assert_eq!(
            DependencyClass::from_prefix("python"),
            Some(DependencyClass::Python)
        );
        assert_eq!(
            DependencyClass::from_prefix("PYTHON"),
            Some(DependencyClass::Python)
        );
        assert_eq!(
            DependencyClass::from_prefix("perl"),
            Some(DependencyClass::Perl)
        );
        assert_eq!(DependencyClass::from_prefix("unknown"), None);
    }

    #[test]
    fn test_dependency_class_is_language() {
        assert!(DependencyClass::Python.is_language());
        assert!(DependencyClass::Perl.is_language());
        assert!(DependencyClass::Ruby.is_language());
        assert!(DependencyClass::Java.is_language());
        assert!(DependencyClass::Cil.is_language());
        assert!(!DependencyClass::Package.is_language());
        assert!(!DependencyClass::Soname.is_language());
        assert!(!DependencyClass::File.is_language());
    }

    // ====================
    // LanguageDep parsing
    // ====================

    #[test]
    fn test_parse_python_dep_simple() {
        let dep = LanguageDep::parse("python(requests)").unwrap();
        assert_eq!(dep.class, DependencyClass::Python);
        assert_eq!(dep.name, "requests");
        assert_eq!(dep.version_constraint, None);
    }

    #[test]
    fn test_parse_python_dep_with_version() {
        let dep = LanguageDep::parse("python(requests>=2.0)").unwrap();
        assert_eq!(dep.class, DependencyClass::Python);
        assert_eq!(dep.name, "requests");
        assert_eq!(dep.version_constraint, Some(">=2.0".to_string()));
    }

    #[test]
    fn test_parse_perl_dep() {
        let dep = LanguageDep::parse("perl(DBI>=1.600)").unwrap();
        assert_eq!(dep.class, DependencyClass::Perl);
        assert_eq!(dep.name, "DBI");
        assert_eq!(dep.version_constraint, Some(">=1.600".to_string()));
    }

    #[test]
    fn test_parse_ruby_dep() {
        let dep = LanguageDep::parse("ruby(bundler)").unwrap();
        assert_eq!(dep.class, DependencyClass::Ruby);
        assert_eq!(dep.name, "bundler");
        assert_eq!(dep.version_constraint, None);
    }

    #[test]
    fn test_parse_file_dep() {
        let dep = LanguageDep::parse("file(/usr/bin/python3)").unwrap();
        assert_eq!(dep.class, DependencyClass::File);
        assert_eq!(dep.name, "/usr/bin/python3");
    }

    #[test]
    fn test_parse_soname_dep() {
        let dep = LanguageDep::parse("soname(libssl.so.3)").unwrap();
        assert_eq!(dep.class, DependencyClass::Soname);
        assert_eq!(dep.name, "libssl.so.3");
    }

    #[test]
    fn test_parse_invalid_no_parens() {
        assert!(LanguageDep::parse("python-requests").is_none());
    }

    #[test]
    fn test_parse_invalid_empty_name() {
        assert!(LanguageDep::parse("python()").is_none());
    }

    #[test]
    fn test_parse_invalid_unmatched_parens() {
        assert!(LanguageDep::parse("python(requests").is_none());
    }

    // ====================
    // LanguageDep formatting
    // ====================

    #[test]
    fn test_to_dep_string_simple() {
        let dep = LanguageDep::new(DependencyClass::Python, "requests");
        assert_eq!(dep.to_dep_string(), "python(requests)");
    }

    #[test]
    fn test_to_dep_string_with_version() {
        let dep = LanguageDep::new(DependencyClass::Python, "requests").with_version(">=2.0");
        assert_eq!(dep.to_dep_string(), "python(requests>=2.0)");
    }

    #[test]
    fn test_roundtrip_parse_format() {
        let original = "python(requests>=2.0.0)";
        let parsed = LanguageDep::parse(original).unwrap();
        assert_eq!(parsed.to_dep_string(), original);
    }

    // ====================
    // Version satisfaction
    // ====================

    #[test]
    fn test_satisfied_no_constraint() {
        let dep = LanguageDep::new(DependencyClass::Python, "requests");
        assert!(dep.is_satisfied_by(Some("2.0")));
        assert!(!dep.is_satisfied_by(None));
    }

    #[test]
    fn test_satisfied_ge_constraint() {
        let dep = LanguageDep::new(DependencyClass::Python, "requests").with_version(">=2.0");
        assert!(dep.is_satisfied_by(Some("2.0")));
        assert!(dep.is_satisfied_by(Some("2.1")));
        assert!(dep.is_satisfied_by(Some("3.0")));
        assert!(!dep.is_satisfied_by(Some("1.9")));
    }

    #[test]
    fn test_satisfied_le_constraint() {
        let dep = LanguageDep::new(DependencyClass::Python, "requests").with_version("<=2.0");
        assert!(dep.is_satisfied_by(Some("2.0")));
        assert!(dep.is_satisfied_by(Some("1.9")));
        assert!(!dep.is_satisfied_by(Some("2.1")));
    }

    #[test]
    fn test_satisfied_eq_constraint() {
        let dep = LanguageDep::new(DependencyClass::Python, "requests").with_version("==2.0");
        assert!(dep.is_satisfied_by(Some("2.0")));
        assert!(!dep.is_satisfied_by(Some("2.1")));
        assert!(!dep.is_satisfied_by(Some("1.9")));
    }
}
