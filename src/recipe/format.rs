// src/recipe/format.rs

//! Recipe file format definitions
//!
//! Recipes are TOML files that describe how to build a package from source.
//! The format is inspired by Foresight Linux but simplified for Rust parsing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A complete recipe for building a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Package metadata
    pub package: PackageSection,

    /// Source archives and signature info
    pub source: SourceSection,

    /// Build instructions
    pub build: BuildSection,

    /// Patches to apply (optional)
    #[serde(default)]
    pub patches: Option<PatchSection>,

    /// Component classification overrides (optional)
    #[serde(default)]
    pub components: Option<ComponentSection>,

    /// Variables for substitution (optional)
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

impl Recipe {
    /// Substitute variables in a string
    ///
    /// Replaces `%(name)s` patterns with their values from:
    /// 1. Built-in variables (version, name, destdir, etc.)
    /// 2. Custom variables from the [variables] section
    pub fn substitute(&self, template: &str, destdir: &str) -> String {
        let mut result = template.to_string();

        // Built-in variables
        result = result.replace("%(version)s", &self.package.version);
        result = result.replace("%(name)s", &self.package.name);
        result = result.replace("%(destdir)s", destdir);

        // Custom variables
        for (key, value) in &self.variables {
            result = result.replace(&format!("%({})s", key), value);
        }

        result
    }

    /// Get the archive URL with variables substituted
    pub fn archive_url(&self) -> String {
        self.substitute(&self.source.archive, "")
    }

    /// Get the archive filename from the URL
    pub fn archive_filename(&self) -> String {
        self.archive_url()
            .split('/')
            .last()
            .unwrap_or("source.tar.gz")
            .to_string()
    }
}

/// Package metadata section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSection {
    /// Package name
    pub name: String,

    /// Package version
    pub version: String,

    /// Release number (for rebuilds of same version)
    #[serde(default = "default_release")]
    pub release: String,

    /// Short description
    #[serde(default)]
    pub summary: Option<String>,

    /// Full description
    #[serde(default)]
    pub description: Option<String>,

    /// License identifier (SPDX)
    #[serde(default)]
    pub license: Option<String>,

    /// Homepage URL
    #[serde(default)]
    pub homepage: Option<String>,
}

fn default_release() -> String {
    "1".to_string()
}

/// Source archive section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSection {
    /// Primary source archive URL
    ///
    /// Supports `%(version)s` substitution.
    /// Example: `https://nginx.org/download/nginx-%(version)s.tar.gz`
    pub archive: String,

    /// Checksum for the archive (sha256:...)
    pub checksum: String,

    /// Optional signature URL for GPG verification
    #[serde(default)]
    pub signature: Option<String>,

    /// Additional source archives (for multi-source builds)
    #[serde(default)]
    pub additional: Vec<AdditionalSource>,

    /// Directory name after extraction (if different from archive name)
    #[serde(default)]
    pub extract_dir: Option<String>,
}

/// Additional source archive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdditionalSource {
    /// Source URL
    pub url: String,
    /// Checksum
    pub checksum: String,
    /// Where to extract (relative to main source)
    #[serde(default)]
    pub extract_to: Option<String>,
}

/// Patch configuration section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchSection {
    /// List of patches to apply
    #[serde(default)]
    pub files: Vec<PatchInfo>,
}

/// Information about a single patch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchInfo {
    /// Patch file URL or local path
    pub file: String,

    /// Checksum for remote patches
    #[serde(default)]
    pub checksum: Option<String>,

    /// Strip level for patch (default: 1)
    #[serde(default = "default_strip")]
    pub strip: u32,

    /// Apply only if condition is met (optional)
    #[serde(default)]
    pub condition: Option<String>,
}

fn default_strip() -> u32 {
    1
}

