// src/commands/generation/takeover_state.rs

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::commands::operation_records::{
    load_json_record, takeover_operations_dir, write_json_record,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TakeoverStatus {
    Planning,
    Running,
    ReadyToActivate,
    CompletedWithWarnings,
    Incomplete,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TakeoverPhase {
    Planning,
    Cas,
    Owned,
    Generation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TakeoverInventory {
    pub already_cas_backed: Vec<String>,
    pub needs_cas_upgrade: Vec<String>,
    pub not_tracked: Vec<String>,
    pub already_owned: Vec<String>,
    pub needs_pm_removal: Vec<String>,
    pub blocked: Vec<String>,
    pub total_system_packages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BootEntryOutcome {
    #[default]
    NotAttempted,
    Written,
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeoverRecord {
    pub id: String,
    pub requested_level: String,
    pub status: TakeoverStatus,
    pub discovered_package_manager: String,
    pub discovered_bootloader: String,
    pub inventory: TakeoverInventory,
    pub started_at: String,
    pub updated_at: String,
    pub completed_phases: Vec<TakeoverPhase>,
    pub current_phase: Option<TakeoverPhase>,
    pub adoption_failures: Vec<String>,
    pub cas_upgrade_failures: Vec<String>,
    pub ownership_query_failures: Vec<String>,
    pub pm_removal_failures: Vec<String>,
    pub warnings: Vec<String>,
    pub failure_reason: Option<String>,
    pub generation_number: Option<i64>,
    pub boot_entry_outcome: BootEntryOutcome,
    pub activation_pending: bool,
}

impl TakeoverRecord {
    #[must_use]
    pub fn planned(
        db_path: &str,
        requested_level: &str,
        inventory: TakeoverInventory,
        discovered_package_manager: &str,
        discovered_bootloader: &str,
    ) -> Self {
        let now = now_string();
        let id = next_operation_id(db_path);
        Self {
            id,
            requested_level: requested_level.to_string(),
            status: TakeoverStatus::Planning,
            discovered_package_manager: discovered_package_manager.to_string(),
            discovered_bootloader: discovered_bootloader.to_string(),
            inventory,
            started_at: now.clone(),
            updated_at: now,
            completed_phases: Vec::new(),
            current_phase: None,
            adoption_failures: Vec::new(),
            cas_upgrade_failures: Vec::new(),
            ownership_query_failures: Vec::new(),
            pm_removal_failures: Vec::new(),
            warnings: Vec::new(),
            failure_reason: None,
            generation_number: None,
            boot_entry_outcome: BootEntryOutcome::NotAttempted,
            activation_pending: false,
        }
    }

    #[must_use]
    pub fn path(&self, db_path: &str) -> PathBuf {
        takeover_operations_dir(db_path).join(format!("{}.json", self.id))
    }

    pub fn save(&self, db_path: &str) -> Result<()> {
        write_json_record(&self.path(db_path), self)
    }

    pub fn load_latest_incomplete(db_path: &str) -> Result<Option<Self>> {
        let dir = takeover_operations_dir(db_path);
        if !dir.exists() {
            return Ok(None);
        }

        let mut latest: Option<Self> = None;
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let record: Self = load_json_record(&path)?;
            if !record.is_resumable() {
                continue;
            }

            let replace = latest
                .as_ref()
                .map(|current| record.started_at >= current.started_at)
                .unwrap_or(true);
            if replace {
                latest = Some(record);
            }
        }

        Ok(latest)
    }

    #[must_use]
    pub fn is_resumable(&self) -> bool {
        matches!(
            self.status,
            TakeoverStatus::Planning | TakeoverStatus::Running | TakeoverStatus::Incomplete
        )
    }

    pub fn start_phase(&mut self, phase: TakeoverPhase) {
        self.current_phase = Some(phase);
        if self.status != TakeoverStatus::Failed {
            self.status = TakeoverStatus::Running;
        }
        self.touch();
    }

    pub fn finish_phase(&mut self, phase: TakeoverPhase) {
        if !self.completed_phases.contains(&phase) {
            self.completed_phases.push(phase);
        }
        self.current_phase = None;
        self.status = reduce_takeover_status(
            &self.completed_phases,
            self.boot_entry_outcome.clone(),
            self.has_incomplete_failures(),
        );
        self.activation_pending = self.status == TakeoverStatus::ReadyToActivate;
        self.touch();
    }

    pub fn finish_generation(
        &mut self,
        generation_number: i64,
        boot_entry_outcome: BootEntryOutcome,
    ) {
        self.generation_number = Some(generation_number);
        self.boot_entry_outcome = boot_entry_outcome;
        self.finish_phase(TakeoverPhase::Generation);
    }

    pub fn mark_failed(&mut self, reason: impl Into<String>) {
        self.failure_reason = Some(reason.into());
        self.current_phase = None;
        self.status = TakeoverStatus::Failed;
        self.activation_pending = false;
        self.touch();
    }

    pub fn mark_incomplete(&mut self, reason: impl Into<String>) {
        self.warnings.push(reason.into());
        self.current_phase = None;
        self.status = TakeoverStatus::Incomplete;
        self.activation_pending = false;
        self.touch();
    }

    pub fn record_adoption_failures(&mut self, failures: Vec<String>) {
        self.adoption_failures.extend(failures);
        self.touch();
    }

    pub fn record_cas_upgrade_failures(&mut self, failures: Vec<String>) {
        self.cas_upgrade_failures.extend(failures);
        self.touch();
    }

    pub fn record_ownership_query_failures(&mut self, failures: Vec<String>) {
        self.ownership_query_failures.extend(failures);
        self.touch();
    }

    pub fn record_pm_removal_failures(&mut self, failures: Vec<String>) {
        self.pm_removal_failures.extend(failures);
        self.touch();
    }

    #[must_use]
    pub fn has_incomplete_failures(&self) -> bool {
        !self.adoption_failures.is_empty()
            || !self.cas_upgrade_failures.is_empty()
            || !self.ownership_query_failures.is_empty()
            || !self.pm_removal_failures.is_empty()
    }

    fn touch(&mut self) {
        self.updated_at = now_string();
    }
}

#[must_use]
pub fn reduce_takeover_status(
    phases: &[TakeoverPhase],
    boot_entry_outcome: BootEntryOutcome,
    has_incomplete_failures: bool,
) -> TakeoverStatus {
    if has_incomplete_failures {
        return TakeoverStatus::Incomplete;
    }

    if phases.contains(&TakeoverPhase::Generation) {
        return match boot_entry_outcome {
            BootEntryOutcome::Written => TakeoverStatus::ReadyToActivate,
            BootEntryOutcome::Skipped(_) | BootEntryOutcome::Failed(_) => {
                TakeoverStatus::CompletedWithWarnings
            }
            BootEntryOutcome::NotAttempted => TakeoverStatus::Incomplete,
        };
    }

    if phases.contains(&TakeoverPhase::Planning) || !phases.is_empty() {
        return TakeoverStatus::Running;
    }

    TakeoverStatus::Planning
}

fn next_operation_id(db_path: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let base = std::path::Path::new(db_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("conary");
    format!("{base}-{millis}-{pid}")
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        BootEntryOutcome, TakeoverInventory, TakeoverPhase, TakeoverRecord, TakeoverStatus,
        reduce_takeover_status,
    };

    #[test]
    fn test_takeover_dry_run_writes_planning_record_without_mutation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("conary.db");
        let record = TakeoverRecord::planned(
            db_path.to_str().expect("db path"),
            "generation",
            TakeoverInventory::default(),
            "rpm",
            "bls",
        );
        record
            .save(db_path.to_str().expect("db path"))
            .expect("save");

        let loaded = TakeoverRecord::load_latest_incomplete(db_path.to_str().expect("db path"))
            .expect("load latest")
            .expect("record");
        assert_eq!(loaded.status, TakeoverStatus::Planning);
        assert!(loaded.completed_phases.is_empty());
        assert_eq!(
            loaded.path(db_path.to_str().expect("db path")),
            PathBuf::from(temp.path())
                .join("takeover/operations")
                .join(format!("{}.json", loaded.id))
        );
    }

    #[test]
    fn test_takeover_ready_to_activate_status_after_generation_phase() {
        let status = reduce_takeover_status(
            &[
                TakeoverPhase::Planning,
                TakeoverPhase::Cas,
                TakeoverPhase::Owned,
                TakeoverPhase::Generation,
            ],
            BootEntryOutcome::Written,
            false,
        );
        assert_eq!(status, TakeoverStatus::ReadyToActivate);
    }

    #[test]
    fn test_takeover_warns_without_reporting_success_when_pm_removals_failed() {
        let status = reduce_takeover_status(
            &[
                TakeoverPhase::Planning,
                TakeoverPhase::Cas,
                TakeoverPhase::Owned,
            ],
            BootEntryOutcome::Skipped("pm failures".into()),
            true,
        );
        assert_eq!(status, TakeoverStatus::Incomplete);
    }

    #[test]
    fn test_takeover_boot_entry_failure_after_generation_is_completed_with_warnings() {
        let status = reduce_takeover_status(
            &[
                TakeoverPhase::Planning,
                TakeoverPhase::Cas,
                TakeoverPhase::Owned,
                TakeoverPhase::Generation,
            ],
            BootEntryOutcome::Failed("bls unavailable".into()),
            false,
        );
        assert_eq!(status, TakeoverStatus::CompletedWithWarnings);
    }
}
