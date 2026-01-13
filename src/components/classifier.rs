// src/components/classifier.rs

//! File-to-component classification based on path patterns
//!
//! This module classifies files into components using strict rules.
//! `:runtime` is the safe default - only files we're 100% confident
//! can be separated go elsewhere.

use std::path::Path;

/// Component types for package splitting
///
/// Components are first-class installable units. A package is split into
/// components during installation, and users can install specific components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentType {
    /// Executables, assets, helpers - the main package content
    /// Default bucket for anything not clearly belonging elsewhere
    Runtime,
    /// Shared libraries (.so files in lib directories)
    Lib,
    /// Development files (headers, static libs, pkg-config)
    Devel,
    /// Documentation (man pages, info, docs)
    Doc,
    /// Configuration files (/etc/*)
    Config,
}

impl ComponentType {
    /// Get the string representation of the component type
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Lib => "lib",
            Self::Devel => "devel",
            Self::Doc => "doc",
            Self::Config => "config",
        }
    }

    /// Parse a component type from a string
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "runtime" => Some(Self::Runtime),
            "lib" => Some(Self::Lib),
            "devel" => Some(Self::Devel),
            "doc" => Some(Self::Doc),
            "config" => Some(Self::Config),
            _ => None,
        }
    }

    /// Is this component installed by default when user doesn't specify?
    ///
    /// Default components: :runtime, :lib, :config
    /// Optional components: :devel, :doc (must be explicitly requested)
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Runtime | Self::Lib | Self::Config)
    }

    /// Return all component types
    pub fn all() -> &'static [ComponentType] {
        &[
            Self::Runtime,
            Self::Lib,
            Self::Devel,
            Self::Doc,
            Self::Config,
        ]
    }

    /// Return only default component types
    pub fn defaults() -> &'static [ComponentType] {
        &[Self::Runtime, Self::Lib, Self::Config]
    }
}

impl std::fmt::Display for ComponentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, ":{}", self.as_str())
    }
}

/// Classifies files into components based on their paths
///
/// Classification rules are STRICT - we only split out files we're
/// 100% confident can be safely separated. Everything else goes to `:runtime`.
pub struct ComponentClassifier;

impl ComponentClassifier {
    /// Classify a file path to its component type
    ///
    /// Order of checks matters - more specific rules come first.
    pub fn classify(path: &Path) -> ComponentType {
        let path_str = path.to_string_lossy();

        // 1. CONFIG (/etc/*)
        if path.starts_with("/etc") {
            return ComponentType::Config;
        }

        // 2. DEVEL (Headers, pkgconfig, static libs, cmake)
        // Check before :lib to ensure static libs go to :devel, not :lib
        if Self::is_devel_file(&path_str) {
            return ComponentType::Devel;
        }

        // 3. DOC (Man pages, info, doc share)
        if Self::is_doc_file(path) {
            return ComponentType::Doc;
        }

        // 4. LIB (Shared objects in library paths)
        // Uses expanded detection for multi-arch support
        if Self::is_lib_file(&path_str) {
            return ComponentType::Lib;
        }

        // 5. RUNTIME (The Safe Default)
        // Includes /bin, /sbin, /usr/share (assets), /usr/libexec, and everything else
        ComponentType::Runtime
    }

    /// Check if file is a development file
    fn is_devel_file(path_str: &str) -> bool {
        // Header files
        if path_str.starts_with("/usr/include/") || path_str.starts_with("/include/") {
            return true;
        }

        // Static libraries and libtool archives
        if path_str.ends_with(".a") || path_str.ends_with(".la") {
            return true;
        }

        // pkg-config files
        if path_str.contains("/pkgconfig/") && path_str.ends_with(".pc") {
            return true;
        }

        // CMake files
        if path_str.contains("/cmake/") {
            return true;
        }

        // Aclocal/autoconf macros
        if path_str.starts_with("/usr/share/aclocal/") {
            return true;
        }

        false
    }

    /// Check if file is documentation
    fn is_doc_file(path: &Path) -> bool {
        path.starts_with("/usr/share/doc")
            || path.starts_with("/usr/share/man")
            || path.starts_with("/usr/share/info")
            || path.starts_with("/usr/share/gtk-doc")
            || path.starts_with("/usr/share/help")
    }

    /// Check if file is a shared library
    ///
    /// Multi-arch aware: checks for /lib/ or /lib64/ anywhere in path
    /// to support Debian multiarch (/usr/lib/x86_64-linux-gnu/) and
    /// various distro layouts.
    fn is_lib_file(path_str: &str) -> bool {
        // Must contain .so (shared object indicator)
        if !path_str.contains(".so") {
            return false;
        }

        // Must be in a library directory
        // Supports: /lib/, /lib64/, /usr/lib/, /usr/lib64/,
        // /usr/lib/x86_64-linux-gnu/, etc.
        path_str.contains("/lib/") || path_str.contains("/lib64/")
    }

    /// Classify multiple paths and return grouped results
    pub fn classify_all(paths: &[String]) -> std::collections::HashMap<ComponentType, Vec<String>> {
        let mut result = std::collections::HashMap::new();

        for path in paths {
            let comp_type = Self::classify(Path::new(path));
            result
                .entry(comp_type)
                .or_insert_with(Vec::new)
                .push(path.clone());
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===================
    // Runtime (Default)
    // ===================

    #[test]
    fn test_classify_runtime_binaries() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/bin/bash")),
            ComponentType::Runtime
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/bin/ls")),
            ComponentType::Runtime
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/sbin/init")),
            ComponentType::Runtime
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/sbin/nginx")),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_classify_runtime_libexec() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/libexec/git-core/git-add")),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_classify_runtime_share_assets() {
        // Assets go to runtime now (no :data catch-all)
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/icons/hicolor/index.theme")),
            ComponentType::Runtime
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/applications/firefox.desktop")),
            ComponentType::Runtime
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/myapp/helper.sh")),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_classify_runtime_var() {
        // /var goes to runtime
        assert_eq!(
            ComponentClassifier::classify(Path::new("/var/lib/myapp/data.db")),
            ComponentType::Runtime
        );
    }

