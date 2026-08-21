// apps/remi/src/server/jobs.rs
//! Conversion job tracking and management
//!
//! Handles the 202 Accepted async conversion pattern:
//! - Create jobs for package conversion requests
//! - Track job status (pending, converting, ready, failed, cancelled)
//! - Prevent stampede (same package = same job)
//! - Clean up completed jobs after TTL

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

// Tracked jobs are lightweight metadata; the semaphore below controls real
// conversion parallelism. Keep the queue broad enough for dependency bursts.
const QUEUE_CAPACITY_MULTIPLIER: usize = 32;

/// Unique job identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl JobId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for JobId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// Job status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    /// Waiting in queue
    Pending,
    /// Actively converting
    Converting,
    /// Conversion complete, manifest available
    Ready,
    /// Conversion failed
    Failed(String),
    /// Conversion was cancelled before completion
    Cancelled(String),
}

/// Conversion result data for completed jobs
#[derive(Debug, Clone)]
pub struct ConversionResult {
    /// Signed controls and exact canonical object set served to clients.
    pub transport: conary_core::ccs::CcsTransportEnvelopeV1,
    /// Total size when reassembled
    pub total_size: u64,
    /// SHA-256 of the complete content
    pub content_hash: String,
    /// Path to the converted CCS package file
    pub ccs_path: std::path::PathBuf,
    /// Actual package version (from repo metadata, may differ from requested)
    pub actual_version: String,
    /// Sanitized passive native lifecycle summary.
    pub scriptlets: crate::server::conversion::ScriptletPackageMetadata,
}

/// A conversion job
#[derive(Debug, Clone)]
pub struct ConversionJob {
    pub id: JobId,
    /// Unique key for deduplication (distro:name:version)
    pub key: String,
    pub distro: String,
    pub package_name: String,
    pub version: Option<String>,
    pub architecture: Option<String>,
    pub status: JobStatus,
    /// Progress percentage (0-100)
    pub progress: Option<u8>,
    /// When the job was created
    pub created_at: Instant,
    /// When the job completed (for TTL cleanup)
    pub completed_at: Option<Instant>,
    /// Conversion result (populated when Ready)
    pub result: Option<ConversionResult>,
}

/// Manages conversion jobs
pub struct JobManager {
    /// Active and recently completed jobs
    jobs: HashMap<JobId, ConversionJob>,
    /// Map from job key to job ID (for deduplication)
    key_to_id: HashMap<String, JobId>,
    /// Semaphore to limit concurrent conversions (Arc for use across lock drops)
    concurrency_semaphore: Arc<Semaphore>,
    /// Maximum concurrent conversions
    max_concurrent: usize,
    /// TTL for completed jobs (1 hour)
    job_ttl: Duration,
}

