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

    /// Cross-compilation configuration (optional)
    ///
    /// Used for bootstrap builds where we need to build for a different
    /// target or use a specific sysroot/toolchain.
    #[serde(default)]
    pub cross: Option<CrossSection>,

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
            .next_back()
            .unwrap_or("source.tar.gz")
            .to_string()
    }

    /// Check if this recipe requires cross-compilation
    pub fn is_cross_build(&self) -> bool {
        self.cross.as_ref().is_some_and(|c| {
            c.target.is_some() || c.sysroot.is_some() || c.cross_tools.is_some()
        })
    }

    /// Get the build stage (defaults to Final)
    pub fn build_stage(&self) -> BuildStage {
        self.cross
            .as_ref()
            .and_then(|c| c.stage)
            .unwrap_or(BuildStage::Final)
    }

    /// Get all build dependencies (requires + makedepends)
    pub fn all_build_deps(&self) -> Vec<&str> {
        let mut deps: Vec<&str> = self.build.requires.iter().map(|s| s.as_str()).collect();
        deps.extend(self.build.makedepends.iter().map(|s| s.as_str()));
        deps
    }

    /// Get cross-compilation environment variables
    ///
    /// Returns a HashMap of env vars like CC, CXX, AR, etc. configured
    /// for cross-compilation based on the [cross] section.
    pub fn cross_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();

        let cross = match &self.cross {
            Some(c) => c,
            None => return env,
        };

        // Get tool prefix for constructing tool names
        let prefix = cross.tool_prefix.as_deref().unwrap_or("");
        let tools_dir = cross.cross_tools.as_deref().unwrap_or("");

        // Helper to construct tool path
        let tool_path = |tool: &str, override_val: &Option<String>| -> String {
            if let Some(val) = override_val {
                return val.clone();
            }
            if prefix.is_empty() {
                return tool.to_string();
            }
            let prefixed = format!("{}-{}", prefix, tool);
            if tools_dir.is_empty() {
                prefixed
            } else {
                format!("{}/{}", tools_dir, prefixed)
            }
        };

        // Set standard cross-compilation variables
        env.insert("CC".to_string(), tool_path("gcc", &cross.cc));
        env.insert("CXX".to_string(), tool_path("g++", &cross.cxx));
        env.insert("AR".to_string(), tool_path("ar", &cross.ar));
        env.insert("LD".to_string(), tool_path("ld", &cross.ld));
        env.insert("RANLIB".to_string(), tool_path("ranlib", &cross.ranlib));
        env.insert("NM".to_string(), tool_path("nm", &cross.nm));
        env.insert("STRIP".to_string(), tool_path("strip", &cross.strip));

        // Set target if specified
        if let Some(target) = &cross.target {
            env.insert("TARGET".to_string(), target.clone());
            env.insert("CROSS_COMPILE".to_string(), format!("{}-", prefix));
        }

        // Set sysroot if specified
        if let Some(sysroot) = &cross.sysroot {
            env.insert("SYSROOT".to_string(), sysroot.clone());
            // GCC needs --sysroot in CFLAGS/LDFLAGS
            let sysroot_flag = format!("--sysroot={}", sysroot);
            env.insert(
                "CFLAGS".to_string(),
                format!("{} {}", env.get("CFLAGS").unwrap_or(&String::new()), sysroot_flag),
            );
            env.insert(
                "CXXFLAGS".to_string(),
                format!(
                    "{} {}",
                    env.get("CXXFLAGS").unwrap_or(&String::new()),
                    sysroot_flag
                ),
            );
            env.insert(
                "LDFLAGS".to_string(),
                format!(
                    "{} {}",
                    env.get("LDFLAGS").unwrap_or(&String::new()),
                    sysroot_flag
                ),
            );
        }

        // Set bootstrap stage marker
        if let Some(stage) = &cross.stage {
            env.insert("CONARY_STAGE".to_string(), stage.as_str().to_string());
        }

        env
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
    /// Runtime dependencies (installed with the package)
    ///
    /// Format: `["package", "package:component", "package>=1.0"]`
    #[serde(default)]
    pub requires: Vec<String>,

    /// Build-time only dependencies (makedepends)
    ///
    /// These packages are needed to build but not at runtime.
    /// The Kitchen will auto-install these before cooking.
    /// Format: `["gcc", "make", "pkgconf", "cmake"]`
    #[serde(default)]
    pub makedepends: Vec<String>,

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

