// src/ccs/policy.rs
//! Build policy system for CCS packages
//!
//! Provides automated quality enforcement during package builds through a
//! trait-based policy engine. Policies can validate, transform, or reject
//! files during the build process.

use crate::ccs::builder::FileEntry;
use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use thiserror::Error;

/// Policy errors
#[derive(Error, Debug)]
pub enum PolicyError {
    #[error("Policy violation: {policy} - {message}")]
    Violation { policy: String, message: String },

    #[error("Policy configuration error: {0}")]
    Config(String),

    #[error("Policy execution error: {0}")]
    Execution(String),
}

/// Result of applying a policy to a file
#[derive(Debug, Clone)]
pub enum PolicyAction {
    /// Keep file unchanged
    Keep,
    /// Replace file content with new bytes
    Replace(Vec<u8>),
    /// Skip this file (remove from package)
    Skip,
    /// Reject the build entirely with error message
    Reject(String),
}

/// Context provided to policies during application
pub struct PolicyContext<'a> {
    /// Source path of the file on disk
    pub source_path: &'a Path,
    /// File entry metadata
    pub entry: &'a FileEntry,
    /// File content bytes
    pub content: &'a [u8],
    /// All policies configuration
    pub config: &'a BuildPolicyConfig,
}

/// A build policy that can validate or transform files
pub trait BuildPolicy: Send + Sync {
    /// Policy name for logging and error messages
    fn name(&self) -> &str;

    /// Apply this policy to a file
    ///
    /// Returns a `PolicyAction` indicating what to do with the file:
    /// - `Keep`: Leave unchanged
    /// - `Replace(bytes)`: Replace content with new bytes
    /// - `Skip`: Remove from package
    /// - `Reject(msg)`: Fail the entire build
    fn apply(&self, ctx: &PolicyContext) -> Result<PolicyAction>;
}

/// Policy chain - applies policies in sequence
pub struct PolicyChain {
    policies: Vec<Box<dyn BuildPolicy>>,
}

impl PolicyChain {
    /// Create an empty policy chain
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    /// Create a policy chain from configuration
    pub fn from_config(config: &BuildPolicyConfig) -> Result<Self> {
        let mut chain = Self::new();

        // Add DenyPaths if configured
        if !config.reject_paths.is_empty() {
            chain.add(Box::new(DenyPathsPolicy::new(&config.reject_paths)?));
        }

        // Add NormalizeTimestamps if configured
        if config.normalize_timestamps {
            chain.add(Box::new(NormalizeTimestampsPolicy::new()));
        }

        // Add StripBinaries if configured
        if config.strip_binaries {
            chain.add(Box::new(StripBinariesPolicy::new()));
        }

        // Add FixShebangs if configured
        if !config.fix_shebangs.is_empty() {
            chain.add(Box::new(FixShebangsPolicy::new(config.fix_shebangs.clone())));
        }

        // Add CompressManpages if configured
        if config.compress_manpages {
            chain.add(Box::new(CompressManpagesPolicy::new()));
        }

        Ok(chain)
    }

    /// Add a policy to the chain
    pub fn add(&mut self, policy: Box<dyn BuildPolicy>) {
        self.policies.push(policy);
    }

    /// Check if the chain is empty
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// Apply all policies to a file entry and content
    ///
    /// Returns the final action and potentially modified content
    pub fn apply(
        &self,
        entry: &mut FileEntry,
        content: Vec<u8>,
        source_path: &Path,
        config: &BuildPolicyConfig,
    ) -> Result<(PolicyAction, Vec<u8>)> {
        let mut current_content = content;

        for policy in &self.policies {
            let ctx = PolicyContext {
                source_path,
                entry,
                content: &current_content,
                config,
            };

            match policy.apply(&ctx)? {
                PolicyAction::Keep => {
                    // Continue to next policy
                }
                PolicyAction::Replace(new_content) => {
                    current_content = new_content;
                    // Continue to next policy with new content
                }
                PolicyAction::Skip => {
                    return Ok((PolicyAction::Skip, current_content));
                }
                PolicyAction::Reject(msg) => {
                    return Err(PolicyError::Violation {
                        policy: policy.name().to_string(),
                        message: msg,
                    }
                    .into());
                }
            }
        }

        // If content was modified, signal that
        Ok((PolicyAction::Keep, current_content))
    }
}