impl JobManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            jobs: HashMap::new(),
            key_to_id: HashMap::new(),
            concurrency_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent,
            job_ttl: Duration::from_secs(3600), // 1 hour
        }
    }

    /// Create a new conversion job
    ///
    /// Returns the existing job ID only while work for this key is active.
    /// Terminal jobs remain available by ID for polling, but they cannot own
    /// deduplication for a later request. A ready artifact is reused through
    /// the persisted converted-package lookup before this method is called.
    /// Returns error if queue is full.
    pub fn create_job(
        &mut self,
        key: String,
        distro: String,
        package_name: String,
        version: Option<String>,
        architecture: Option<String>,
    ) -> Result<JobId, &'static str> {
        // Deduplicate only genuinely active work. Keep terminal jobs in
        // `jobs` so existing poll URLs retain their exact outcome, while
        // releasing the package key for a fresh attempt.
        if let Some(&existing_id) = self.key_to_id.get(&key) {
            if self
                .jobs
                .get(&existing_id)
                .is_some_and(|job| is_active_status(&job.status))
            {
                return Ok(existing_id);
            }
            self.key_to_id.remove(&key);
        }

        self.evict_terminal_jobs_for_capacity();

        // Check queue capacity against tracked jobs after evicting terminal
        // entries. Completed jobs are useful for polling, but they must not
        // block fresh conversion work for the full TTL during bursty package
        // installs.
        if self.jobs.len() >= self.max_concurrent * QUEUE_CAPACITY_MULTIPLIER {
            return Err("Conversion queue full");
        }

        // Create new job
        let job_id = JobId::new();
        let job = ConversionJob {
            id: job_id,
            key: key.clone(),
            distro,
            package_name,
            version,
            architecture,
            status: JobStatus::Pending,
            progress: None,
            created_at: Instant::now(),
            completed_at: None,
            result: None,
        };

        self.jobs.insert(job_id, job);
        self.key_to_id.insert(key, job_id);

        Ok(job_id)
    }

    fn evict_terminal_jobs_for_capacity(&mut self) {
        let capacity = self.max_concurrent * QUEUE_CAPACITY_MULTIPLIER;
        while self.jobs.len() >= capacity {
            let Some(oldest_terminal_id) = self
                .jobs
                .iter()
                .filter(|(_, job)| is_terminal_status(&job.status))
                .min_by_key(|(_, job)| job.completed_at.unwrap_or(job.created_at))
                .map(|(id, _)| *id)
            else {
                break;
            };

            if let Some(job) = self.jobs.remove(&oldest_terminal_id) {
                self.remove_key_mapping_if_current(&job.key, oldest_terminal_id);
                tracing::debug!(
                    "Evicted terminal conversion job for capacity: {} ({})",
                    oldest_terminal_id,
                    job.key
                );
            }
        }
    }

    /// Get a job by ID
    pub fn get_job(&self, id: &JobId) -> Option<&ConversionJob> {
        self.jobs.get(id)
    }

    /// Get the latest job associated with a package key, including terminal
    /// state retained for polling and persisted-artifact revalidation.
    pub fn get_job_by_key(&self, key: &str) -> Option<&ConversionJob> {
        self.key_to_id.get(key).and_then(|id| self.jobs.get(id))
    }

    /// Get the active job for a package key.
    ///
    /// Ready, failed, and cancelled jobs remain addressable through [`Self::get_job`], but
    /// package requests must consult persisted artifact authority for ready
    /// work and may retry failed work.
    pub fn get_active_job_by_key(&self, key: &str) -> Option<&ConversionJob> {
        self.get_job_by_key(key)
            .filter(|job| is_active_status(&job.status))
    }

    /// Update job status
    pub fn update_status(&mut self, id: &JobId, status: JobStatus) {
        if let Some(job) = self.jobs.get_mut(id) {
            let is_terminal = is_terminal_status(&status);
            job.status = status;
            if is_terminal {
                job.completed_at = Some(Instant::now());
            }
        }
    }

    /// Update job status with conversion result
    pub fn complete_with_result(&mut self, id: &JobId, result: ConversionResult) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = JobStatus::Ready;
            job.completed_at = Some(Instant::now());
            job.result = Some(result);
        }
    }

    /// Update job progress
    pub fn update_progress(&mut self, id: &JobId, progress: u8) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.progress = Some(progress.min(100));
        }
    }

    /// Clean up expired jobs
    pub fn cleanup_expired(&mut self) {
        let now = Instant::now();
        let expired: Vec<JobId> = self
            .jobs
            .iter()
            .filter(|(_, job)| {
                job.completed_at
                    .map(|t| now.duration_since(t) > self.job_ttl)
                    .unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();

        for id in expired {
            if let Some(job) = self.jobs.remove(&id) {
                self.remove_key_mapping_if_current(&job.key, id);
                tracing::debug!("Cleaned up expired job: {} ({})", id, job.key);
            }
        }
    }

    fn remove_key_mapping_if_current(&mut self, key: &str, id: JobId) {
        if self.key_to_id.get(key) == Some(&id) {
            self.key_to_id.remove(key);
        }
    }

    /// Get the concurrency semaphore for limiting parallel conversions.
    ///
    /// Returns an `Arc` clone so the permit can outlive the lock guard.
    pub fn semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.concurrency_semaphore)
    }

    /// Get statistics
    pub fn stats(&self) -> JobStats {
        let mut pending = 0;
        let mut converting = 0;
        let mut completed = 0;
        let mut failed = 0;

        for job in self.jobs.values() {
            match job.status {
                JobStatus::Pending => pending += 1,
                JobStatus::Converting => converting += 1,
                JobStatus::Ready => completed += 1,
                JobStatus::Failed(_) | JobStatus::Cancelled(_) => failed += 1,
            }
        }

        JobStats {
            pending,
            converting,
            completed,
            failed,
            total: self.jobs.len(),
        }
    }
}

fn is_terminal_status(status: &JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Ready | JobStatus::Failed(_) | JobStatus::Cancelled(_)
    )
}

fn is_active_status(status: &JobStatus) -> bool {
    matches!(status, JobStatus::Pending | JobStatus::Converting)
}

/// Job statistics
#[derive(Debug)]
pub struct JobStats {
    pub pending: usize,
    pub converting: usize,
    pub completed: usize,
    pub failed: usize,
    pub total: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_QUEUE_CAPACITY_MULTIPLIER: usize = 32;

    #[test]
    fn create_job_rejects_when_active_tracked_jobs_hit_capacity() {
        let mut manager = JobManager::new(2);

        for n in 0..(2 * EXPECTED_QUEUE_CAPACITY_MULTIPLIER) {
            manager
                .create_job(
                    format!("key-{n}"),
                    "arch".into(),
                    format!("pkg-{n}"),
                    None,
                    None,
                )
                .expect("capacity should allow initial jobs");
        }

        let err = manager
            .create_job(
                "overflow".into(),
                "arch".into(),
                "pkg-overflow".into(),
                None,
                None,
            )
            .expect_err("total tracked jobs should be capped");
        assert_eq!(err, "Conversion queue full");
    }

