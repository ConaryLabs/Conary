// src/daemon/enhance.rs
//! Enhancement job handler for conaryd daemon
//!
//! Handles background enhancement of converted packages, including:
//! - Processing pending enhancements in priority order
//! - Emitting SSE events for progress tracking
//! - Supporting cancellation via cancel tokens

use crate::ccs::enhancement::{
    get_pending_by_priority, EnhancementOptions, EnhancementPriority, EnhancementRunner,
    EnhancementType,
};
use crate::daemon::{DaemonEvent, DaemonState};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Enhancement job specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhanceJobSpec {
    /// Maximum number of packages to enhance in this job
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Minimum priority to process (packages below this are skipped)
    #[serde(default)]
    pub min_priority: EnhancementPriority,
    /// Specific trove IDs to enhance (if empty, process all pending)
    #[serde(default)]
    pub trove_ids: Vec<i64>,
    /// Enhancement types to run (if empty, run all)
    #[serde(default)]
    pub types: Vec<EnhancementType>,
    /// Force re-enhancement even if already done
    #[serde(default)]
    pub force: bool,
}

fn default_batch_size() -> usize {
    10
}

impl Default for EnhanceJobSpec {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            min_priority: EnhancementPriority::Low,
            trove_ids: Vec::new(),
            types: Vec::new(),
            force: false,
        }
    }
}

/// Result of an enhancement job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhanceJobResult {
    /// Number of packages processed
    pub processed: usize,
    /// Number of packages that succeeded
    pub succeeded: usize,
    /// Number of packages that failed
    pub failed: usize,
    /// Number of packages skipped (already enhanced or below priority)
    pub skipped: usize,
    /// Individual results for each package
    pub packages: Vec<EnhancedPackageResult>,
}

/// Result for a single enhanced package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedPackageResult {
    /// Trove ID
    pub trove_id: i64,
    /// Package name
    pub name: String,
    /// Whether enhancement succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Which enhancements were applied
    pub applied: Vec<String>,
}

/// Execute an enhancement job
///
/// This function is called by the daemon's job executor when an Enhance job
/// is dequeued. It processes pending packages in priority order, emitting
/// events for progress tracking.
///
/// # Arguments
/// * `state` - Daemon state for database access and event emission
/// * `spec` - Job specification
/// * `cancel_token` - Token for checking cancellation
///
/// # Returns
/// * `EnhanceJobResult` with details of what was enhanced
pub async fn execute_enhance_job(
    state: Arc<DaemonState>,
    spec: EnhanceJobSpec,
    cancel_token: Arc<AtomicBool>,
) -> Result<EnhanceJobResult, crate::Error> {
    info!("Starting enhancement job with batch_size={}", spec.batch_size);

    // Get database connection
    let conn = state.open_db()?;

    // Determine which packages to enhance
    let trove_ids = if spec.trove_ids.is_empty() {
        // Get pending packages by priority
        get_pending_by_priority(&conn, spec.batch_size)
            .map_err(|e| crate::Error::IoError(e.to_string()))?
    } else {
        spec.trove_ids.clone()
    };

    if trove_ids.is_empty() {
        info!("No packages pending enhancement");
        return Ok(EnhanceJobResult {
            processed: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            packages: Vec::new(),
        });
    }

    info!("Processing {} packages for enhancement", trove_ids.len());

    // Build enhancement options
    let types = if spec.types.is_empty() {
        EnhancementType::all().to_vec()
    } else {
        spec.types.clone()
    };

    let options = EnhancementOptions {
        types,
        force: spec.force,
        install_root: PathBuf::from("/"),
        fail_fast: false,
        parallel: true,
        parallel_workers: 0,
        cancel_token: Some(cancel_token.clone()),
    };

    let runner = EnhancementRunner::with_options(&conn, options);

    // Process each package
    let mut result = EnhanceJobResult {
        processed: 0,
        succeeded: 0,
        failed: 0,
        skipped: 0,
        packages: Vec::new(),
    };

    let total = trove_ids.len();

    for (idx, trove_id) in trove_ids.iter().enumerate() {
        // Check for cancellation
        if cancel_token.load(Ordering::Relaxed) {
            info!("Enhancement job cancelled after {} packages", idx);
            break;
        }

        // Get package name for events
        let package_name = get_package_name(&conn, *trove_id)
            .unwrap_or_else(|| format!("trove_{}", trove_id));

        // Emit start event
        state.emit(DaemonEvent::EnhancementStarted {
            trove_id: *trove_id,
            package_name: package_name.clone(),
        });

        // Emit progress event
        state.emit(DaemonEvent::EnhancementProgress {
            trove_id: *trove_id,
            package_name: package_name.clone(),
            current: (idx + 1) as u32,
            total: total as u32,
            phase: "analyzing".to_string(),
        });

        // Run enhancement
        match runner.enhance(*trove_id) {
            Ok(enhancement_result) => {
                result.processed += 1;

                if enhancement_result.is_success() {
                    result.succeeded += 1;
                    state.emit(DaemonEvent::EnhancementCompleted {
                        trove_id: *trove_id,
                        package_name: package_name.clone(),
                        capabilities_inferred: enhancement_result
                            .applied
                            .contains(&EnhancementType::Capabilities),
                    });
                } else {
                    result.failed += 1;
                    let error_msg = enhancement_result
                        .failed
                        .iter()
                        .map(|(t, e)| format!("{}: {}", t, e))
                        .collect::<Vec<_>>()
                        .join("; ");
                    state.emit(DaemonEvent::EnhancementFailed {
                        trove_id: *trove_id,
                        package_name: package_name.clone(),
                        error: error_msg.clone(),
                    });
                }

                result.packages.push(EnhancedPackageResult {
                    trove_id: *trove_id,
                    name: package_name,
                    success: enhancement_result.is_success(),
                    error: if enhancement_result.is_success() {
                        None
                    } else {
                        Some(
                            enhancement_result
                                .failed
                                .iter()
                                .map(|(t, e)| format!("{}: {}", t, e))
                                .collect::<Vec<_>>()
                                .join("; "),
                        )
                    },
                    applied: enhancement_result
                        .applied
                        .iter()
                        .map(|t| t.to_string())
                        .collect(),
                });
            }
            Err(e) => {
                result.processed += 1;
                result.failed += 1;

                let error_msg = e.to_string();
                warn!("Enhancement failed for {}: {}", package_name, error_msg);

                state.emit(DaemonEvent::EnhancementFailed {
                    trove_id: *trove_id,
                    package_name: package_name.clone(),
                    error: error_msg.clone(),
                });

                result.packages.push(EnhancedPackageResult {
                    trove_id: *trove_id,
                    name: package_name,
                    success: false,
                    error: Some(error_msg),
                    applied: Vec::new(),
                });
            }
        }
    }

    info!(
        "Enhancement job complete: {} processed, {} succeeded, {} failed",
        result.processed, result.succeeded, result.failed
    );

    Ok(result)
}

