// src/daemon/jobs.rs

//! Job persistence and queue management for conaryd
//!
//! Jobs represent asynchronous operations (install, remove, update, etc.) that
//! are queued for execution. They persist across daemon restarts.

use crate::daemon::{DaemonError, JobId, JobKind, JobStatus};
use crate::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// A persisted job record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonJob {
    /// Unique job identifier (UUID)
    pub id: JobId,
    /// Client-provided idempotency key for deduplication
    pub idempotency_key: Option<String>,
    /// Type of operation
    pub kind: JobKind,
    /// Operation specification (serialized JSON)
    pub spec: serde_json::Value,
    /// Current status
    pub status: JobStatus,
    /// Result (if completed)
    pub result: Option<serde_json::Value>,
    /// Error details (if failed)
    pub error: Option<DaemonError>,
    /// UID of requesting user
    pub requested_by_uid: Option<u32>,
    /// Client information
    pub client_info: Option<String>,
    /// Creation timestamp
    pub created_at: String,
    /// Start timestamp
    pub started_at: Option<String>,
    /// Completion timestamp
    pub completed_at: Option<String>,
}

impl DaemonJob {
    /// Create a new job
    pub fn new(kind: JobKind, spec: serde_json::Value) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            idempotency_key: None,
            kind,
            spec,
            status: JobStatus::Queued,
            result: None,
            error: None,
            requested_by_uid: None,
            client_info: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            started_at: None,
            completed_at: None,
        }
    }

    /// Create a new job with an idempotency key
    pub fn with_idempotency_key(mut self, key: String) -> Self {
        self.idempotency_key = Some(key);
        self
    }

    /// Set the requesting user
    pub fn with_uid(mut self, uid: u32) -> Self {
        self.requested_by_uid = Some(uid);
        self
    }

    /// Set client info
    pub fn with_client_info(mut self, info: String) -> Self {
        self.client_info = Some(info);
        self
    }

    /// Insert this job into the database
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        let kind_str = serde_json::to_string(&self.kind)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;
        // Remove quotes from serialized enum
        let kind_str = kind_str.trim_matches('"');

        let status_str = serde_json::to_string(&self.status)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;
        let status_str = status_str.trim_matches('"');

        let spec_json = serde_json::to_string(&self.spec)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;

        let error_json = self.error.as_ref().map(|e| {
            serde_json::to_string(e).unwrap_or_default()
        });

        let result_json = self.result.as_ref().map(|r| {
            serde_json::to_string(r).unwrap_or_default()
        });

        conn.execute(
            "INSERT INTO daemon_jobs (id, idempotency_key, kind, spec_json, status, result_json,
             error_json, requested_by_uid, client_info, created_at, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                &self.id,
                &self.idempotency_key,
                kind_str,
                spec_json,
                status_str,
                result_json,
                error_json,
                self.requested_by_uid.map(|u| u as i64),
                &self.client_info,
                &self.created_at,
                &self.started_at,
                &self.completed_at,
            ],
        )?;

        Ok(())
    }

    /// Find a job by ID
    pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, idempotency_key, kind, spec_json, status, result_json, error_json,
             requested_by_uid, client_info, created_at, started_at, completed_at
             FROM daemon_jobs WHERE id = ?1"
        )?;

        let job = stmt.query_row([id], Self::from_row).optional()?;
        Ok(job)
    }

    /// Find a job by idempotency key
    pub fn find_by_idempotency_key(conn: &Connection, key: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, idempotency_key, kind, spec_json, status, result_json, error_json,
             requested_by_uid, client_info, created_at, started_at, completed_at
             FROM daemon_jobs WHERE idempotency_key = ?1"
        )?;

        let job = stmt.query_row([key], Self::from_row).optional()?;
        Ok(job)
    }

    /// List jobs by status
    pub fn list_by_status(conn: &Connection, status: JobStatus) -> Result<Vec<Self>> {
        let status_str = serde_json::to_string(&status)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;
        let status_str = status_str.trim_matches('"');

        let mut stmt = conn.prepare(
            "SELECT id, idempotency_key, kind, spec_json, status, result_json, error_json,
             requested_by_uid, client_info, created_at, started_at, completed_at
             FROM daemon_jobs WHERE status = ?1 ORDER BY created_at ASC"
        )?;

        let jobs = stmt
            .query_map([status_str], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(jobs)
    }

    /// List all jobs (most recent first)
    pub fn list_all(conn: &Connection, limit: Option<usize>) -> Result<Vec<Self>> {
        let sql = match limit {
            Some(n) => format!(
                "SELECT id, idempotency_key, kind, spec_json, status, result_json, error_json,
                 requested_by_uid, client_info, created_at, started_at, completed_at
                 FROM daemon_jobs ORDER BY created_at DESC LIMIT {}", n
            ),
            None => "SELECT id, idempotency_key, kind, spec_json, status, result_json, error_json,
                 requested_by_uid, client_info, created_at, started_at, completed_at
                 FROM daemon_jobs ORDER BY created_at DESC".to_string(),
        };

        let mut stmt = conn.prepare(&sql)?;
        let jobs = stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(jobs)
    }

    /// Update job status
    pub fn update_status(conn: &Connection, id: &str, status: JobStatus) -> Result<bool> {
        let status_str = serde_json::to_string(&status)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;
        let status_str = status_str.trim_matches('"');

        let timestamp = chrono::Utc::now().to_rfc3339();

        let (_timestamp_field, rows) = match status {
            JobStatus::Running => {
                let rows = conn.execute(
                    "UPDATE daemon_jobs SET status = ?1, started_at = ?2 WHERE id = ?3",
                    params![status_str, &timestamp, id],
                )?;
                ("started_at", rows)
            }
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
                let rows = conn.execute(
                    "UPDATE daemon_jobs SET status = ?1, completed_at = ?2 WHERE id = ?3",
                    params![status_str, &timestamp, id],
                )?;
                ("completed_at", rows)
            }
            JobStatus::Queued => {
                let rows = conn.execute(
                    "UPDATE daemon_jobs SET status = ?1 WHERE id = ?2",
                    params![status_str, id],
                )?;
                ("", rows)
            }
        };

        Ok(rows > 0)
    }

    /// Update job with result
    pub fn set_result(conn: &Connection, id: &str, result: &serde_json::Value) -> Result<bool> {
        let result_json = serde_json::to_string(result)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;

        let rows = conn.execute(
            "UPDATE daemon_jobs SET result_json = ?1 WHERE id = ?2",
            params![result_json, id],
        )?;

        Ok(rows > 0)
    }

    /// Update job with error
    pub fn set_error(conn: &Connection, id: &str, error: &DaemonError) -> Result<bool> {
        let error_json = serde_json::to_string(error)
            .map_err(|e| crate::Error::IoError(e.to_string()))?;

        let rows = conn.execute(
            "UPDATE daemon_jobs SET error_json = ?1 WHERE id = ?2",
            params![error_json, id],
        )?;

        Ok(rows > 0)
    }

    /// Delete old completed jobs (cleanup)
    pub fn cleanup_old(conn: &Connection, days: i64) -> Result<usize> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
        let cutoff_str = cutoff.to_rfc3339();

        let rows = conn.execute(
            "DELETE FROM daemon_jobs WHERE status IN ('completed', 'failed', 'cancelled')
             AND completed_at < ?1",
            params![cutoff_str],
        )?;

        Ok(rows)
    }

    /// Convert a database row to a DaemonJob
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let id: String = row.get(0)?;
        let idempotency_key: Option<String> = row.get(1)?;
        let kind_str: String = row.get(2)?;
        let spec_json: String = row.get(3)?;
        let status_str: String = row.get(4)?;
        let result_json: Option<String> = row.get(5)?;
        let error_json: Option<String> = row.get(6)?;
        let requested_by_uid: Option<i64> = row.get(7)?;
        let client_info: Option<String> = row.get(8)?;
        let created_at: String = row.get(9)?;
        let started_at: Option<String> = row.get(10)?;
        let completed_at: Option<String> = row.get(11)?;

        // Parse kind
        let kind: JobKind = serde_json::from_str(&format!("\"{}\"", kind_str))
            .unwrap_or(JobKind::Install);

        // Parse status
        let status: JobStatus = serde_json::from_str(&format!("\"{}\"", status_str))
            .unwrap_or(JobStatus::Queued);

        // Parse spec
        let spec: serde_json::Value = serde_json::from_str(&spec_json)
            .unwrap_or(serde_json::Value::Null);

        // Parse result
        let result: Option<serde_json::Value> = result_json
            .and_then(|s| serde_json::from_str(&s).ok());

        // Parse error
        let error: Option<DaemonError> = error_json
            .and_then(|s| serde_json::from_str(&s).ok());

        Ok(Self {
            id,
            idempotency_key,
            kind,
            spec,
            status,
            result,
            error,
            requested_by_uid: requested_by_uid.map(|u| u as u32),
            client_info,
            created_at,
            started_at,
            completed_at,
        })
    }
}

