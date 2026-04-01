// apps/conaryd/src/daemon/enhance.rs
//! Enhancement job handler for conaryd daemon
//!
//! Handles background enhancement of converted packages, including:
//! - Processing pending enhancements in priority order
//! - Emitting SSE events for progress tracking
//! - Supporting cancellation via cancel tokens

use crate::daemon::{DaemonEvent, DaemonState};
use conary_core::ccs::enhancement::{
    EnhancementOptions, EnhancementPriority, EnhancementRunner, EnhancementType,
    get_pending_by_priority,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
) -> Result<EnhanceJobResult, conary_core::Error> {
    info!(
        "Starting enhancement job with batch_size={}",
        spec.batch_size
    );

    // Run the blocking database query on a background thread to avoid
    // blocking the async executor.
    let state_clone = state.clone();
    let spec_clone = spec.clone();
    let trove_ids = tokio::task::spawn_blocking(move || -> Result<Vec<i64>, conary_core::Error> {
        let conn = state_clone.open_db()?;
        if spec_clone.trove_ids.is_empty() {
            // Fetch more candidates than needed so we can filter by min_priority
            // client-side (get_pending_by_priority returns results ordered by
            // priority DESC, so we can stop once we hit a package below the
            // threshold).
            let min_priority_val = spec_clone.min_priority.as_i32();
            let candidates = get_pending_by_priority(&conn, spec_clone.batch_size * 2)
                .map_err(|e| conary_core::Error::IoError(e.to_string()))?;

            // Filter out packages below min_priority
            if min_priority_val > 0 {
                let mut filtered = Vec::new();
                for trove_id in candidates {
                    // Look up the stored priority for this trove
                    let priority: i32 = conn
                        .query_row(
                            "SELECT COALESCE(enhancement_priority, 1) FROM converted_packages WHERE trove_id = ?1",
                            [trove_id],
                            |row| row.get(0),
                        )
                        .unwrap_or(1);
                    if priority >= min_priority_val {
                        filtered.push(trove_id);
                    }
                    if filtered.len() >= spec_clone.batch_size {
                        break;
                    }
                }
                Ok(filtered)
            } else {
                // min_priority is Low (0) -- accept everything, just cap at batch_size
                Ok(candidates.into_iter().take(spec_clone.batch_size).collect())
            }
        } else {
            Ok(spec_clone.trove_ids.clone())
        }
    })
    .await
    .map_err(|e| conary_core::Error::IoError(format!("Task join error: {e}")))??;

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

    // Run the main enhancement loop on a blocking thread since
    // EnhancementRunner does synchronous file I/O and database queries.
    let state_for_loop = state.clone();
    let cancel_for_loop = cancel_token.clone();
    tokio::task::spawn_blocking(move || -> Result<EnhanceJobResult, conary_core::Error> {
        let conn = state_for_loop.open_db()?;

        let options = EnhancementOptions {
            types,
            force: spec.force,
            install_root: state_for_loop.config.root.clone(),
            fail_fast: false,
            parallel: true,
            parallel_workers: 0,
            cancel_token: Some(cancel_for_loop.clone()),
        };

        let runner = EnhancementRunner::with_options(&conn, options);

        let mut job_result = EnhanceJobResult {
            processed: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            packages: Vec::new(),
        };

        let total = trove_ids.len();

        for (idx, trove_id) in trove_ids.iter().enumerate() {
            // Check for cancellation
            if cancel_for_loop.load(Ordering::Relaxed) {
                info!("Enhancement job cancelled after {} packages", idx);
                break;
            }

            let package_name =
                get_package_name(&conn, *trove_id).unwrap_or_else(|| format!("trove_{}", trove_id));

            state_for_loop.emit(DaemonEvent::EnhancementStarted {
                trove_id: *trove_id,
                package_name: package_name.clone(),
            });

            state_for_loop.emit(DaemonEvent::EnhancementProgress {
                trove_id: *trove_id,
                package_name: package_name.clone(),
                current: (idx + 1) as u32,
                total: total as u32,
                phase: "analyzing".to_string(),
            });

            match runner.enhance(*trove_id) {
                Ok(enhancement_result) => {
                    job_result.processed += 1;

                    let error_msg = if enhancement_result.is_success() {
                        job_result.succeeded += 1;
                        state_for_loop.emit(DaemonEvent::EnhancementCompleted {
                            trove_id: *trove_id,
                            package_name: package_name.clone(),
                            capabilities_inferred: enhancement_result
                                .applied
                                .contains(&EnhancementType::Capabilities),
                        });
                        None
                    } else {
                        job_result.failed += 1;
                        let msg = enhancement_result
                            .failed
                            .iter()
                            .map(|(t, e)| format!("{}: {}", t, e))
                            .collect::<Vec<_>>()
                            .join("; ");
                        state_for_loop.emit(DaemonEvent::EnhancementFailed {
                            trove_id: *trove_id,
                            package_name: package_name.clone(),
                            error: msg.clone(),
                        });
                        Some(msg)
                    };

                    job_result.packages.push(EnhancedPackageResult {
                        trove_id: *trove_id,
                        name: package_name,
                        success: enhancement_result.is_success(),
                        error: error_msg,
                        applied: enhancement_result
                            .applied
                            .iter()
                            .map(|t| t.to_string())
                            .collect(),
                    });
                }
                Err(e) => {
                    job_result.processed += 1;
                    job_result.failed += 1;

                    let error_msg = e.to_string();
                    warn!("Enhancement failed for {}: {}", package_name, error_msg);

                    state_for_loop.emit(DaemonEvent::EnhancementFailed {
                        trove_id: *trove_id,
                        package_name: package_name.clone(),
                        error: error_msg.clone(),
                    });

                    job_result.packages.push(EnhancedPackageResult {
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
            job_result.processed, job_result.succeeded, job_result.failed
        );

        Ok(job_result)
    })
    .await
    .map_err(|e| conary_core::Error::IoError(format!("Task join error: {e}")))?
}

/// Get package name from trove ID
fn get_package_name(conn: &Connection, trove_id: i64) -> Option<String> {
    conn.query_row("SELECT name FROM troves WHERE id = ?1", [trove_id], |row| {
        row.get(0)
    })
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
