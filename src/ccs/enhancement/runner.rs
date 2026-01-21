// src/ccs/enhancement/runner.rs
//! Enhancement runner that orchestrates enhancement execution

use super::context::{ConvertedPackageInfo, EnhancementContext, EnhancementStats};
use super::error::{EnhancementError, EnhancementResult};
use super::registry::EnhancementRegistry;
use super::{EnhancementResult_, EnhancementStatus, EnhancementType, ENHANCEMENT_VERSION};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    /// Enable parallel file analysis within packages
    pub parallel: bool,
    /// Number of worker threads for parallel analysis (0 = auto)
    pub parallel_workers: usize,
    /// Cancellation token for aborting enhancement
    pub cancel_token: Option<Arc<AtomicBool>>,
}

impl Default for EnhancementOptions {
    fn default() -> Self {
        Self {
            types: EnhancementType::all().to_vec(),
            force: false,
            install_root: PathBuf::from("/"),
            fail_fast: false,
            parallel: true, // Enable by default for performance
            parallel_workers: 0, // Auto-detect
            cancel_token: None,
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

    /// Enable or disable parallel processing
    pub fn parallel(mut self, enabled: bool) -> Self {
        self.parallel = enabled;
        self
    }

    /// Set number of parallel workers
    pub fn workers(mut self, count: usize) -> Self {
        self.parallel_workers = count;
        self
    }

    /// Set cancellation token
    pub fn with_cancel_token(mut self, token: Arc<AtomicBool>) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Check if cancellation was requested
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .map(|t| t.load(Ordering::Relaxed))
            .unwrap_or(false)
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
        // Check cancellation before starting
        if self.options.is_cancelled() {
            return Err(EnhancementError::Cancelled);
        }

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
            // Check cancellation between enhancement types
            if self.options.is_cancelled() {
                info!("Enhancement cancelled for {}", ctx.metadata.name);
                ctx.set_status_with_error(EnhancementStatus::Failed, "cancelled")?;
                return Err(EnhancementError::Cancelled);
            }

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

// ============================================================================
// Lazy Enhancement Scheduling
// ============================================================================

/// Mode for handling enhancement during package installation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhancementMode {
    /// Run enhancement synchronously during installation (blocking)
    Immediate,
    /// Schedule enhancement for background processing (non-blocking)
    Lazy,
    /// Skip enhancement entirely
    Skip,
}

impl Default for EnhancementMode {
    fn default() -> Self {
        Self::Lazy
    }
}

/// Schedule a converted package for lazy enhancement
///
/// This marks the package's enhancement status as 'pending' without actually
/// running the enhancement. The enhancement will be picked up later by:
/// - The conaryd daemon's background worker
/// - An explicit `conary ccs enhance --all-pending` command
///
/// # Arguments
/// * `conn` - Database connection
/// * `trove_id` - The trove ID of the converted package
/// * `priority` - Higher priority packages will be enhanced first
///
/// # Returns
/// * `Ok(true)` if the package was scheduled
/// * `Ok(false)` if the package is already enhanced or being enhanced
pub fn schedule_for_enhancement(
    conn: &Connection,
    trove_id: i64,
    priority: EnhancementPriority,
) -> EnhancementResult<bool> {
    // Check current status
    let status: Option<String> = conn
        .query_row(
            "SELECT enhancement_status FROM converted_packages WHERE trove_id = ?1",
            [trove_id],
            |row| row.get(0),
        )
        .ok();

    match status.as_deref() {
        Some("complete") | Some("in_progress") => {
            debug!("Skipping schedule for trove_id={}: already enhanced/in progress", trove_id);
            return Ok(false);
        }
        _ => {}
    }

    // Update status to pending with priority
    conn.execute(
        "UPDATE converted_packages
         SET enhancement_status = 'pending', enhancement_priority = ?2
         WHERE trove_id = ?1",
        rusqlite::params![trove_id, priority.as_i32()],
    )?;

    info!("Scheduled trove_id={} for lazy enhancement (priority={})", trove_id, priority);
    Ok(true)
}

/// Priority levels for enhancement scheduling
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnhancementPriority {
    /// Low priority - background enhancement
    Low = 0,
    /// Normal priority - standard packages
    Normal = 1,
    /// High priority - security-sensitive packages
    High = 2,
    /// Critical priority - packages with network access
    Critical = 3,
}

impl Default for EnhancementPriority {
    fn default() -> Self {
        Self::Normal
    }
}

impl EnhancementPriority {
    /// Convert to i32 for database storage
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Create from i32
    pub fn from_i32(val: i32) -> Self {
        match val {
            0 => Self::Low,
            1 => Self::Normal,
            2 => Self::High,
            _ => Self::Critical,
        }
    }
}

impl std::fmt::Display for EnhancementPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Get pending packages ordered by priority for processing
pub fn get_pending_by_priority(conn: &Connection, limit: usize) -> EnhancementResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT trove_id FROM converted_packages
         WHERE enhancement_status = 'pending'
         ORDER BY COALESCE(enhancement_priority, 1) DESC, id ASC
         LIMIT ?1",
    )?;