impl Default for PolicyChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Build policy configuration from ccs.toml
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildPolicyConfig {
    /// Paths to reject (glob patterns)
    #[serde(default)]
    pub reject_paths: Vec<String>,

    /// Whether to strip ELF binaries
    #[serde(default)]
    pub strip_binaries: bool,

    /// Shebang replacements (old -> new)
    #[serde(default)]
    pub fix_shebangs: HashMap<String, String>,

    /// Whether to normalize file timestamps
    #[serde(default)]
    pub normalize_timestamps: bool,

    /// Whether to compress man pages
    #[serde(default)]
    pub compress_manpages: bool,
}

// =============================================================================
// Policy Implementations
// =============================================================================

/// Policy to reject files matching forbidden path patterns
pub struct DenyPathsPolicy {
    patterns: Vec<Pattern>,
    pattern_strings: Vec<String>,
}

impl DenyPathsPolicy {
    pub fn new(patterns: &[String]) -> Result<Self> {
        let mut compiled = Vec::new();
        for pat in patterns {
            let pattern = Pattern::new(pat)
                .map_err(|e| PolicyError::Config(format!("Invalid glob pattern '{}': {}", pat, e)))?;
            compiled.push(pattern);
        }
        Ok(Self {
            patterns: compiled,
            pattern_strings: patterns.to_vec(),
        })
    }
}

impl BuildPolicy for DenyPathsPolicy {
    fn name(&self) -> &str {
        "DenyPaths"
    }

    fn apply(&self, ctx: &PolicyContext) -> Result<PolicyAction> {
        for (pattern, pattern_str) in self.patterns.iter().zip(self.pattern_strings.iter()) {
            if pattern.matches(&ctx.entry.path) {
                return Ok(PolicyAction::Reject(format!(
                    "path '{}' matches reject pattern '{}'",
                    ctx.entry.path, pattern_str
                )));
            }
        }
        Ok(PolicyAction::Keep)
    }
}

/// Policy to normalize file timestamps for reproducible builds
pub struct NormalizeTimestampsPolicy {
    /// The timestamp to use (Unix epoch seconds)
    timestamp: u64,
}

impl NormalizeTimestampsPolicy {
    pub fn new() -> Self {
        // Check SOURCE_DATE_EPOCH environment variable (standard for reproducible builds)
        let timestamp = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1704067200); // 2024-01-01 00:00:00 UTC

        Self { timestamp }
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }
}

impl Default for NormalizeTimestampsPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildPolicy for NormalizeTimestampsPolicy {
    fn name(&self) -> &str {
        "NormalizeTimestamps"
    }

    fn apply(&self, _ctx: &PolicyContext) -> Result<PolicyAction> {
        // Timestamp normalization doesn't modify file content
        // It's applied at the tar archive level when writing the package
        // We just return Keep here and the builder handles mtime normalization
        Ok(PolicyAction::Keep)
    }
}

/// Policy to strip debug symbols from ELF binaries
pub struct StripBinariesPolicy;

impl StripBinariesPolicy {
    pub fn new() -> Self {
        Self
    }

    /// Check if content is an ELF binary
    fn is_elf(content: &[u8]) -> bool {
        content.len() >= 4 && &content[0..4] == b"\x7fELF"
    }

    /// Check if file is executable
    fn is_executable(entry: &FileEntry) -> bool {
        entry.mode & 0o111 != 0
    }
}

impl Default for StripBinariesPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildPolicy for StripBinariesPolicy {
    fn name(&self) -> &str {
        "StripBinaries"
    }

    fn apply(&self, ctx: &PolicyContext) -> Result<PolicyAction> {
        // Only process ELF files that are executable or shared libraries
        if !Self::is_elf(ctx.content) {
            return Ok(PolicyAction::Keep);
        }

        // Skip if not executable and not a .so file
        if !Self::is_executable(ctx.entry) && !ctx.entry.path.contains(".so") {
            return Ok(PolicyAction::Keep);
        }

        // Try to strip the binary
        // We need to write to a temp file, run strip, and read back
        let temp_file = tempfile::NamedTempFile::new()
            .map_err(|e| PolicyError::Execution(format!("Failed to create temp file: {}", e)))?;

        std::fs::write(temp_file.path(), ctx.content)
            .map_err(|e| PolicyError::Execution(format!("Failed to write temp file: {}", e)))?;

        let output = Command::new("strip")
            .arg("--strip-unneeded")
            .arg(temp_file.path())
            .output();

        match output {
            Ok(result) if result.status.success() => {
                // Read stripped binary
                let stripped = std::fs::read(temp_file.path()).map_err(|e| {
                    PolicyError::Execution(format!("Failed to read stripped binary: {}", e))
                })?;
                Ok(PolicyAction::Replace(stripped))
            }
            Ok(result) => {
                // Strip failed, but don't reject - just keep original
                // This can happen for static libraries or other edge cases
                let stderr = String::from_utf8_lossy(&result.stderr);
                log::debug!("strip failed for {}: {}", ctx.entry.path, stderr);
                Ok(PolicyAction::Keep)
            }
            Err(e) => {
                // strip command not available - keep original
                log::debug!("strip command failed: {}", e);
                Ok(PolicyAction::Keep)
            }
        }
    }
}

