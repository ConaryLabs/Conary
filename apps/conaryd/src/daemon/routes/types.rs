// apps/conaryd/src/daemon/routes/types.rs
//! Shared request and response types for daemon routes.

use crate::daemon::{DaemonError, DaemonJob, DaemonState};
use conary_core::db::models::{Changeset, DependencyEntry, GenerationPublication, Trove};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Shared daemon state type
pub type SharedState = Arc<DaemonState>;

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_secs: u64,
}

/// Version information response
#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub version: &'static str,
    pub api_version: &'static str,
    pub build_date: Option<&'static str>,
    pub git_commit: Option<&'static str>,
}

// =============================================================================
// Query Response Types
// =============================================================================

/// Package summary for list endpoints
#[derive(Debug, Serialize)]
pub struct PackageSummary {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub package_type: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub installed_at: Option<String>,
    pub install_reason: String,
    pub pinned: bool,
}

impl From<&Trove> for PackageSummary {
    fn from(trove: &Trove) -> Self {
        Self {
            name: trove.name.clone(),
            version: trove.version.clone(),
            package_type: trove.trove_type.as_str().to_string(),
            architecture: trove.architecture.clone(),
            description: trove.description.clone(),
            installed_at: trove.installed_at.clone(),
            install_reason: trove.install_reason.as_str().to_string(),
            pinned: trove.pinned,
        }
    }
}

/// Package details response
#[derive(Debug, Serialize)]
pub struct PackageDetails {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub package_type: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub installed_at: Option<String>,
    pub install_source: String,
    pub install_reason: String,
    pub selection_reason: Option<String>,
    pub flavor: Option<String>,
    pub pinned: bool,
    pub dependencies: Vec<DependencyInfo>,
}

/// Dependency info for package details
#[derive(Debug, Serialize)]
pub struct DependencyInfo {
    pub name: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub dependency_type: String,
    pub version_constraint: Option<String>,
}

impl From<&DependencyEntry> for DependencyInfo {
    fn from(dep: &DependencyEntry) -> Self {
        Self {
            name: dep.depends_on_name.clone(),
            kind: dep.kind.clone(),
            dependency_type: dep.dependency_type.clone(),
            version_constraint: dep.version_constraint.clone(),
        }
    }
}

/// Changeset history entry
#[derive(Debug, Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub description: String,
    pub status: String,
    pub created_at: Option<String>,
    pub applied_at: Option<String>,
    pub publication_status: Option<String>,
}

impl From<&Changeset> for HistoryEntry {
    fn from(cs: &Changeset) -> Self {
        Self {
            id: cs.id.unwrap_or(0),
            description: cs.description.clone(),
            status: cs.status.as_str().to_string(),
            created_at: cs.created_at.clone(),
            applied_at: cs.applied_at.clone(),
            publication_status: None,
        }
    }
}

impl HistoryEntry {
    pub(super) fn from_changeset_with_publication(
        cs: &Changeset,
        publications: &[GenerationPublication],
    ) -> Self {
        let mut entry = Self::from(cs);
        entry.publication_status = publication_status_for_changeset(publications, cs.id);
        entry
    }
}

fn publication_status_for_changeset(
    publications: &[GenerationPublication],
    changeset_id: Option<i64>,
) -> Option<String> {
    let changeset_id = changeset_id?;
    publications
        .iter()
        .find(|publication| publication.trigger_changeset_id == Some(changeset_id))
        .map(|publication| publication.status.as_str().to_string())
}

/// Search query parameters
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

// =============================================================================
// Transaction Response Types
// =============================================================================

