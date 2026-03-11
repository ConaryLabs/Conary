// conary-core/src/bootstrap/toolchain.rs

//! Toolchain representation and management
//!
//! A toolchain is a set of tools (compiler, linker, libraries) that can be
//! used to build software. This module provides a type-safe way to work with
//! toolchains from different bootstrap stages.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Kind of toolchain
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolchainKind {
    /// Host system toolchain
    Host,
    /// Stage 0: Cross-compiler from crosstool-ng
    Stage0,
    /// Stage 1: Self-hosted toolchain
    Stage1,
    /// Stage 2: Pure rebuild toolchain
    Stage2,
}

impl ToolchainKind {
    /// Get a human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Stage0 => "stage0",
            Self::Stage1 => "stage1",
            Self::Stage2 => "stage2",
        }
    }
}

impl std::fmt::Display for ToolchainKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A toolchain for building software
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Toolchain {
    /// Kind of toolchain
    pub kind: ToolchainKind,

    /// Prefix path (e.g., /tools)
    pub path: PathBuf,

    /// Target triple (e.g., x86_64-conary-linux-gnu)
    pub target: String,

    /// GCC version
    pub gcc_version: Option<String>,

    /// glibc version
    pub glibc_version: Option<String>,

    /// binutils version
    pub binutils_version: Option<String>,

    /// Whether the toolchain is static (no shared lib deps)
    pub is_static: bool,
}