/// Priority levels for jobs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobPriority {
    /// Low priority (background tasks like GC)
    Low = 0,
    /// Normal priority (user-initiated operations)
    Normal = 1,
    /// High priority (security updates)
    High = 2,
}

/// A queued job with priority
#[derive(Debug, Clone)]
pub struct QueuedJob {
    pub job: DaemonJob,
    pub priority: JobPriority,
    pub cancel_token: Arc<AtomicBool>,
}

/// Operation queue for managing job execution
///
/// Jobs are executed serially (one at a time) to ensure database consistency.
/// The queue supports priority ordering.
pub struct OperationQueue {
    /// Pending jobs (priority queue)
    queue: Mutex<VecDeque<QueuedJob>>,
    /// Currently running job ID (if any)
    current_job: RwLock<Option<JobId>>,
    /// Cancel tokens for jobs
    cancel_tokens: RwLock<std::collections::HashMap<JobId, Arc<AtomicBool>>>,
}

impl OperationQueue {
    /// Create a new operation queue
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            current_job: RwLock::new(None),
            cancel_tokens: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Enqueue a job
    pub async fn enqueue(&self, job: DaemonJob, priority: JobPriority) -> Arc<AtomicBool> {
        let cancel_token = Arc::new(AtomicBool::new(false));
        let queued = QueuedJob {
            job: job.clone(),
            priority,
            cancel_token: cancel_token.clone(),
        };

        // Store cancel token
        self.cancel_tokens.write().await.insert(job.id.clone(), cancel_token.clone());

        // Insert in priority order
        let mut queue = self.queue.lock().await;
        let insert_pos = queue.iter()
            .position(|j| j.priority < priority)
            .unwrap_or(queue.len());
        queue.insert(insert_pos, queued);

        cancel_token
    }