/// Transaction (job) summary for list endpoints
#[derive(Debug, Serialize)]
pub struct TransactionSummary {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

impl From<&DaemonJob> for TransactionSummary {
    fn from(job: &DaemonJob) -> Self {
        Self {
            id: job.id.clone(),
            kind: job.kind.as_str().to_string(),
            status: job.status.as_str().to_string(),
            created_at: job.created_at.clone(),
            started_at: job.started_at.clone(),
            completed_at: job.completed_at.clone(),
        }
    }
}

/// Full transaction details
#[derive(Debug, Serialize)]
pub struct TransactionDetails {
    pub id: String,
    pub idempotency_key: Option<String>,
    pub kind: String,
    pub status: String,
    pub spec: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<DaemonError>,
    pub requested_by_uid: Option<u32>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    /// Position in queue (if queued)
    pub queue_position: Option<usize>,
}

impl TransactionDetails {
    pub(super) fn from_job(job: &DaemonJob, queue_position: Option<usize>) -> Self {
        Self {
            id: job.id.clone(),
            idempotency_key: job.idempotency_key.clone(),
            kind: job.kind.as_str().to_string(),
            status: job.status.as_str().to_string(),
            spec: job.spec.clone(),
            result: job.result.clone(),
            error: job.error.clone(),
            requested_by_uid: job.requested_by_uid,
            created_at: job.created_at.clone(),
            started_at: job.started_at.clone(),
            completed_at: job.completed_at.clone(),
            queue_position,
        }
    }
}

/// Transaction list query parameters
#[derive(Debug, Deserialize)]
pub struct TransactionListQuery {
    /// Filter by status
    pub status: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
}

// =============================================================================
// Request Types for Mutating Operations
// =============================================================================

fn is_false(value: &bool) -> bool {
    !*value
}

/// A single operation in a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransactionOperation {
    /// Install packages
    Install {
        /// Package names or file paths to install
        packages: Vec<String>,
        /// Allow downgrades
        #[serde(default)]
        allow_downgrade: bool,
        /// Skip dependency resolution
        #[serde(default)]
        skip_deps: bool,
        /// Preview without installing
        #[serde(default, skip_serializing_if = "is_false")]
        dry_run: bool,
        /// Skip package scriptlets
        #[serde(default, skip_serializing_if = "is_false")]
        no_scripts: bool,
        /// Assume yes to confirmation prompts
        #[serde(default, skip_serializing_if = "is_false")]
        yes: bool,
        /// Confirm applying this package operation
        #[serde(default, skip_serializing_if = "is_false")]
        apply_intent: bool,
        /// Explicitly acknowledge live-host mutation risk
        #[serde(default, skip_serializing_if = "is_false")]
        allow_live_system_mutation: bool,
    },
    /// Remove packages
    Remove {
        /// Package names to remove
        packages: Vec<String>,
        /// Also remove packages that depend on these
        #[serde(default)]
        cascade: bool,
        /// Also remove orphaned dependencies
        #[serde(default)]
        remove_orphans: bool,
        /// Skip package scriptlets
        #[serde(default, skip_serializing_if = "is_false")]
        no_scripts: bool,
        /// Delete adopted package files from disk
        #[serde(default, skip_serializing_if = "is_false")]
        purge_files: bool,
        /// Confirm applying this package operation
        #[serde(default, skip_serializing_if = "is_false")]
        apply_intent: bool,
        /// Explicitly acknowledge live-host mutation risk
        #[serde(default, skip_serializing_if = "is_false")]
        allow_live_system_mutation: bool,
    },
    /// Update packages
    Update {
        /// Package names to update (empty = update all)
        packages: Vec<String>,
        /// Only apply security updates
        #[serde(default)]
        security_only: bool,
        /// Preview without applying updates
        #[serde(default, skip_serializing_if = "is_false")]
        dry_run: bool,
        /// Assume yes to confirmation prompts
        #[serde(default, skip_serializing_if = "is_false")]
        yes: bool,
        /// Confirm applying this package operation
        #[serde(default, skip_serializing_if = "is_false")]
        apply_intent: bool,
        /// Explicitly acknowledge live-host mutation risk
        #[serde(default, skip_serializing_if = "is_false")]
        allow_live_system_mutation: bool,
    },
}

/// Request body for creating a transaction
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTransactionRequest {
    /// Operations to perform (install, remove, update)
    pub operations: Vec<TransactionOperation>,
}

/// Convenience request for package operations
#[derive(Debug, Clone, Deserialize)]
pub struct PackageOperationRequest {
    /// Package names to operate on
    pub packages: Vec<String>,
    /// Additional options (varies by operation type)
    #[serde(default)]
    pub options: PackageOperationOptions,
}