/// Policy to normalize shebangs in scripts
pub struct FixShebangsPolicy {
    replacements: HashMap<String, String>,
}

impl FixShebangsPolicy {
    pub fn new(replacements: HashMap<String, String>) -> Self {
        Self { replacements }
    }

    /// Check if content looks like a script (starts with #!)
    fn is_script(content: &[u8]) -> bool {
        content.len() >= 2 && &content[0..2] == b"#!"
    }

    /// Extract shebang line from content
    fn extract_shebang(content: &[u8]) -> Option<&[u8]> {
        if !Self::is_script(content) {
            return None;
        }
        let end = content.iter().position(|&b| b == b'\n').unwrap_or(content.len());
        Some(&content[0..end])
    }
}

impl BuildPolicy for FixShebangsPolicy {
    fn name(&self) -> &str {
        "FixShebangs"
    }

    fn apply(&self, ctx: &PolicyContext) -> Result<PolicyAction> {
        let Some(shebang_bytes) = Self::extract_shebang(ctx.content) else {
            return Ok(PolicyAction::Keep);
        };

        let Ok(shebang) = std::str::from_utf8(shebang_bytes) else {
            return Ok(PolicyAction::Keep);
        };

        // Check each replacement pattern
        for (old, new) in &self.replacements {
            // Check if shebang contains the old pattern
            if shebang.contains(old) {
                let new_shebang = shebang.replace(old, new);
                let mut new_content = new_shebang.into_bytes();
                new_content.extend_from_slice(&ctx.content[shebang_bytes.len()..]);
                return Ok(PolicyAction::Replace(new_content));
            }
        }

        Ok(PolicyAction::Keep)
    }
}

/// Policy to compress man pages
pub struct CompressManpagesPolicy;

impl CompressManpagesPolicy {
    pub fn new() -> Self {
        Self
    }

    /// Check if path looks like a man page
    fn is_manpage(path: &str) -> bool {
        // Man pages are typically in /usr/share/man/manN/ with extensions like .1, .2, etc.
        if !path.contains("/man/") && !path.contains("/man1/") && !path.contains("/man2/")
            && !path.contains("/man3/") && !path.contains("/man4/") && !path.contains("/man5/")
            && !path.contains("/man6/") && !path.contains("/man7/") && !path.contains("/man8/")
        {
            return false;
        }

        // Check for man page extensions
        let filename = path.rsplit('/').next().unwrap_or("");
        for ext in ["1", "2", "3", "4", "5", "6", "7", "8", "n", "l"] {
            if filename.ends_with(&format!(".{}", ext)) {
                return true;
            }
        }

        false
    }

    /// Check if content is already gzipped
    fn is_gzipped(content: &[u8]) -> bool {
        content.len() >= 2 && content[0] == 0x1f && content[1] == 0x8b
    }
}

impl Default for CompressManpagesPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildPolicy for CompressManpagesPolicy {
    fn name(&self) -> &str {
        "CompressManpages"
    }