    /// Dequeue the next job to execute
    pub async fn dequeue(&self) -> Option<QueuedJob> {
        let mut queue = self.queue.lock().await;
        queue.pop_front()
    }

    /// Get the current running job ID
    pub async fn current(&self) -> Option<JobId> {
        self.current_job.read().await.clone()
    }

    /// Set the current running job
    pub async fn set_current(&self, job_id: Option<JobId>) {
        *self.current_job.write().await = job_id;
    }

    /// Get queue length
    pub async fn len(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Check if queue is empty
    pub async fn is_empty(&self) -> bool {
        self.queue.lock().await.is_empty()
    }

    /// Get position of a job in queue (0-based)
    pub async fn position(&self, job_id: &str) -> Option<usize> {
        let queue = self.queue.lock().await;
        queue.iter().position(|j| j.job.id == job_id)
    }

    /// Cancel a job by ID
    pub async fn cancel(&self, job_id: &str) -> bool {
        // Check if it's the currently running job
        if let Some(ref current) = *self.current_job.read().await {
            if current == job_id {
                // Set cancel token
                if let Some(token) = self.cancel_tokens.read().await.get(job_id) {
                    token.store(true, Ordering::Relaxed);
                    return true;
                }
            }
        }

        // Check if it's in the queue
        let mut queue = self.queue.lock().await;
        if let Some(pos) = queue.iter().position(|j| j.job.id == job_id) {
            queue.remove(pos);
            self.cancel_tokens.write().await.remove(job_id);
            return true;
        }

        false
    }

    /// Remove a job's cancel token (called when job completes)
    pub async fn remove_token(&self, job_id: &str) {
        self.cancel_tokens.write().await.remove(job_id);
    }

    /// Get cancel token for a job
    pub async fn get_cancel_token(&self, job_id: &str) -> Option<Arc<AtomicBool>> {
        self.cancel_tokens.read().await.get(job_id).cloned()
    }

    /// List all queued jobs
    pub async fn list_queued(&self) -> Vec<DaemonJob> {
        let queue = self.queue.lock().await;
        queue.iter().map(|qj| qj.job.clone()).collect()
    }
}

impl Default for OperationQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_job_crud() {
        let (_temp, conn) = create_test_db();

