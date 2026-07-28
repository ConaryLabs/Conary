// conary-core/src/ccs/policy.rs
//! Build-policy orchestration for CCS authoring.
//!
//! Metadata policies mutate typed file authority in place. Content policies
//! write replacements into an authoring workspace and return a new path, so
//! the builder never needs a whole-file byte vector.

use crate::ccs::builder::FileEntry;
use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

mod content;

pub use content::{CompressManpagesPolicy, FixShebangsPolicy, StripBinariesPolicy};

/// Policy errors.
#[derive(Error, Debug)]
pub enum PolicyError {
    #[error("Policy violation: {policy} - {message}")]
    Violation { policy: String, message: String },

    #[error("Policy configuration error: {0}")]
    Config(String),

    #[error("Policy execution error: {0}")]
    Execution(String),
}

/// Result of applying a policy to one source-backed entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAction {
    /// Keep the current source unchanged.
    Keep,
    /// Replace the current source with this disk-backed path.
    Replace(PathBuf),
    /// Skip this file.
    Skip,
    /// Reject the build.
    Reject(String),
}

/// Context provided to policies during application.
pub struct PolicyContext<'a> {
    /// Current source path. This may be an earlier policy's replacement.
    pub source_path: &'a Path,
    /// Unique destination reserved for this policy invocation.
    pub replacement_path: &'a Path,
    /// File-entry metadata.
    pub entry: &'a mut FileEntry,
    /// All policy configuration.
    pub config: &'a BuildPolicyConfig,
}

/// A build policy that can validate, transform, or reject one entry.
pub trait BuildPolicy: Send + Sync {
    fn name(&self) -> &str;

    /// Apply the policy without materializing the complete content in memory.
    fn apply(&self, ctx: &mut PolicyContext<'_>) -> Result<PolicyAction>;
}

/// Ordered policy chain.
pub struct PolicyChain {
    policies: Vec<Box<dyn BuildPolicy>>,
}

impl PolicyChain {
    #[must_use]
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    pub fn from_config(config: &BuildPolicyConfig) -> std::result::Result<Self, PolicyError> {
        let mut chain = Self::new();
        if !config.reject_paths.is_empty() {
            chain.add(Box::new(DenyPathsPolicy::new(&config.reject_paths)?));
        }
        chain.add(Box::new(StripSetuidPolicy::new()));
        if config.normalize_timestamps {
            chain.add(Box::new(NormalizeTimestampsPolicy::new()));
        }
        if config.strip_binaries {
            chain.add(Box::new(StripBinariesPolicy::new()));
        }
        if !config.fix_shebangs.is_empty() {
            if config.fix_shebangs.keys().any(String::is_empty) {
                return Err(PolicyError::Config(
                    "fix_shebangs patterns must not be empty".to_string(),
                ));
            }
            chain.add(Box::new(FixShebangsPolicy::new(
                config.fix_shebangs.clone(),
            )));
        }
        if config.compress_manpages {
            chain.add(Box::new(CompressManpagesPolicy::new()));
        }
        Ok(chain)
    }

    pub fn add(&mut self, policy: Box<dyn BuildPolicy>) {
        self.policies.push(policy);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// Apply all policies and return the final source path when replaced.
    pub fn apply(
        &self,
        entry: &mut FileEntry,
        source_path: &Path,
        workspace: &Path,
        config: &BuildPolicyConfig,
    ) -> Result<PolicyAction> {
        let mut current_source = source_path.to_path_buf();
        let mut replaced = false;

        for (index, policy) in self.policies.iter().enumerate() {
            let replacement_path = workspace.join(format!("{index:04}"));
            let mut ctx = PolicyContext {
                source_path: &current_source,
                replacement_path: &replacement_path,
                entry,
                config,
            };
            match policy.apply(&mut ctx)? {
                PolicyAction::Keep => {}
                PolicyAction::Replace(path) => {
                    current_source = path;
                    replaced = true;
                }
                PolicyAction::Skip => return Ok(PolicyAction::Skip),
                PolicyAction::Reject(message) => {
                    return Err(PolicyError::Violation {
                        policy: policy.name().to_string(),
                        message,
                    }
                    .into());
                }
            }
        }

        Ok(if replaced {
            PolicyAction::Replace(current_source)
        } else {
            PolicyAction::Keep
        })
    }
}

impl Default for PolicyChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Build-policy configuration from `ccs.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildPolicyConfig {
    #[serde(default)]
    pub reject_paths: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_setuid_paths: Vec<String>,

    #[serde(default)]
    pub strip_binaries: bool,

    #[serde(default)]
    pub fix_shebangs: HashMap<String, String>,

    #[serde(default)]
    pub normalize_timestamps: bool,

    #[serde(default)]
    pub compress_manpages: bool,
}

/// Reject paths selected by exact author configuration.
pub struct DenyPathsPolicy {
    patterns: Vec<Pattern>,
    pattern_strings: Vec<String>,
}

impl DenyPathsPolicy {
    pub fn new(patterns: &[String]) -> std::result::Result<Self, PolicyError> {
        let patterns_compiled = patterns
            .iter()
            .map(|pattern| {
                Pattern::new(pattern).map_err(|error| {
                    PolicyError::Config(format!("Invalid glob pattern '{pattern}': {error}"))
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self {
            patterns: patterns_compiled,
            pattern_strings: patterns.to_vec(),
        })
    }
}

impl BuildPolicy for DenyPathsPolicy {
    fn name(&self) -> &str {
        "DenyPaths"
    }

    fn apply(&self, ctx: &mut PolicyContext<'_>) -> Result<PolicyAction> {
        for (pattern, pattern_text) in self.patterns.iter().zip(&self.pattern_strings) {
            if pattern.matches(&ctx.entry.path) {
                return Ok(PolicyAction::Reject(format!(
                    "path '{}' matches reject pattern '{pattern_text}'",
                    ctx.entry.path
                )));
            }
        }
        Ok(PolicyAction::Keep)
    }
}

/// Reproducible timestamp policy marker.
pub struct NormalizeTimestampsPolicy {
    timestamp: u64,
}

impl NormalizeTimestampsPolicy {
    #[must_use]
    pub fn new() -> Self {
        let timestamp = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_704_067_200);
        Self { timestamp }
    }

    #[must_use]
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

    fn apply(&self, _ctx: &mut PolicyContext<'_>) -> Result<PolicyAction> {
        Ok(PolicyAction::Keep)
    }
}

/// Always remove setgid and remove setuid unless explicitly allowed.
#[derive(Default)]
pub struct StripSetuidPolicy;

impl StripSetuidPolicy {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl BuildPolicy for StripSetuidPolicy {
    fn name(&self) -> &str {
        "StripSetuid"
    }

    fn apply(&self, ctx: &mut PolicyContext<'_>) -> Result<PolicyAction> {
        let allow_setuid = ctx
            .config
            .allow_setuid_paths
            .iter()
            .any(|path| path == &ctx.entry.path);
        if allow_setuid {
            ctx.entry.node.mode &= !0o2000;
        } else {
            ctx.entry.node.mode &= !0o6000;
        }
        Ok(PolicyAction::Keep)
    }
}

#[cfg(test)]
#[path = "policy/tests.rs"]
mod tests;