    // ===================
    // Config
    // ===================

    #[test]
    fn test_classify_config() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/etc/nginx/nginx.conf")),
            ComponentType::Config
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/etc/passwd")),
            ComponentType::Config
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/etc/systemd/system/myapp.service")),
            ComponentType::Config
        );
    }

    // ===================
    // Devel
    // ===================

    #[test]
    fn test_classify_devel_headers() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/include/stdio.h")),
            ComponentType::Devel
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/include/openssl/ssl.h")),
            ComponentType::Devel
        );
    }

    #[test]
    fn test_classify_devel_static_libs() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/libssl.a")),
            ComponentType::Devel
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib64/libcrypto.a")),
            ComponentType::Devel
        );
    }

    #[test]
    fn test_classify_devel_libtool() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/libfoo.la")),
            ComponentType::Devel
        );
    }

    #[test]
    fn test_classify_devel_pkgconfig() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/pkgconfig/openssl.pc")),
            ComponentType::Devel
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib64/pkgconfig/zlib.pc")),
            ComponentType::Devel
        );
    }

    #[test]
    fn test_classify_devel_cmake() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/cmake/OpenSSL/OpenSSLConfig.cmake")),
            ComponentType::Devel
        );
    }

    // ===================
    // Lib
    // ===================

    #[test]
    fn test_classify_lib_basic() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/libssl.so.3")),
            ComponentType::Lib
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/lib64/libc.so.6")),
            ComponentType::Lib
        );
    }

    #[test]
    fn test_classify_lib_multiarch() {
        // Debian multiarch paths
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/x86_64-linux-gnu/libssl.so.3")),
            ComponentType::Lib
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/lib/x86_64-linux-gnu/libc.so.6")),
            ComponentType::Lib
        );
    }

    #[test]
    fn test_classify_lib_versioned() {
        // Various versioned .so patterns
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib64/libcurl.so.4.8.0")),
            ComponentType::Lib
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/lib/libz.so")),
            ComponentType::Lib
        );
    }

    // ===================
    // Doc
    // ===================

    #[test]
    fn test_classify_doc_man() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/man/man1/ls.1.gz")),
            ComponentType::Doc
        );
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/man/man5/passwd.5")),
            ComponentType::Doc
        );
    }

    #[test]
    fn test_classify_doc_info() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/info/gcc.info")),
            ComponentType::Doc
        );
    }

    #[test]
    fn test_classify_doc_share() {
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/doc/nginx/README.md")),
            ComponentType::Doc
        );
    }

    // ===================
    // Component Type Methods
    // ===================

    #[test]
    fn test_component_type_is_default() {
        assert!(ComponentType::Runtime.is_default());
        assert!(ComponentType::Lib.is_default());
        assert!(ComponentType::Config.is_default());
        assert!(!ComponentType::Devel.is_default());
        assert!(!ComponentType::Doc.is_default());
    }

    #[test]
    fn test_component_type_parse() {
        assert_eq!(ComponentType::parse("runtime"), Some(ComponentType::Runtime));
        assert_eq!(ComponentType::parse("lib"), Some(ComponentType::Lib));
        assert_eq!(ComponentType::parse("devel"), Some(ComponentType::Devel));
        assert_eq!(ComponentType::parse("doc"), Some(ComponentType::Doc));
        assert_eq!(ComponentType::parse("config"), Some(ComponentType::Config));
        assert_eq!(ComponentType::parse("invalid"), None);
    }

    #[test]
    fn test_component_type_display() {
        assert_eq!(format!("{}", ComponentType::Runtime), ":runtime");
        assert_eq!(format!("{}", ComponentType::Lib), ":lib");
    }

    // ===================
    // classify_all
    // ===================

    #[test]
    fn test_classify_all() {
        let paths = vec![
            "/usr/bin/nginx".to_string(),
            "/usr/lib/libssl.so.3".to_string(),
            "/etc/nginx/nginx.conf".to_string(),
            "/usr/share/doc/nginx/README".to_string(),
            "/usr/include/openssl/ssl.h".to_string(),
        ];

        let classified = ComponentClassifier::classify_all(&paths);

        assert_eq!(classified.get(&ComponentType::Runtime).map(|v| v.len()), Some(1));
        assert_eq!(classified.get(&ComponentType::Lib).map(|v| v.len()), Some(1));
        assert_eq!(classified.get(&ComponentType::Config).map(|v| v.len()), Some(1));
        assert_eq!(classified.get(&ComponentType::Doc).map(|v| v.len()), Some(1));
        assert_eq!(classified.get(&ComponentType::Devel).map(|v| v.len()), Some(1));
    }

    // ===================
    // Edge Cases
    // ===================

    #[test]
    fn test_edge_case_so_in_name_but_not_lib() {
        // File with "so" in name but not a shared library
        // Should go to runtime since it's not in a lib directory
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/bin/also-something")),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_edge_case_lib_in_usr_share() {
        // .so file in /usr/share should NOT be classified as :lib
        // (it's probably a plugin or data file)
        assert_eq!(
            ComponentClassifier::classify(Path::new("/usr/share/myapp/plugin.so")),
            ComponentType::Runtime
        );
    }
}