/// Get package name from trove ID
fn get_package_name(conn: &Connection, trove_id: i64) -> Option<String> {
    conn.query_row(
        "SELECT name FROM troves WHERE id = ?1",
        [trove_id],
        |row| row.get(0),
    )
    .ok()
}

/// Background enhancement worker
///
/// This function can be spawned as a background task to periodically
/// process pending enhancements when the system is idle.
pub async fn enhancement_background_worker(
    state: Arc<DaemonState>,
    check_interval_secs: u64,
    batch_size: usize,
) {
    info!("Starting enhancement background worker");

    let cancel_token = Arc::new(AtomicBool::new(false));

    loop {
        // Wait for the check interval
        tokio::time::sleep(tokio::time::Duration::from_secs(check_interval_secs)).await;

        // Check if there are pending enhancements
        let pending_count = match state.open_db() {
            Ok(conn) => get_pending_by_priority(&conn, 1)
                .map(|ids| ids.len())
                .unwrap_or(0),
            Err(e) => {
                error!("Failed to check pending enhancements: {}", e);
                continue;
            }
        };

        if pending_count == 0 {
            debug!("No pending enhancements, sleeping");
            continue;
        }

        info!("Found pending enhancements, processing batch");

        // Process a batch
        let spec = EnhanceJobSpec {
            batch_size,
            ..Default::default()
        };

        match execute_enhance_job(state.clone(), spec, cancel_token.clone()).await {
            Ok(result) => {
                if result.processed > 0 {
                    info!(
                        "Background enhancement: {} processed ({} succeeded, {} failed)",
                        result.processed, result.succeeded, result.failed
                    );
                }
            }
            Err(e) => {
                error!("Background enhancement failed: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhance_job_spec_default() {
        let spec = EnhanceJobSpec::default();
        assert_eq!(spec.batch_size, 10);
        assert!(spec.trove_ids.is_empty());
        assert!(!spec.force);
    }

    #[test]
    fn test_enhance_job_spec_serde() {
        let spec = EnhanceJobSpec {
            batch_size: 5,
            min_priority: EnhancementPriority::High,
            trove_ids: vec![1, 2, 3],
            types: vec![EnhancementType::Capabilities],
            force: true,
        };

        let json = serde_json::to_string(&spec).unwrap();
        let parsed: EnhanceJobSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.batch_size, 5);
        assert_eq!(parsed.trove_ids, vec![1, 2, 3]);
        assert!(parsed.force);
    }

    #[test]
    fn test_enhanced_package_result() {
        let result = EnhancedPackageResult {
            trove_id: 42,
            name: "nginx".to_string(),
            success: true,
            error: None,
            applied: vec!["capabilities".to_string(), "provenance".to_string()],
        };

        assert!(result.success);
        assert_eq!(result.applied.len(), 2);
    }
}