/// Cross-compilation configuration
///
/// Used for bootstrap builds where we need to compile for a different
/// target architecture or use a specific sysroot containing the toolchain.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrossSection {
    /// Target triple (e.g., "x86_64-unknown-linux-gnu", "aarch64-linux-gnu")
    ///
    /// If not specified, builds for the host architecture.
    #[serde(default)]
    pub target: Option<String>,

    /// Path to the sysroot containing the target's libraries and headers
    ///
    /// For bootstrap: `/opt/sysroot/stage0`, `/opt/sysroot/stage1`
    #[serde(default)]
    pub sysroot: Option<String>,

    /// Directory containing cross-compilation tools
    ///
    /// If specified, these tools are used instead of system tools.
    /// Example: `/opt/cross/bin` containing `x86_64-linux-gnu-gcc`
    #[serde(default)]
    pub cross_tools: Option<String>,

    /// Bootstrap stage
    ///
    /// - `stage0`: Built with host toolchain, runs on host, produces target code
    /// - `stage1`: Built with stage0 tools, runs on target, may still use host libs
    /// - `stage2`: Fully self-hosted, built with stage1 tools
    /// - `final`: Production build (default if not specified)
    #[serde(default)]
    pub stage: Option<BuildStage>,

    /// Prefix for cross-compiler commands
    ///
    /// If specified, commands like `gcc` become `<prefix>-gcc`.
    /// Example: `x86_64-linux-gnu` → `x86_64-linux-gnu-gcc`
    #[serde(default)]
    pub tool_prefix: Option<String>,

    /// Override CC compiler
    #[serde(default)]
    pub cc: Option<String>,

    /// Override CXX compiler
    #[serde(default)]
    pub cxx: Option<String>,

    /// Override AR archiver
    #[serde(default)]
    pub ar: Option<String>,

    /// Override LD linker
    #[serde(default)]
    pub ld: Option<String>,

    /// Override RANLIB
    #[serde(default)]
    pub ranlib: Option<String>,

    /// Override NM
    #[serde(default)]
    pub nm: Option<String>,

    /// Override STRIP
    #[serde(default)]
    pub strip: Option<String>,
}

/// Bootstrap build stage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BuildStage {
    /// Stage 0: Cross-compiled from host
    ///
    /// Built using host toolchain to produce target-runnable code.
    /// Typically a minimal toolchain (binutils + gcc + glibc).
    Stage0,

    /// Stage 1: Built with stage0 tools
    ///
    /// Runs on target but may still link against some host libraries.
    /// Used to build a fully native toolchain.
    Stage1,

    /// Stage 2: Fully self-hosted
    ///
    /// Built entirely with stage1 tools. This is the first "native" build
    /// that doesn't depend on the host system at all.
    Stage2,

    /// Final: Production build (default)
    ///
    /// Normal production build using the system's native toolchain.
    #[default]
    Final,
}

impl BuildStage {
    /// Get the stage name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildStage::Stage0 => "stage0",
            BuildStage::Stage1 => "stage1",
            BuildStage::Stage2 => "stage2",
            BuildStage::Final => "final",
        }
    }

    /// Check if this is a bootstrap stage (not final)
    pub fn is_bootstrap(&self) -> bool {
        !matches!(self, BuildStage::Final)
    }
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

    const CROSS_RECIPE: &str = r#"
[package]
name = "glibc"
version = "2.38"

[source]
archive = "https://ftp.gnu.org/gnu/glibc/glibc-%(version)s.tar.xz"
checksum = "sha256:abc123"

[build]
requires = ["linux-headers"]
makedepends = ["gcc", "make", "bison", "gawk", "texinfo"]
configure = "../configure --prefix=/usr --host=%(target)s"
make = "make"
install = "make install DESTDIR=%(destdir)s"

