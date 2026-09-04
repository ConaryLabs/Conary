// crates/conary-core/src/recipe/format.rs

//! Recipe file format definitions
//!
//! Recipes are TOML files that describe how to build a package from source.
//! The format is inspired by Foresight Linux but simplified for Rust parsing.

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

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
        self.remote_source()
            .map(|source| self.substitute(&source.archive, ""))
            .unwrap_or_default()
    }

    /// Get the archive filename from the URL
    pub fn archive_filename(&self) -> String {
        self.archive_url()
            .split('/')
            .next_back()
            .unwrap_or("source.tar.gz")
            .to_string()
    }

    /// Get the remote archive source section, if this recipe uses one.
    pub fn remote_source(&self) -> Option<&RemoteSourceSection> {
        self.source.remote()
    }

    /// Get the local path source section, if this recipe uses one.
    pub fn local_source(&self) -> Option<&LocalSourceSection> {
        self.source.local()
    }

    /// Check if this recipe requires cross-compilation
    pub fn is_cross_build(&self) -> bool {
        self.cross
            .as_ref()
            .is_some_and(|c| c.target.is_some() || c.sysroot.is_some() || c.cross_tools.is_some())
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
                format!(
                    "{} {}",
                    env.get("CFLAGS").unwrap_or(&String::new()),
                    sysroot_flag
                ),
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

/// Source section.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SourceSection {
    /// Source fetched from an archive URL or local archive file.
    Remote(RemoteSourceSection),
    /// Source taken from a local workspace path relative to the recipe file.
    Local(LocalSourceSection),
}

impl SourceSection {
    /// Get the remote archive source, if present.
    pub fn remote(&self) -> Option<&RemoteSourceSection> {
        match self {
            Self::Remote(source) => Some(source),
            Self::Local(_) => None,
        }
    }

    /// Get the local path source, if present.
    pub fn local(&self) -> Option<&LocalSourceSection> {
        match self {
            Self::Remote(_) => None,
            Self::Local(source) => Some(source),
        }
    }
}

impl<'de> Deserialize<'de> for SourceSection {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSourceSection {
            #[serde(default)]
            archive: Option<String>,
            #[serde(default)]
            checksum: Option<String>,
            #[serde(default)]
            path: Option<PathBuf>,
            #[serde(default)]
            signature: Option<String>,
            #[serde(default)]
            additional: Vec<AdditionalSource>,
            #[serde(default)]
            extract_dir: Option<String>,
        }

        let raw = RawSourceSection::deserialize(deserializer)?;

        match (raw.archive, raw.path) {
            (Some(archive), None) => {
                let checksum = raw.checksum.ok_or_else(|| {
                    de::Error::custom("[source] archive recipes require a 'checksum' field")
                })?;
                Ok(Self::Remote(RemoteSourceSection {
                    archive,
                    checksum,
                    signature: raw.signature,
                    additional: raw.additional,
                    extract_dir: raw.extract_dir,
                }))
            }
            (None, Some(path)) => {
                if raw.checksum.is_some()
                    || raw.signature.is_some()
                    || !raw.additional.is_empty()
                    || raw.extract_dir.is_some()
                {
                    return Err(de::Error::custom(
                        "[source] path recipes cannot include archive-only fields",
                    ));
                }
                LocalSourceSection::validate_path(&path).map_err(de::Error::custom)?;
                Ok(Self::Local(LocalSourceSection { path }))
            }
            (Some(_), Some(_)) => Err(de::Error::custom(
                "[source] cannot contain both 'archive' and 'path'",
            )),
            (None, None) => Err(de::Error::custom(
                "[source] must contain either 'archive' or 'path'",
            )),
        }
    }
}

/// Remote source archive section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSourceSection {
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

/// Local workspace source section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSourceSection {
    /// Path to the local source workspace, relative to the recipe file.
    pub path: PathBuf,
}

impl LocalSourceSection {
    fn validate_path(path: &Path) -> std::result::Result<(), String> {
        if path.as_os_str().is_empty() {
            return Err("[source] path cannot be empty".to_string());
        }
        if path.is_absolute() {
            return Err("[source] path must be relative to the recipe directory".to_string());
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("[source] path must stay within the recipe directory".to_string());
        }
        Ok(())
    }

    /// Resolve this local source path against the directory containing the recipe.
    pub fn resolve_against(&self, recipe_dir: &Path) -> std::result::Result<PathBuf, String> {
        Self::validate_path(&self.path)?;

        let mut resolved = recipe_dir.to_path_buf();
        for component in self.path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => resolved.push(part),
                Component::ParentDir => {
                    return Err("[source] path must stay within the recipe directory".to_string());
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(
                        "[source] path must be relative to the recipe directory".to_string()
                    );
                }
            }
        }

        Ok(resolved)
    }
}

/// Additional source archive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdditionalSource {
    /// Source URL
    pub url: String,
    /// Checksum
    pub checksum: String,
    /// Whether to extract the archive automatically after staging.
    #[serde(default = "default_true")]
    pub extract: bool,
    /// Where to extract (relative to main source)
    #[serde(default)]
    pub extract_to: Option<String>,
}

fn default_true() -> bool {
    true
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

    /// Manual stage assignment hint for bootstrap ordering.
    ///
    /// When set, overrides the automatic stage classification in the
    /// derivation stage assignment algorithm. Valid values: "toolchain",
    /// "foundation", "system", "customization".
    #[serde(default)]
    pub stage: Option<String>,
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

/// Check if a string is a remote URL (http:// or https://)
pub fn is_remote_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
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
mod tests;