    let trove_ids = stmt
        .query_map([limit as i64], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(trove_ids)
}

/// Check if a package can be safely used while enhancement is pending
///
/// This is the "enhancement window safety" check. During the window between
/// installation and enhancement completion:
/// - Package is fully functional for its declared purpose
/// - Capability restrictions are not yet enforced
/// - A warning is shown to users
///
/// # Arguments
/// * `conn` - Database connection
/// * `trove_id` - The trove ID to check
///
/// # Returns
/// * `EnhancementWindowStatus` indicating the package's enhancement state
pub fn check_enhancement_window(conn: &Connection, trove_id: i64) -> EnhancementWindowStatus {
    let status: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT cp.enhancement_status, t.name
             FROM converted_packages cp
             JOIN troves t ON t.id = cp.trove_id
             WHERE cp.trove_id = ?1",
            [trove_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    match status {
        Some((status, name)) => {
            let package_name = name.unwrap_or_else(|| "unknown".to_string());
            match status.as_str() {
                "complete" => EnhancementWindowStatus::Complete,
                "pending" | "in_progress" => EnhancementWindowStatus::InProgress { package_name },
                "failed" => EnhancementWindowStatus::Failed { package_name },
                "skipped" => EnhancementWindowStatus::Skipped,
                _ => EnhancementWindowStatus::Unknown,
            }
        }
        None => EnhancementWindowStatus::NotConverted,
    }
}

/// Status of a package's enhancement window
#[derive(Debug, Clone)]
pub enum EnhancementWindowStatus {
    /// Enhancement is complete - full capability enforcement available
    Complete,
    /// Enhancement is pending or in progress - capabilities not yet known
    InProgress {
        package_name: String,
    },
    /// Enhancement failed - capabilities may be incomplete
    Failed {
        package_name: String,
    },
    /// Enhancement was skipped (not needed)
    Skipped,
    /// Not a converted package
    NotConverted,
    /// Unknown status
    Unknown,
}

impl EnhancementWindowStatus {
    /// Check if the package is safe to use (may have incomplete capabilities)
    pub fn is_usable(&self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Check if enhancement is complete
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete | Self::Skipped)
    }

    /// Get a warning message if applicable
    pub fn warning_message(&self) -> Option<String> {
        match self {
            Self::InProgress { package_name } => Some(format!(
                "Package '{}' is pending capability analysis. \
                 Capability restrictions will be enforced after analysis completes.",
                package_name
            )),
            Self::Failed { package_name } => Some(format!(
                "Package '{}' capability analysis failed. \
                 Running without full capability restrictions.",
                package_name
            )),
            _ => None,
        }
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

    #[test]
    fn test_enhancement_mode_default() {
        let mode = EnhancementMode::default();
        assert_eq!(mode, EnhancementMode::Lazy);
    }

    #[test]
    fn test_enhancement_priority_ordering() {
        assert!(EnhancementPriority::Critical > EnhancementPriority::High);
        assert!(EnhancementPriority::High > EnhancementPriority::Normal);
        assert!(EnhancementPriority::Normal > EnhancementPriority::Low);
    }

    #[test]
    fn test_enhancement_priority_roundtrip() {
        for priority in [
            EnhancementPriority::Low,
            EnhancementPriority::Normal,
            EnhancementPriority::High,
            EnhancementPriority::Critical,
        ] {
            let val = priority.as_i32();
            let roundtrip = EnhancementPriority::from_i32(val);
            assert_eq!(priority, roundtrip);
        }
    }

    #[test]
    fn test_enhancement_window_status() {
        // Test usability
        assert!(EnhancementWindowStatus::Complete.is_usable());
        assert!(EnhancementWindowStatus::InProgress { package_name: "test".into() }.is_usable());
        assert!(!EnhancementWindowStatus::Unknown.is_usable());

        // Test completeness
        assert!(EnhancementWindowStatus::Complete.is_complete());
        assert!(EnhancementWindowStatus::Skipped.is_complete());
        assert!(!EnhancementWindowStatus::InProgress { package_name: "test".into() }.is_complete());
    }

    #[test]
    fn test_enhancement_window_warning() {
        // No warning for complete
        assert!(EnhancementWindowStatus::Complete.warning_message().is_none());

        // Warning for in-progress
        let warning = EnhancementWindowStatus::InProgress {
            package_name: "nginx".into(),
        }
        .warning_message();
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("nginx"));

        // Warning for failed
        let warning = EnhancementWindowStatus::Failed {
            package_name: "httpd".into(),
        }
        .warning_message();
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("httpd"));
    }

    #[test]
    fn test_cancellation_token() {
        let token = Arc::new(AtomicBool::new(false));
        let opts = EnhancementOptions::default().with_cancel_token(token.clone());

        assert!(!opts.is_cancelled());

        token.store(true, Ordering::Relaxed);
        assert!(opts.is_cancelled());
    }

    #[test]
    fn test_parallel_options() {
        let opts = EnhancementOptions::default()
            .parallel(true)
            .workers(4);

        assert!(opts.parallel);
        assert_eq!(opts.parallel_workers, 4);
    }
}