[cross]
target = "x86_64-conary-linux-gnu"
sysroot = "/opt/sysroot/stage0"
cross_tools = "/opt/cross/bin"
stage = "stage1"
tool_prefix = "x86_64-conary-linux-gnu"
"#;

    #[test]
    fn test_parse_cross_recipe() {
        let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();

        assert_eq!(recipe.package.name, "glibc");
        assert!(recipe.cross.is_some());

        let cross = recipe.cross.as_ref().unwrap();
        assert_eq!(cross.target.as_deref(), Some("x86_64-conary-linux-gnu"));
        assert_eq!(cross.sysroot.as_deref(), Some("/opt/sysroot/stage0"));
        assert_eq!(cross.cross_tools.as_deref(), Some("/opt/cross/bin"));
        assert_eq!(cross.stage, Some(BuildStage::Stage1));
        assert_eq!(cross.tool_prefix.as_deref(), Some("x86_64-conary-linux-gnu"));
    }

    #[test]
    fn test_is_cross_build() {
        // Recipe without cross section
        let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
        assert!(!recipe.is_cross_build());

        // Recipe with cross section
        let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
        assert!(recipe.is_cross_build());

        // Recipe with empty cross section (no actual cross settings)
        let empty_cross = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[cross]
"#;
        let recipe: Recipe = toml::from_str(empty_cross).unwrap();
        assert!(!recipe.is_cross_build());
    }

    #[test]
    fn test_build_stage() {
        // Default stage is Final
        let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
        assert_eq!(recipe.build_stage(), BuildStage::Final);

        // Cross recipe with explicit stage
        let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
        assert_eq!(recipe.build_stage(), BuildStage::Stage1);

        // Test each stage
        for (stage_str, expected) in [
            ("stage0", BuildStage::Stage0),
            ("stage1", BuildStage::Stage1),
            ("stage2", BuildStage::Stage2),
            ("final", BuildStage::Final),
        ] {
            let toml = format!(
                r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[cross]
stage = "{}"
"#,
                stage_str
            );
            let recipe: Recipe = toml::from_str(&toml).unwrap();
            assert_eq!(recipe.build_stage(), expected);
        }
    }

    #[test]
    fn test_build_stage_methods() {
        assert_eq!(BuildStage::Stage0.as_str(), "stage0");
        assert_eq!(BuildStage::Stage1.as_str(), "stage1");
        assert_eq!(BuildStage::Stage2.as_str(), "stage2");
        assert_eq!(BuildStage::Final.as_str(), "final");

        assert!(BuildStage::Stage0.is_bootstrap());
        assert!(BuildStage::Stage1.is_bootstrap());
        assert!(BuildStage::Stage2.is_bootstrap());
        assert!(!BuildStage::Final.is_bootstrap());
    }

    #[test]
    fn test_all_build_deps() {
        let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
        let deps = recipe.all_build_deps();

        // Should include both requires and makedepends
        assert!(deps.contains(&"linux-headers"));
        assert!(deps.contains(&"gcc"));
        assert!(deps.contains(&"make"));
        assert!(deps.contains(&"bison"));
        assert!(deps.contains(&"gawk"));
        assert!(deps.contains(&"texinfo"));
        assert_eq!(deps.len(), 6); // 1 require + 5 makedepends
    }

    #[test]
    fn test_cross_env_basic() {
        let recipe: Recipe = toml::from_str(CROSS_RECIPE).unwrap();
        let env = recipe.cross_env();

        // Should have cross-compiler paths
        assert_eq!(
            env.get("CC").unwrap(),
            "/opt/cross/bin/x86_64-conary-linux-gnu-gcc"
        );
        assert_eq!(
            env.get("CXX").unwrap(),
            "/opt/cross/bin/x86_64-conary-linux-gnu-g++"
        );
        assert_eq!(
            env.get("AR").unwrap(),
            "/opt/cross/bin/x86_64-conary-linux-gnu-ar"
        );

        // Should have target and sysroot
        assert_eq!(env.get("TARGET").unwrap(), "x86_64-conary-linux-gnu");
        assert_eq!(env.get("SYSROOT").unwrap(), "/opt/sysroot/stage0");

        // Should have sysroot in CFLAGS
        assert!(env.get("CFLAGS").unwrap().contains("--sysroot=/opt/sysroot/stage0"));

        // Should have stage marker
        assert_eq!(env.get("CONARY_STAGE").unwrap(), "stage1");
    }

    #[test]
    fn test_cross_env_with_overrides() {
        let toml = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
make = "make"

[cross]
target = "aarch64-linux-gnu"
tool_prefix = "aarch64-linux-gnu"
cc = "/custom/path/clang"
cxx = "/custom/path/clang++"
"#;
        let recipe: Recipe = toml::from_str(toml).unwrap();
        let env = recipe.cross_env();

        // Overridden tools should use custom paths
        assert_eq!(env.get("CC").unwrap(), "/custom/path/clang");
        assert_eq!(env.get("CXX").unwrap(), "/custom/path/clang++");

        // Non-overridden tools should use prefix
        assert_eq!(env.get("AR").unwrap(), "aarch64-linux-gnu-ar");
        assert_eq!(env.get("LD").unwrap(), "aarch64-linux-gnu-ld");
    }

    #[test]
    fn test_cross_env_empty_for_non_cross() {
        let recipe: Recipe = toml::from_str(SAMPLE_RECIPE).unwrap();
        let env = recipe.cross_env();

        // Non-cross recipe should return empty env
        assert!(env.is_empty());
    }

    #[test]
    fn test_makedepends_parsing() {
        let toml = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
requires = ["runtime-dep"]
makedepends = ["cmake", "ninja", "pkgconf"]
configure = "cmake -B build"
make = "cmake --build build"
install = "cmake --install build --prefix %(destdir)s"
"#;
        let recipe: Recipe = toml::from_str(toml).unwrap();

        assert_eq!(recipe.build.requires, vec!["runtime-dep"]);
        assert_eq!(recipe.build.makedepends, vec!["cmake", "ninja", "pkgconf"]);
    }

    #[test]
    fn test_cross_section_defaults() {
        let cross = CrossSection::default();

        assert!(cross.target.is_none());
        assert!(cross.sysroot.is_none());
        assert!(cross.cross_tools.is_none());
        assert!(cross.stage.is_none());
        assert!(cross.tool_prefix.is_none());
        assert!(cross.cc.is_none());
        assert!(cross.cxx.is_none());
        assert!(cross.ar.is_none());
        assert!(cross.ld.is_none());
    }
}