    fn apply(&self, ctx: &PolicyContext) -> Result<PolicyAction> {
        // Only process man pages
        if !Self::is_manpage(&ctx.entry.path) {
            return Ok(PolicyAction::Keep);
        }

        // Skip if already compressed
        if Self::is_gzipped(ctx.content) {
            return Ok(PolicyAction::Keep);
        }

        // Compress with gzip
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(ctx.content)
            .map_err(|e| PolicyError::Execution(format!("Failed to compress: {}", e)))?;
        let compressed = encoder
            .finish()
            .map_err(|e| PolicyError::Execution(format!("Failed to finish compression: {}", e)))?;

        Ok(PolicyAction::Replace(compressed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::FileType;

    fn make_entry(path: &str, mode: u32) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            hash: String::new(),
            size: 0,
            mode,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }
    }

    #[test]
    fn test_deny_paths_policy() {
        let policy = DenyPathsPolicy::new(&["/home/*".to_string(), "/tmp/build*".to_string()]).unwrap();

        // Should reject /home/user/file
        let entry = make_entry("/home/user/file", 0o644);
        let ctx = PolicyContext {
            source_path: Path::new("/src/home/user/file"),
            entry: &entry,
            content: b"test",
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        assert!(matches!(result, PolicyAction::Reject(_)));

        // Should reject /tmp/build123
        let entry = make_entry("/tmp/build123", 0o644);
        let ctx = PolicyContext {
            source_path: Path::new("/src/tmp/build123"),
            entry: &entry,
            content: b"test",
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        assert!(matches!(result, PolicyAction::Reject(_)));

        // Should allow /usr/bin/myapp
        let entry = make_entry("/usr/bin/myapp", 0o755);
        let ctx = PolicyContext {
            source_path: Path::new("/src/usr/bin/myapp"),
            entry: &entry,
            content: b"test",
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        assert!(matches!(result, PolicyAction::Keep));
    }

    #[test]
    fn test_fix_shebangs_policy() {
        let mut replacements = HashMap::new();
        replacements.insert("/usr/bin/env python".to_string(), "/usr/bin/python3".to_string());
        let policy = FixShebangsPolicy::new(replacements);

        // Should fix python shebang
        let entry = make_entry("/usr/bin/script.py", 0o755);
        let content = b"#!/usr/bin/env python\nprint('hello')";
        let ctx = PolicyContext {
            source_path: Path::new("/src/usr/bin/script.py"),
            entry: &entry,
            content,
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        if let PolicyAction::Replace(new_content) = result {
            assert!(new_content.starts_with(b"#!/usr/bin/python3"));
        } else {
            panic!("Expected Replace action");
        }

        // Should keep bash shebang unchanged
        let content = b"#!/bin/bash\necho hello";
        let ctx = PolicyContext {
            source_path: Path::new("/src/usr/bin/script.sh"),
            entry: &entry,
            content,
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        assert!(matches!(result, PolicyAction::Keep));
    }

    #[test]
    fn test_compress_manpages_policy() {
        let policy = CompressManpagesPolicy::new();

        // Should compress man page
        let entry = make_entry("/usr/share/man/man1/myapp.1", 0o644);
        let content = b".TH MYAPP 1\n.SH NAME\nmyapp - test application";
        let ctx = PolicyContext {
            source_path: Path::new("/src/usr/share/man/man1/myapp.1"),
            entry: &entry,
            content,
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        if let PolicyAction::Replace(new_content) = result {
            // Should be gzipped (magic bytes)
            assert_eq!(new_content[0], 0x1f);
            assert_eq!(new_content[1], 0x8b);
        } else {
            panic!("Expected Replace action");
        }

        // Should skip non-man files
        let entry = make_entry("/usr/bin/myapp", 0o755);
        let ctx = PolicyContext {
            source_path: Path::new("/src/usr/bin/myapp"),
            entry: &entry,
            content: b"test",
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        assert!(matches!(result, PolicyAction::Keep));
    }

    #[test]
    fn test_normalize_timestamps_policy() {
        // Test with SOURCE_DATE_EPOCH
        // SAFETY: We're in a single-threaded test context
        unsafe { std::env::set_var("SOURCE_DATE_EPOCH", "1700000000") };
        let policy = NormalizeTimestampsPolicy::new();
        assert_eq!(policy.timestamp(), 1700000000);
        // SAFETY: We're in a single-threaded test context
        unsafe { std::env::remove_var("SOURCE_DATE_EPOCH") };

        // Test without SOURCE_DATE_EPOCH (should use default)
        let policy = NormalizeTimestampsPolicy::new();
        assert_eq!(policy.timestamp(), 1704067200); // 2024-01-01
    }

    #[test]
    fn test_policy_chain() {
        let config = BuildPolicyConfig {
            reject_paths: vec!["/home/*".to_string()],
            strip_binaries: false,
            fix_shebangs: HashMap::new(),
            normalize_timestamps: false,
            compress_manpages: false,
        };

        let chain = PolicyChain::from_config(&config).unwrap();
        assert!(!chain.is_empty());
    }

    #[test]
    fn test_empty_policy_chain() {
        let config = BuildPolicyConfig::default();
        let chain = PolicyChain::from_config(&config).unwrap();
        assert!(chain.is_empty());
    }
}