    #[test]
    fn create_job_absorbs_dependency_bursts_above_conversion_parallelism() {
        let max_concurrent = 16;
        let mut manager = JobManager::new(max_concurrent);

        for n in 0..(max_concurrent * 8) {
            manager
                .create_job(
                    format!("burst-key-{n}"),
                    "arch".into(),
                    format!("burst-pkg-{n}"),
                    None,
                    None,
                )
                .expect("dependency bursts should queue without making Remi the bottleneck");
        }

        assert_eq!(manager.stats().total, max_concurrent * 8);
    }

    #[test]
    fn create_job_evicts_terminal_jobs_at_capacity() {
        let mut manager = JobManager::new(1);
        let completed = manager
            .create_job("done".into(), "arch".into(), "pkg-done".into(), None, None)
            .expect("first job");
        manager.update_status(&completed, JobStatus::Ready);

        for n in 1..EXPECTED_QUEUE_CAPACITY_MULTIPLIER {
            manager
                .create_job(
                    format!("pending-{n}"),
                    "arch".into(),
                    format!("pkg-pending-{n}"),
                    None,
                    None,
                )
                .expect("fill capacity");
        }

        let fresh = manager
            .create_job(
                "fresh".into(),
                "arch".into(),
                "pkg-fresh".into(),
                None,
                None,
            )
            .expect("terminal jobs should be evicted before rejecting new work");

        assert!(manager.get_active_job_by_key("done").is_none());
        assert!(manager.get_job(&fresh).is_some());
        assert!(manager.get_active_job_by_key("pending-1").is_some());
    }

    #[test]
    fn create_job_still_deduplicates_existing_keys_at_capacity() {
        let mut manager = JobManager::new(1);
        let existing = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("first job");

        for n in 1..EXPECTED_QUEUE_CAPACITY_MULTIPLIER {
            manager
                .create_job(
                    format!("other-{n}"),
                    "arch".into(),
                    format!("pkg-other-{n}"),
                    None,
                    None,
                )
                .expect("fill capacity");
        }

        let deduped = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("existing keys should still deduplicate");
        assert_eq!(deduped, existing);
    }

    #[test]
    fn failed_job_remains_pollable_while_retry_gets_a_new_id() {
        let mut manager = JobManager::new(1);
        let failed = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("first job");
        manager.update_status(&failed, JobStatus::Failed("repository not ready".into()));

        assert!(manager.get_active_job_by_key("shared").is_none());

        let retry = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("terminal failure must release the package key");

        assert_ne!(retry, failed);
        assert!(matches!(
            manager.get_job(&failed).map(|job| &job.status),
            Some(JobStatus::Failed(error)) if error == "repository not ready"
        ));
        assert_eq!(manager.get_active_job_by_key("shared").unwrap().id, retry);
    }

    #[test]
    fn cancelled_job_remains_pollable_while_retry_gets_a_new_id() {
        let mut manager = JobManager::new(1);
        let cancelled = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("first job");
        manager.update_status(&cancelled, JobStatus::Cancelled("server shutdown".into()));

        let retry = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("cancelled work must release the package key");

        assert_ne!(retry, cancelled);
        assert!(matches!(
            manager.get_job(&cancelled).map(|job| &job.status),
            Some(JobStatus::Cancelled(error)) if error == "server shutdown"
        ));
        assert_eq!(manager.get_active_job_by_key("shared").unwrap().id, retry);
    }

    #[test]
    fn expired_old_attempt_cannot_remove_a_replacement_key_mapping() {
        let mut manager = JobManager::new(1);
        let failed = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("first job");
        manager.update_status(&failed, JobStatus::Failed("transient failure".into()));
        let retry = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("retry job");

        manager.jobs.get_mut(&failed).unwrap().completed_at =
            Some(Instant::now() - manager.job_ttl - Duration::from_secs(1));
        manager.cleanup_expired();

        assert!(manager.get_job(&failed).is_none());
        assert_eq!(manager.get_active_job_by_key("shared").unwrap().id, retry);
    }

    #[test]
    fn ready_job_does_not_replace_persisted_artifact_validation() {
        let mut manager = JobManager::new(1);
        let ready = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("first job");
        manager.update_status(&ready, JobStatus::Ready);

        let retry = manager
            .create_job("shared".into(), "arch".into(), "pkg".into(), None, None)
            .expect("a caller reaching job creation already missed persisted artifact lookup");

        assert_ne!(retry, ready);
        assert!(matches!(
            manager.get_job(&ready).map(|job| &job.status),
            Some(JobStatus::Ready)
        ));
    }
}