/// Options for package operations
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackageOperationOptions {
    /// For install: allow downgrades
    #[serde(default)]
    pub allow_downgrade: bool,
    /// For install: skip dependency resolution
    #[serde(default)]
    pub skip_deps: bool,
    /// For remove: cascade to dependents
    #[serde(default)]
    pub cascade: bool,
    /// For remove: also remove orphaned dependencies
    #[serde(default)]
    pub remove_orphans: bool,
    /// For update: only apply security updates
    #[serde(default)]
    pub security_only: bool,
    /// Preview without making changes
    #[serde(default)]
    pub dry_run: bool,
    /// Skip package scriptlets where the operation supports it
    #[serde(default)]
    pub no_scripts: bool,
    /// Assume yes to prompts where the operation supports it
    #[serde(default)]
    pub yes: bool,
    /// Confirm applying this package operation
    #[serde(default)]
    pub apply_intent: bool,
    /// Delete adopted package files from disk during remove
    #[serde(default)]
    pub purge_files: bool,
    /// Explicitly acknowledge live-host mutation risk
    #[serde(default)]
    pub allow_live_system_mutation: bool,
}

/// Response body for transaction creation
#[derive(Debug, Serialize)]
pub struct CreateTransactionResponse {
    /// Job ID
    pub job_id: String,
    /// Status
    pub status: String,
    /// Position in queue
    pub queue_position: usize,
    /// URL to check status
    pub location: String,
}

/// Response body for dry-run transaction
#[derive(Debug, Serialize)]
pub struct DryRunResponse {
    /// Operations that would be performed
    pub operations: Vec<TransactionOperation>,
    /// Summary of changes (placeholder)
    pub summary: DryRunSummary,
}

/// Summary of changes in a dry-run
#[derive(Debug, Serialize)]
pub struct DryRunSummary {
    /// Packages that would be installed
    pub install: Vec<String>,
    /// Packages that would be removed
    pub remove: Vec<String>,
    /// Packages that would be updated
    pub update: Vec<String>,
    /// Total number of packages affected
    pub total_affected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonJob;
    use conary_core::db::models::{
        GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus,
    };

    #[test]
    fn test_health_response_serialization() {
        let resp = HealthResponse {
            status: "healthy",
            version: "0.2.0",
            uptime_secs: 100,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("healthy"));
        assert!(!json.contains("pid"));
    }

    #[test]
    fn test_version_response_serialization() {
        let resp = VersionResponse {
            version: "0.2.0",
            api_version: "1.0",
            build_date: None,
            git_commit: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("0.2.0"));
        assert!(!json.contains("schema_version"));
    }

    #[test]
    fn history_publication_status_matches_changeset_debt() {
        let publications = vec![GenerationPublication {
            id: Some(1),
            trigger_changeset_id: Some(42),
            published_through_changeset_id: None,
            tx_uuid: None,
            db_path: "/tmp/conary.db".to_string(),
            runtime_root: "/tmp/conary".to_string(),
            phase: GenerationPublicationPhase::PendingBuild,
            status: GenerationPublicationStatus::Failed,
            state_number: None,
            generation_number: None,
            summary: "fixture".to_string(),
            last_error: Some("forced".to_string()),
            retry_count: 1,
            recoverable: true,
            created_at: None,
            updated_at: None,
            completed_at: None,
        }];
        assert_eq!(
            publication_status_for_changeset(&publications, Some(42)),
            Some("failed".to_string())
        );
        assert_eq!(
            publication_status_for_changeset(&publications, Some(7)),
            None
        );
    }

    #[test]
    fn daemon_job_transaction_summary_does_not_claim_publication_status() {
        let job = DaemonJob::new(
            crate::daemon::JobKind::Install,
            serde_json::json!({"packages": ["fixture"]}),
        );
        let json = serde_json::to_value(TransactionSummary::from(&job)).unwrap();
        assert!(json.get("publication_status").is_none());
        assert!(json.get("pending_publications").is_none());
    }
}