/// Build instructions section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSection {
    /// Build-time dependencies
    ///
    /// Format: `["package", "package:component", "package>=1.0"]`
    #[serde(default)]
    pub requires: Vec<String>,

    /// Configure command(s)
    ///
    /// Supports `%(variable)s` substitution.
    #[serde(default)]
    pub configure: Option<String>,

    /// Make/build command(s)
    #[serde(default)]
    pub make: Option<String>,

    /// Install command(s)
    ///
    /// Must install to `%(destdir)s`.
    #[serde(default)]
    pub install: Option<String>,

    /// Check/test command(s) (optional)
    #[serde(default)]
    pub check: Option<String>,

    /// Pre-configure setup commands
    #[serde(default)]
    pub setup: Option<String>,

    /// Post-install commands
    #[serde(default)]
    pub post_install: Option<String>,

    /// Environment variables to set during build
    #[serde(default)]
    pub environment: HashMap<String, String>,

    /// Working directory within source (relative path)
    #[serde(default)]
    pub workdir: Option<String>,

    /// Build script file (alternative to inline commands)
    ///
    /// Points to a Lua script that handles the build.
    /// Takes precedence over configure/make/install commands.
    #[serde(default)]
    pub script_file: Option<String>,

    /// Number of parallel jobs (default: auto)
    #[serde(default)]
    pub jobs: Option<u32>,
}

/// Component classification overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentSection {
    /// Files that belong to :devel component
    #[serde(default)]
    pub devel: Vec<String>,

    /// Files that belong to :doc component
    #[serde(default)]
    pub doc: Vec<String>,

    /// Files that belong to :lib component
    #[serde(default)]
    pub lib: Vec<String>,

    /// Files to exclude from packaging
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RECIPE: &str = r#"
[package]
name = "nginx"
version = "1.24.0"
summary = "High-performance HTTP server"
license = "BSD-2-Clause"
homepage = "https://nginx.org"

[source]
archive = "https://nginx.org/download/nginx-%(version)s.tar.gz"
checksum = "sha256:77a2541637b92a621e3ee76571f6e9af0b4e6a6a1f5b0fd3d5c9cf6c8c55e3"

[build]
requires = ["openssl:devel", "pcre:devel", "zlib:devel"]
configure = "./configure --prefix=/usr --with-http_ssl_module --with-http_v2_module"
make = "make -j%(jobs)s"
install = "make install DESTDIR=%(destdir)s"

[patches]
files = [
    { file = "nginx-1.24-fix-headers.patch", strip = 1 },
]

[variables]
jobs = "4"
"#;

    #[test]
    fn test_parse_recipe() {
        let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();

        assert_eq!(recipe.package.name, "nginx");
        assert_eq!(recipe.package.version, "1.24.0");
        assert_eq!(recipe.package.license.as_deref(), Some("BSD-2-Clause"));

        assert!(recipe.source.archive.contains("%(version)s"));
        assert!(recipe.source.checksum.starts_with("sha256:"));

        assert_eq!(recipe.build.requires.len(), 3);
        assert!(recipe.build.configure.is_some());
    }

    #[test]
    fn test_variable_substitution() {
        let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();

        let url = recipe.archive_url();
        assert!(url.contains("1.24.0"));
        assert!(!url.contains("%(version)s"));

        let install = recipe.substitute(recipe.build.install.as_ref().unwrap(), "/tmp/dest");
        assert!(install.contains("/tmp/dest"));
        assert!(!install.contains("%(destdir)s"));
    }

    #[test]
    fn test_archive_filename() {
        let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
        assert_eq!(recipe.archive_filename(), "nginx-1.24.0.tar.gz");
    }

    #[test]
    fn test_minimal_recipe() {
        let minimal = r#"
[package]
name = "hello"
version = "1.0"

[source]
archive = "https://example.com/hello-1.0.tar.gz"
checksum = "sha256:abc123"

[build]
configure = "./configure"
make = "make"
install = "make install DESTDIR=%(destdir)s"
"#;

        let recipe: Recipe = toml::from_str(minimal).unwrap();
        assert_eq!(recipe.package.name, "hello");
        assert_eq!(recipe.package.release, "1"); // default
        assert!(recipe.patches.is_none());
    }
}
