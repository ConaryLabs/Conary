// src/ccs/enhancement/runner.rs
//! Enhancement runner that orchestrates enhancement execution

use super::context::{ConvertedPackageInfo, EnhancementContext, EnhancementStats};
use super::error::EnhancementResult;
use super::registry::EnhancementRegistry;
use super::{EnhancementResult_, EnhancementStatus, EnhancementType, ENHANCEMENT_VERSION};
use rusqlite::Connection;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Options for running enhancements
#[derive(Debug, Clone)]
pub struct EnhancementOptions {
    /// Enhancement types to apply (default: all)
    pub types: Vec<EnhancementType>,
    /// Force re-enhancement even if already done
    pub force: bool,
    /// Install root path (default: /)
    pub install_root: PathBuf,
    /// Stop on first error
    pub fail_fast: bool,
}

impl Default for EnhancementOptions {
    fn default() -> Self {
        Self {
            types: EnhancementType::all().to_vec(),
            force: false,
            install_root: PathBuf::from("/"),
            fail_fast: false,
        }
    }
}

impl EnhancementOptions {
    /// Create options for specific enhancement types
    pub fn with_types(types: Vec<EnhancementType>) -> Self {
        Self {
            types,
            ..Default::default()
        }
    }

    /// Enable force mode
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    /// Set install root
    pub fn install_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.install_root = path.into();
        self
    }
}

/// Enhancement runner that coordinates enhancement execution
pub struct EnhancementRunner<'a> {
    conn: &'a Connection,
    registry: EnhancementRegistry,
    options: EnhancementOptions,
}

impl<'a> EnhancementRunner<'a> {
    /// Create a new enhancement runner
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            registry: EnhancementRegistry::default(),
            options: EnhancementOptions::default(),
        }
    }

    /// Create a runner with custom options
    pub fn with_options(conn: &'a Connection, options: EnhancementOptions) -> Self {
        Self {
            conn,
            registry: EnhancementRegistry::default(),
            options,
        }
    }

    /// Create a runner with a custom registry
    pub fn with_registry(conn: &'a Connection, registry: EnhancementRegistry) -> Self {
        Self {
            conn,
            registry,
            options: EnhancementOptions::default(),
        }
    }

    /// Enhance a specific package by trove ID
    pub fn enhance(&self, trove_id: i64) -> EnhancementResult<EnhancementResult_> {
        info!("Enhancing trove_id={}", trove_id);

        // Create enhancement context
        let mut ctx = EnhancementContext::new(
            self.conn,
            trove_id,
            self.options.install_root.clone(),
        )?;

        // Mark as in progress
        ctx.set_status(EnhancementStatus::InProgress)?;

        let mut result = EnhancementResult_::new(trove_id);

        // Run each enhancement type
        for enhancement_type in &self.options.types {
            let engine = match self.registry.get(*enhancement_type) {
                Some(e) => e,
                None => {
                    result.record_skipped(*enhancement_type);
                    continue;
                }
            };

            // Check if enhancement should be applied
            if !engine.should_enhance(&ctx) {
                result.record_skipped(*enhancement_type);
                continue;
            }

            // Run the enhancement
            debug!(
                "Running {} enhancement for {}",
                enhancement_type, ctx.metadata.name
            );

            match engine.enhance(&mut ctx) {
                Ok(()) => {
                    result.record_success(*enhancement_type);
                }
                Err(e) => {
                    warn!(
                        "Enhancement {} failed for {}: {}",
                        enhancement_type, ctx.metadata.name, e
                    );
                    result.record_failure(*enhancement_type, e.to_string());

                    if self.options.fail_fast {
                        ctx.set_status_with_error(EnhancementStatus::Failed, &e.to_string())?;
                        return Err(e);
                    }
                }
            }
        }

        // Update status based on results
        if result.is_success() {
            ctx.set_enhancement_version(ENHANCEMENT_VERSION)?;
            info!(
                "Enhancement complete for {} (applied: {:?})",
                ctx.metadata.name, result.applied
            );
        } else {
            let error_msg = result
                .failed
                .iter()
                .map(|(t, e)| format!("{}: {}", t, e))
                .collect::<Vec<_>>()
                .join("; ");
            ctx.set_status_with_error(EnhancementStatus::Failed, &error_msg)?;
        }

        Ok(result)
    }

    /// Enhance all pending packages
    pub fn enhance_all_pending(&self) -> EnhancementResult<Vec<EnhancementResult_>> {
        let pending = ConvertedPackageInfo::find_pending(self.conn)?;
        info!("Found {} packages pending enhancement", pending.len());

        let mut results = Vec::with_capacity(pending.len());
        for package in pending {
            match self.enhance(package.trove_id) {
                Ok(result) => results.push(result),
                Err(e) => {
                    warn!(
                        "Failed to enhance {}: {}",
                        package.name, e
                    );
                    if self.options.fail_fast {
                        return Err(e);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Enhance all packages with outdated enhancement version
    pub fn enhance_all_outdated(&self) -> EnhancementResult<Vec<EnhancementResult_>> {
        let outdated = ConvertedPackageInfo::find_outdated(self.conn, ENHANCEMENT_VERSION)?;
        info!(
            "Found {} packages with outdated enhancement (version < {})",
            outdated.len(),
            ENHANCEMENT_VERSION
        );

        let mut results = Vec::with_capacity(outdated.len());
        for package in outdated {
            match self.enhance(package.trove_id) {
                Ok(result) => results.push(result),
                Err(e) => {
                    warn!(
                        "Failed to enhance {}: {}",
                        package.name, e
                    );
                    if self.options.fail_fast {
                        return Err(e);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Get enhancement statistics
    pub fn stats(&self) -> EnhancementResult<EnhancementStats> {
        ConvertedPackageInfo::count_by_status(self.conn)
    }
}

/// Summary of an enhancement batch run
#[derive(Debug, Clone)]
pub struct EnhancementSummary {
    /// Total packages processed
    pub total: usize,
    /// Successfully enhanced
    pub succeeded: usize,
    /// Enhancement failed
    pub failed: usize,
    /// Individual results
    pub results: Vec<EnhancementResult_>,
}

impl EnhancementSummary {
    /// Create a summary from results
    pub fn from_results(results: Vec<EnhancementResult_>) -> Self {
        let succeeded = results.iter().filter(|r| r.is_success()).count();
        let failed = results.len() - succeeded;
        Self {
            total: results.len(),
            succeeded,
            failed,
            results,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhancement_options_default() {
        let opts = EnhancementOptions::default();
        assert!(!opts.force);
        assert_eq!(opts.install_root, PathBuf::from("/"));
        assert_eq!(opts.types.len(), 3);
    }

    #[test]
    fn test_enhancement_options_builder() {
        let opts = EnhancementOptions::with_types(vec![EnhancementType::Capabilities])
            .force()
            .install_root("/custom");

        assert!(opts.force);
        assert_eq!(opts.install_root, PathBuf::from("/custom"));
        assert_eq!(opts.types.len(), 1);
    }

    #[test]
    fn test_enhancement_summary() {
        let mut r1 = EnhancementResult_::new(1);
        r1.record_success(EnhancementType::Capabilities);

        let mut r2 = EnhancementResult_::new(2);
        r2.record_failure(EnhancementType::Capabilities, "test error");

        let summary = EnhancementSummary::from_results(vec![r1, r2]);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 1);
    }
}