impl Toolchain {
    /// Parse version from tool's `--version` output.
    /// Extracts the first semver-like pattern (X.Y.Z or X.Y).
    pub fn parse_version_output(output: &str) -> Option<String> {
        // Use a simple manual parser (no regex dependency needed):
        // Find first digit sequence that matches X.Y or X.Y.Z
        let bytes = output.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i].is_ascii_digit() {
                let start = i;
                // Eat digits
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // Need a dot
                if i < len && bytes[i] == b'.' {
                    i += 1;
                    // Need more digits (minor)
                    if i < len && bytes[i].is_ascii_digit() {
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        // Optional .patch
                        if i < len && bytes[i] == b'.' {
                            let dot_pos = i;
                            i += 1;
                            if i < len && bytes[i].is_ascii_digit() {
                                while i < len && bytes[i].is_ascii_digit() {
                                    i += 1;
                                }
                                return Some(output[start..i].to_string());
                            }
                            return Some(output[start..dot_pos].to_string());
                        }
                        return Some(output[start..i].to_string());
                    }
                }
            }
            i += 1;
        }
        None
    }

    /// Detect glibc version by running the toolchain's ldd --version.
    pub fn detect_glibc_version(&mut self) {
        let ldd = self.tool("ldd");
        if let Ok(output) = std::process::Command::new(&ldd).arg("--version").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}{stderr}");
            self.glibc_version = Self::parse_version_output(&combined);
        }
    }

    /// Detect binutils version by running ld --version.
    pub fn detect_binutils_version(&mut self) {
        let ld = self.tool("ld");
        if let Ok(output) = std::process::Command::new(&ld).arg("--version").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            self.binutils_version = Self::parse_version_output(&stdout);
        }
    }

    /// Create a toolchain from an existing prefix
    pub fn from_prefix(prefix: impl AsRef<Path>) -> Result<Self> {
        let prefix = prefix.as_ref().to_path_buf();

        // Find the target by looking at what's in bin/
        let bin_dir = prefix.join("bin");
        if !bin_dir.exists() {
            bail!("Toolchain bin directory not found: {}", bin_dir.display());
        }

        // Look for *-gcc to determine target
        let target = Self::detect_target(&bin_dir)?;

        // Get versions
        let gcc_path = bin_dir.join(format!("{}-gcc", target));
        let gcc_version = Self::get_gcc_version(&gcc_path).ok();

        // Check if static
        let is_static = Self::check_static(&gcc_path);

        let mut toolchain = Self {
            kind: ToolchainKind::Stage0, // Assume Stage0, caller can override
            path: prefix,
            target,
            gcc_version,
            glibc_version: None,
            binutils_version: None,
            is_static,
        };

        toolchain.detect_glibc_version();
        toolchain.detect_binutils_version();

        Ok(toolchain)
    }

    /// Create a toolchain representing the host system
    pub fn host() -> Result<Self> {
        // Find host gcc
        let gcc_path = which::which("gcc").context("gcc not found in PATH")?;
        let gcc_version = Self::get_gcc_version(&gcc_path).ok();

        // Get host target
        let output = Command::new(&gcc_path)
            .arg("-dumpmachine")
            .output()
            .context("Failed to run gcc -dumpmachine")?;

        let target = String::from_utf8_lossy(&output.stdout).trim().to_string();

        Ok(Self {
            kind: ToolchainKind::Host,
            path: PathBuf::from("/usr"),
            target,
            gcc_version,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        })
    }

    /// Detect target from bin directory
    fn detect_target(bin_dir: &Path) -> Result<String> {
        for entry in std::fs::read_dir(bin_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Look for *-gcc pattern
            if name_str.ends_with("-gcc") && !name_str.contains("..") {
                return Ok(name_str.trim_end_matches("-gcc").to_string());
            }
        }

        bail!("Could not detect target from {}", bin_dir.display());
    }

    /// Get GCC version
    fn get_gcc_version(gcc_path: &Path) -> Result<String> {
        let output = Command::new(gcc_path)
            .arg("--version")
            .output()
            .context("Failed to run gcc --version")?;

        let version_line = String::from_utf8_lossy(&output.stdout);
        // Parse "gcc (GCC) X.Y.Z" or similar
        if let Some(line) = version_line.lines().next() {
            // Extract version number
            for part in line.split_whitespace() {
                if part.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Ok(part.to_string());
                }
            }
        }

        bail!("Could not parse GCC version from: {}", version_line);
    }

    /// Check if a binary is statically linked
    fn check_static(path: &Path) -> bool {
        if !path.exists() {
            return false;
        }

        Command::new("ldd")
            .arg(path)
            .output()
            .ok()
            .map(|o| {
                let output = String::from_utf8_lossy(&o.stdout);
                output.contains("not a dynamic executable") || output.contains("statically linked")
            })
            .unwrap_or(false)
    }

    /// Get path to the bin directory
    pub fn bin_dir(&self) -> PathBuf {
        self.path.join("bin")
    }

    /// Get path to a tool (e.g., "gcc" -> "/tools/bin/x86_64-conary-linux-gnu-gcc")
    pub fn tool(&self, name: &str) -> PathBuf {
        if self.kind == ToolchainKind::Host {
            // Host tools don't have prefix
            self.bin_dir().join(name)
        } else {
            self.bin_dir().join(format!("{}-{}", self.target, name))
        }
    }

    /// Get path to GCC
    pub fn gcc(&self) -> PathBuf {
        self.tool("gcc")
    }

    /// Get path to G++
    pub fn gxx(&self) -> PathBuf {
        self.tool("g++")
    }

    /// Get path to ar
    pub fn ar(&self) -> PathBuf {
        self.tool("ar")
    }

    /// Get path to ld
    pub fn ld(&self) -> PathBuf {
        self.tool("ld")
    }

    /// Get path to ranlib
    pub fn ranlib(&self) -> PathBuf {
        self.tool("ranlib")
    }

    /// Get path to strip
    pub fn strip(&self) -> PathBuf {
        self.tool("strip")
    }

    /// Minimal safe PATH for bootstrap builds.
    ///
    /// Deliberately excludes the host's PATH to prevent accidental use of host
    /// tools that could silently contaminate the hermetic build environment.
    /// Only standard system directories known to be safe are included.
    pub const BOOTSTRAP_PATH_FALLBACK: &'static str =
        "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin";

    /// Get environment variables for using this toolchain
    pub fn env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Sanitize PATH: do NOT inherit the host PATH blindly.
        // Prepend the toolchain bin dir to a minimal, fixed fallback so that
        // host-only tools (e.g. a user-installed rustup shim or custom gcc
        // wrapper) cannot leak into bootstrap builds and produce unreproducible
        // results.  Callers that genuinely need the host PATH (e.g. Toolchain::host()
        // for development use) can override this after calling env().
        env.insert(
            "PATH".to_string(),
            format!(
                "{}:{}",
                self.bin_dir().display(),
                Self::BOOTSTRAP_PATH_FALLBACK,
            ),
        );

        if self.kind != ToolchainKind::Host {
            // Set CC, CXX, etc. for cross-compilation
            env.insert("CC".to_string(), self.gcc().display().to_string());
            env.insert("CXX".to_string(), self.gxx().display().to_string());
            env.insert("AR".to_string(), self.ar().display().to_string());
            env.insert("LD".to_string(), self.ld().display().to_string());
            env.insert("RANLIB".to_string(), self.ranlib().display().to_string());
            env.insert("STRIP".to_string(), self.strip().display().to_string());

            // Target for configure scripts
            env.insert("TARGET".to_string(), self.target.clone());
            env.insert("HOST".to_string(), self.target.clone());
        }

        env
    }

    /// Run a command using this toolchain
    pub fn run(&self, program: &str, args: &[&str]) -> Result<std::process::Output> {
        let mut cmd = Command::new(program);
        cmd.args(args);

        // Set environment
        for (key, value) in self.env() {
            cmd.env(key, value);
        }

        cmd.output()
            .with_context(|| format!("Failed to run {} {:?}", program, args))
    }

    /// Verify the toolchain is functional
    pub fn verify(&self) -> Result<()> {
        // Check GCC exists
        let gcc = self.gcc();
        if !gcc.exists() {
            bail!("GCC not found at {}", gcc.display());
        }

        // Check it runs
        let output = Command::new(&gcc)
            .arg("--version")
            .output()
            .context("Failed to run GCC")?;

        if !output.status.success() {
            bail!("GCC failed: {}", String::from_utf8_lossy(&output.stderr));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolchain_kind_name() {
        assert_eq!(ToolchainKind::Host.name(), "host");
        assert_eq!(ToolchainKind::Stage0.name(), "stage0");
        assert_eq!(ToolchainKind::Stage1.name(), "stage1");
    }

    #[test]
    fn test_host_toolchain() {
        // This should work on any system with gcc
        if which::which("gcc").is_ok() {
            let toolchain = Toolchain::host().unwrap();
            assert_eq!(toolchain.kind, ToolchainKind::Host);
            assert!(!toolchain.target.is_empty());
        }
    }

    #[test]
    fn test_toolchain_tool_paths() {
        let toolchain = Toolchain {
            kind: ToolchainKind::Stage0,
            path: PathBuf::from("/tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: Some("13.3.0".to_string()),
            glibc_version: None,
            binutils_version: None,
            is_static: true,
        };

        assert_eq!(
            toolchain.gcc(),
            PathBuf::from("/tools/bin/x86_64-conary-linux-gnu-gcc")
        );
        assert_eq!(
            toolchain.gxx(),
            PathBuf::from("/tools/bin/x86_64-conary-linux-gnu-g++")
        );
        assert_eq!(
            toolchain.ar(),
            PathBuf::from("/tools/bin/x86_64-conary-linux-gnu-ar")
        );
    }

    #[test]
    fn test_host_toolchain_paths() {
        let toolchain = Toolchain {
            kind: ToolchainKind::Host,
            path: PathBuf::from("/usr"),
            target: "x86_64-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        // Host tools don't have target prefix
        assert_eq!(toolchain.gcc(), PathBuf::from("/usr/bin/gcc"));
        assert_eq!(toolchain.gxx(), PathBuf::from("/usr/bin/g++"));
    }

    #[test]
    fn test_parse_gcc_version() {
        let output = "gcc (GCC) 15.2.0\nCopyright (C) 2025 Free Software Foundation, Inc.";
        assert_eq!(
            Toolchain::parse_version_output(output),
            Some("15.2.0".to_string())
        );
    }

    #[test]
    fn test_parse_glibc_version() {
        let output = "ldd (GNU libc) 2.42\nCopyright (C) 2025 Free Software Foundation, Inc.";
        assert_eq!(
            Toolchain::parse_version_output(output),
            Some("2.42".to_string())
        );
    }

    #[test]
    fn test_parse_binutils_version() {
        let output = "GNU ld (GNU Binutils) 2.45";
        assert_eq!(
            Toolchain::parse_version_output(output),
            Some("2.45".to_string())
        );
    }

    #[test]
    fn test_parse_version_empty() {
        assert_eq!(Toolchain::parse_version_output(""), None);
    }

    #[test]
    fn test_parse_version_no_version() {
        assert_eq!(Toolchain::parse_version_output("no version here"), None);
    }

    #[test]
    fn test_toolchain_env() {
        let toolchain = Toolchain {
            kind: ToolchainKind::Stage0,
            path: PathBuf::from("/tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: true,
        };

        let env = toolchain.env();

        assert!(env.get("PATH").unwrap().starts_with("/tools/bin:"));
        assert_eq!(
            env.get("CC").unwrap(),
            "/tools/bin/x86_64-conary-linux-gnu-gcc"
        );
        assert_eq!(
            env.get("CXX").unwrap(),
            "/tools/bin/x86_64-conary-linux-gnu-g++"
        );
    }
}