        // Create a job
        let job = DaemonJob::new(
            JobKind::Install,
            serde_json::json!({"packages": ["nginx"]}),
        );
        let job_id = job.id.clone();

        job.insert(&conn).unwrap();

        // Find by ID
        let found = DaemonJob::find_by_id(&conn, &job_id).unwrap().unwrap();
        assert_eq!(found.id, job_id);
        assert_eq!(found.kind, JobKind::Install);
        assert_eq!(found.status, JobStatus::Queued);

        // Update status
        DaemonJob::update_status(&conn, &job_id, JobStatus::Running).unwrap();
        let running = DaemonJob::find_by_id(&conn, &job_id).unwrap().unwrap();
        assert_eq!(running.status, JobStatus::Running);
        assert!(running.started_at.is_some());

        // Complete with result
        DaemonJob::update_status(&conn, &job_id, JobStatus::Completed).unwrap();
        DaemonJob::set_result(&conn, &job_id, &serde_json::json!({"installed": 1})).unwrap();

        let completed = DaemonJob::find_by_id(&conn, &job_id).unwrap().unwrap();
        assert_eq!(completed.status, JobStatus::Completed);
        assert!(completed.completed_at.is_some());
        assert!(completed.result.is_some());
    }

    #[test]
    fn test_idempotency_key() {
        let (_temp, conn) = create_test_db();

        let job = DaemonJob::new(
            JobKind::Install,
            serde_json::json!({"packages": ["nginx"]}),
        ).with_idempotency_key("unique-key-123".to_string());

        job.insert(&conn).unwrap();

        // Find by idempotency key
        let found = DaemonJob::find_by_idempotency_key(&conn, "unique-key-123")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, job.id);

        // Duplicate idempotency key should fail
        let dup = DaemonJob::new(
            JobKind::Install,
            serde_json::json!({"packages": ["curl"]}),
        ).with_idempotency_key("unique-key-123".to_string());

        assert!(dup.insert(&conn).is_err());
    }

    #[test]
    fn test_list_by_status() {
        let (_temp, conn) = create_test_db();

        // Create jobs with different statuses
        let job1 = DaemonJob::new(JobKind::Install, serde_json::json!({}));
        let job2 = DaemonJob::new(JobKind::Remove, serde_json::json!({}));
        let job3 = DaemonJob::new(JobKind::Update, serde_json::json!({}));

        job1.insert(&conn).unwrap();
        job2.insert(&conn).unwrap();
        job3.insert(&conn).unwrap();

        DaemonJob::update_status(&conn, &job2.id, JobStatus::Running).unwrap();
        DaemonJob::update_status(&conn, &job3.id, JobStatus::Completed).unwrap();

        // List queued
        let queued = DaemonJob::list_by_status(&conn, JobStatus::Queued).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, job1.id);

        // List running
        let running = DaemonJob::list_by_status(&conn, JobStatus::Running).unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, job2.id);
    }

    #[tokio::test]
    async fn test_operation_queue() {
        let queue = OperationQueue::new();

        // Enqueue jobs with different priorities
        let job1 = DaemonJob::new(JobKind::Install, serde_json::json!({}));
        let job2 = DaemonJob::new(JobKind::GarbageCollect, serde_json::json!({}));
        let job3 = DaemonJob::new(JobKind::Update, serde_json::json!({}));

        let id1 = job1.id.clone();
        let id2 = job2.id.clone();
        let id3 = job3.id.clone();

        queue.enqueue(job1, JobPriority::Normal).await;
        queue.enqueue(job2, JobPriority::Low).await;
        queue.enqueue(job3, JobPriority::High).await;

        // High priority should come first
        let next = queue.dequeue().await.unwrap();
        assert_eq!(next.job.id, id3);

        // Then normal
        let next = queue.dequeue().await.unwrap();
        assert_eq!(next.job.id, id1);

        // Then low
        let next = queue.dequeue().await.unwrap();
        assert_eq!(next.job.id, id2);

        // Queue should be empty
        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_cancel_queued_job() {
        let queue = OperationQueue::new();

        let job = DaemonJob::new(JobKind::Install, serde_json::json!({}));
        let job_id = job.id.clone();

        queue.enqueue(job, JobPriority::Normal).await;
        assert_eq!(queue.len().await, 1);

        // Cancel the job
        let cancelled = queue.cancel(&job_id).await;
        assert!(cancelled);
        assert!(queue.is_empty().await);
    }
}
