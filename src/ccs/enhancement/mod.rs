// src/ccs/enhancement/mod.rs
//! Enhancement framework for converted packages
//!
//! This module provides the infrastructure for enhancing converted packages
//! with additional metadata that wasn't available in the original format:
//!
//! - **Capability inference**: Determine what system resources a package needs
//! - **Provenance extraction**: Extract build/source metadata from legacy formats
//! - **Subpackage relationships**: Track relationships between base packages and subpackages
//!
//! # Architecture
//!
//! The enhancement framework uses a trait-based plugin system:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    EnhancementRunner                         │
//! │  (orchestrates enhancement execution)                        │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   EnhancementRegistry                        │
//! │  (manages registered enhancers)                              │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!              ┌───────────────┼───────────────┐
//!              ▼               ▼               ▼
//! ┌──────────────────┐ ┌──────────────┐ ┌──────────────────┐
//! │CapabilityEnhancer│ │ProvenanceEnh │ │SubpackageEnhancer│
//! │                  │ │              │ │                  │
//! │(infers caps from │ │(extracts     │ │(detects base/    │
//! │ binaries/files)  │ │ provenance)  │ │ subpackage rels) │
//! └──────────────────┘ └──────────────┘ └──────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use conary::ccs::enhancement::{EnhancementRunner, EnhancementType};
//!
//! let runner = EnhancementRunner::new(&db)?;
//!
//! // Enhance a specific package
//! runner.enhance(trove_id, &[EnhancementType::Capabilities])?;
//!
//! // Enhance all pending packages
//! runner.enhance_all_pending()?;
//! ```

pub mod context;
pub mod error;
pub mod registry;
pub mod runner;

pub use context::EnhancementContext;
pub use error::{EnhancementError, EnhancementResult};
pub use registry::EnhancementRegistry;
pub use runner::EnhancementRunner;

use serde::{Deserialize, Serialize};

/// Current enhancement algorithm version
///
/// Bump this when the enhancement logic changes significantly enough
/// that we want to re-enhance previously enhanced packages.
pub const ENHANCEMENT_VERSION: i32 = 1;

/// Types of enhancements that can be applied to converted packages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnhancementType {
    /// Infer security capabilities from package contents
    Capabilities,
    /// Extract provenance information from legacy metadata
    Provenance,
    /// Detect and record subpackage relationships
    Subpackages,
}

impl EnhancementType {
    /// Get the string name of this enhancement type
    pub fn name(&self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Provenance => "provenance",
            Self::Subpackages => "subpackages",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "capabilities" | "caps" => Some(Self::Capabilities),
            "provenance" | "prov" => Some(Self::Provenance),
            "subpackages" | "subpkg" => Some(Self::Subpackages),
            _ => None,
        }
    }

    /// Get all enhancement types
    pub fn all() -> &'static [EnhancementType] {
        &[
            EnhancementType::Capabilities,
            EnhancementType::Provenance,
            EnhancementType::Subpackages,
        ]
    }
}

impl std::fmt::Display for EnhancementType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Status of an enhancement operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnhancementStatus {
    /// Enhancement is pending (not yet started)
    Pending,
    /// Enhancement is currently in progress
    InProgress,
    /// Enhancement completed successfully
    Complete,
    /// Enhancement failed
    Failed,
    /// Enhancement was skipped (e.g., nothing to enhance)
    Skipped,
}

impl EnhancementStatus {
    /// Parse from database string
    pub fn from_db_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "complete" => Self::Complete,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            _ => Self::Pending,
        }
    }

    /// Convert to database string
    pub fn to_db_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Result of enhancing a single package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancementResult_ {
    /// Trove ID that was enhanced
    pub trove_id: i64,
    /// Which enhancements were applied
    pub applied: Vec<EnhancementType>,
    /// Which enhancements were skipped
    pub skipped: Vec<EnhancementType>,
    /// Which enhancements failed (with error messages)
    pub failed: Vec<(EnhancementType, String)>,
    /// New enhancement version
    pub enhancement_version: i32,
}

impl EnhancementResult_ {
    /// Create a new result for a trove
    pub fn new(trove_id: i64) -> Self {
        Self {
            trove_id,
            applied: Vec::new(),
            skipped: Vec::new(),
            failed: Vec::new(),
            enhancement_version: ENHANCEMENT_VERSION,
        }
    }

    /// Check if all enhancements succeeded
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Record a successful enhancement
    pub fn record_success(&mut self, enhancement_type: EnhancementType) {
        self.applied.push(enhancement_type);
    }

    /// Record a skipped enhancement
    pub fn record_skipped(&mut self, enhancement_type: EnhancementType) {
        self.skipped.push(enhancement_type);
    }

    /// Record a failed enhancement
    pub fn record_failure(&mut self, enhancement_type: EnhancementType, error: impl Into<String>) {
        self.failed.push((enhancement_type, error.into()));
    }
}

/// Trait for enhancement plugins
///
/// Each enhancement type implements this trait to provide its specific
/// enhancement logic.
pub trait EnhancementEngine: Send + Sync {
    /// Get the type of enhancement this engine provides
    fn enhancement_type(&self) -> EnhancementType;

    /// Check if this enhancement should be applied to the given package
    ///
    /// This is called before `enhance()` to allow fast filtering of packages
    /// that don't need this enhancement.
    fn should_enhance(&self, ctx: &EnhancementContext) -> bool;

    /// Apply the enhancement to the package
    ///
    /// The enhancement should:
    /// 1. Read package data from the context
    /// 2. Perform inference/extraction
    /// 3. Store results via the context's database connection
    ///
    /// Returns `Ok(())` on success, or an error describing what went wrong.
    fn enhance(&self, ctx: &mut EnhancementContext) -> EnhancementResult<()>;

    /// Get a human-readable description of this enhancer
    fn description(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhancement_type_parsing() {
        assert_eq!(
            EnhancementType::from_str("capabilities"),
            Some(EnhancementType::Capabilities)
        );
        assert_eq!(
            EnhancementType::from_str("caps"),
            Some(EnhancementType::Capabilities)
        );
        assert_eq!(
            EnhancementType::from_str("provenance"),
            Some(EnhancementType::Provenance)
        );
        assert_eq!(
            EnhancementType::from_str("subpackages"),
            Some(EnhancementType::Subpackages)
        );
        assert_eq!(EnhancementType::from_str("unknown"), None);
    }

    #[test]
    fn test_enhancement_status_roundtrip() {
        for status in [
            EnhancementStatus::Pending,
            EnhancementStatus::InProgress,
            EnhancementStatus::Complete,
            EnhancementStatus::Failed,
            EnhancementStatus::Skipped,
        ] {
            let db_str = status.to_db_str();
            let parsed = EnhancementStatus::from_db_str(db_str);
            assert_eq!(status, parsed);
        }
    }

    #[test]
    fn test_enhancement_result() {
        let mut result = EnhancementResult_::new(42);
        assert!(result.is_success());

        result.record_success(EnhancementType::Capabilities);
        assert!(result.is_success());
        assert_eq!(result.applied.len(), 1);

        result.record_failure(EnhancementType::Provenance, "test error");
        assert!(!result.is_success());
        assert_eq!(result.failed.len(), 1);
    }
}
