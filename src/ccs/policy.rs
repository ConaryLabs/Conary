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

        // Try to strip the binary using native Rust implementation
        match strip_elf_binary(ctx.content) {
            Ok(stripped) => {
                // Only replace if we actually reduced size
                if stripped.len() < ctx.content.len() {
                    log::debug!(
                        "Stripped {}: {} -> {} bytes",
                        ctx.entry.path,
                        ctx.content.len(),
                        stripped.len()
                    );
                    Ok(PolicyAction::Replace(stripped))
                } else {
                    Ok(PolicyAction::Keep)
                }
            }
            Err(e) => {
                // Strip failed, but don't reject - just keep original
                log::debug!("strip failed for {}: {}", ctx.entry.path, e);
                Ok(PolicyAction::Keep)
            }
        }
    }
}

/// Strip debug symbols from an ELF binary using native Rust
///
/// This implementation removes section headers and debug sections by:
/// 1. Parsing the ELF structure with goblin
/// 2. Finding the end of the last loadable segment (PT_LOAD)
/// 3. Truncating the file to that point
/// 4. Updating the ELF header to indicate no section headers
///
/// This is equivalent to a basic `strip --strip-unneeded` for executables
/// and shared libraries. It preserves program headers needed for execution.
fn strip_elf_binary(content: &[u8]) -> std::result::Result<Vec<u8>, String> {
    use goblin::elf::{header::*, program_header::PT_LOAD, Elf};

    // Parse the ELF file
    let elf = Elf::parse(content).map_err(|e| format!("Failed to parse ELF: {}", e))?;

    // Only strip executables (ET_EXEC) and shared objects (ET_DYN)
    // Don't strip relocatable files (ET_REL) as they need section headers
    if elf.header.e_type != ET_EXEC && elf.header.e_type != ET_DYN {
        return Err("Not an executable or shared library".to_string());
    }

    // Find the end of the last loadable segment
    let mut last_segment_end: u64 = 0;
    for phdr in &elf.program_headers {
        if phdr.p_type == PT_LOAD {
            let segment_end = phdr.p_offset + phdr.p_filesz;
            if segment_end > last_segment_end {
                last_segment_end = segment_end;
            }
        }
    }

    // Also need to keep program headers
    let phdr_end = elf.header.e_phoff + (elf.header.e_phnum as u64 * elf.header.e_phentsize as u64);
    if phdr_end > last_segment_end {
        last_segment_end = phdr_end;
    }

    // Ensure we keep at least the ELF header
    let elf_header_size = elf.header.e_ehsize as u64;
    if last_segment_end < elf_header_size {
        last_segment_end = elf_header_size;
    }

    // Truncate the binary
    let truncate_at = last_segment_end as usize;
    if truncate_at >= content.len() {
        // Nothing to strip
        return Err("No sections to strip".to_string());
    }

    let mut stripped = content[..truncate_at].to_vec();

    // Zero out section header references in the ELF header
    // This tells the loader there are no section headers
    if elf.is_64 {
        // 64-bit ELF: e_shoff at offset 40 (8 bytes), e_shnum at 60 (2 bytes), e_shstrndx at 62 (2 bytes)
        if stripped.len() >= 64 {
            // e_shoff (8 bytes at offset 40)
            stripped[40..48].copy_from_slice(&0u64.to_le_bytes());
            // e_shnum (2 bytes at offset 60)
            stripped[60..62].copy_from_slice(&0u16.to_le_bytes());
            // e_shstrndx (2 bytes at offset 62)
            stripped[62..64].copy_from_slice(&0u16.to_le_bytes());
        }
    } else {
        // 32-bit ELF: e_shoff at offset 32 (4 bytes), e_shnum at 48 (2 bytes), e_shstrndx at 50 (2 bytes)
        if stripped.len() >= 52 {
            // e_shoff (4 bytes at offset 32)
            stripped[32..36].copy_from_slice(&0u32.to_le_bytes());
            // e_shnum (2 bytes at offset 48)
            stripped[48..50].copy_from_slice(&0u16.to_le_bytes());
            // e_shstrndx (2 bytes at offset 50)
            stripped[50..52].copy_from_slice(&0u16.to_le_bytes());
        }
    }

    Ok(stripped)
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

    #[test]
    fn test_strip_elf_binary_not_elf() {
        // Non-ELF content should fail
        let content = b"not an elf file";
        let result = strip_elf_binary(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_strip_elf_binary_real() {
        // Test with a real system binary (if available)
        // This test is skipped if the binary doesn't exist
        let binary_paths = ["/bin/true", "/usr/bin/true", "/bin/ls", "/usr/bin/ls"];

        let content = binary_paths
            .iter()
            .find_map(|path| std::fs::read(path).ok());

        let Some(content) = content else {
            // Skip test if no binary found
            return;
        };

        // Verify it's an ELF
        assert!(content.len() >= 4 && &content[0..4] == b"\x7fELF");

        let result = strip_elf_binary(&content);

        // Strip should either succeed (reducing size) or fail gracefully
        // Modern binaries are often already stripped, so we accept both outcomes
        match result {
            Ok(stripped) => {
                // If successful, verify the output is valid
                assert!(stripped.len() >= 64, "stripped binary should have ELF header");
                assert_eq!(&stripped[0..4], b"\x7fELF", "should still be ELF");

                // Section header offset should be zeroed
                let shoff = u64::from_le_bytes(stripped[40..48].try_into().unwrap());
                assert_eq!(shoff, 0, "shoff should be zeroed");
            }
            Err(e) => {
                // Acceptable errors: already stripped, nothing to strip
                assert!(
                    e.contains("No sections") || e.contains("Not an executable"),
                    "unexpected error: {}",
                    e
                );
            }
        }
    }

    #[test]
    fn test_strip_binaries_policy_non_elf() {
        let policy = StripBinariesPolicy::new();

        // Non-ELF file should be kept unchanged
        let entry = make_entry("/usr/bin/script", 0o755);
        let content = b"#!/bin/bash\necho hello";
        let ctx = PolicyContext {
            source_path: Path::new("/src/usr/bin/script"),
            entry: &entry,
            content,
            config: &BuildPolicyConfig::default(),
        };
        let result = policy.apply(&ctx).unwrap();
        assert!(matches!(result, PolicyAction::Keep));
    }
}
