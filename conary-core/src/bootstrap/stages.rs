// conary-core/src/bootstrap/stages.rs

//! Bootstrap stage management and progress tracking
//!
//! Tracks which stages have been completed and allows resuming
//! bootstrap from the last successful stage.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Bootstrap stages in order
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BootstrapStage {
    /// Phase 1: Cross-toolchain (LFS Ch5)
    CrossTools,
    /// Phase 2: Temporary tools (LFS Ch6-7)
    TempTools,
    /// Phase 3: Final system (LFS Ch8)
    FinalSystem,
    /// Phase 4: System configuration (LFS Ch9)
    SystemConfig,
    /// Phase 5: Bootable image (LFS Ch10)
    BootableImage,
    /// Phase 6: Tier 2 -- BLFS + Conary
    Tier2,
}

impl BootstrapStage {
    /// Get all stages in order
    pub fn all() -> &'static [BootstrapStage] {
        &[
            Self::CrossTools,
            Self::TempTools,
            Self::FinalSystem,
            Self::SystemConfig,
            Self::BootableImage,
            Self::Tier2,
        ]
    }

    /// Get the next stage after this one
    pub fn next(&self) -> Option<BootstrapStage> {
        match self {
            Self::CrossTools => Some(Self::TempTools),
            Self::TempTools => Some(Self::FinalSystem),
            Self::FinalSystem => Some(Self::SystemConfig),
            Self::SystemConfig => Some(Self::BootableImage),
            Self::BootableImage => Some(Self::Tier2),
            Self::Tier2 => None,
        }
    }

    /// Get the previous stage before this one
    pub fn previous(&self) -> Option<BootstrapStage> {
        match self {
            Self::CrossTools => None,
            Self::TempTools => Some(Self::CrossTools),
            Self::FinalSystem => Some(Self::TempTools),
            Self::SystemConfig => Some(Self::FinalSystem),
            Self::BootableImage => Some(Self::SystemConfig),
            Self::Tier2 => Some(Self::BootableImage),
        }
    }

    /// Get a human-readable name for the stage
    pub fn name(&self) -> &'static str {
        match self {
            Self::CrossTools => "Phase 1: Cross-toolchain (LFS Ch5)",
            Self::TempTools => "Phase 2: Temporary tools (LFS Ch6-7)",
            Self::FinalSystem => "Phase 3: Final system (LFS Ch8)",
            Self::SystemConfig => "Phase 4: System configuration (LFS Ch9)",
            Self::BootableImage => "Phase 5: Bootable image (LFS Ch10)",
            Self::Tier2 => "Phase 6: Tier 2 (BLFS + Conary)",
        }
    }

    /// Check if this stage is required (vs optional)
    pub fn is_required(&self) -> bool {
        // Tier 2 is optional -- a minimal LFS system can boot without it
        !matches!(self, Self::Tier2)
    }
}

impl std::fmt::Display for BootstrapStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// State of a single stage
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageState {
    /// Whether this stage is complete
    pub complete: bool,

    /// Timestamp when completed (if complete)
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Path to artifacts produced by this stage
    pub artifact_path: Option<PathBuf>,

    /// Error message if stage failed
    pub error: Option<String>,

    /// Duration of the build in seconds
    pub duration_secs: Option<u64>,

    /// Packages completed within this stage (for per-package checkpointing)
    #[serde(default)]
    pub completed_packages: Vec<String>,
}

/// Manager for tracking bootstrap progress
#[derive(Debug, Serialize, Deserialize)]
pub struct StageManager {
    /// State of each stage
    stages: HashMap<BootstrapStage, StageState>,

    /// Path to the state file
    #[serde(skip)]
    state_file: PathBuf,
}

impl StageManager {
    /// Create a new stage manager
    pub fn new(work_dir: impl AsRef<Path>) -> Result<Self> {
        let state_file = work_dir.as_ref().join("bootstrap-state.json");

        // Try to load existing state
        if state_file.exists() {
            let content = std::fs::read_to_string(&state_file)
                .context("Failed to read bootstrap state file")?;
            let mut manager: StageManager =
                serde_json::from_str(&content).context("Failed to parse bootstrap state")?;
            manager.state_file = state_file;
            return Ok(manager);
        }

        // Create new state
        let mut stages = HashMap::new();
        for stage in BootstrapStage::all() {
            stages.insert(*stage, StageState::default());
        }

        Ok(Self { stages, state_file })
    }

    /// Get the state of a stage
    pub fn get(&self, stage: BootstrapStage) -> Result<&StageState> {
        self.stages
            .get(&stage)
            .ok_or_else(|| anyhow::anyhow!("stage not found in tracker: {stage:?}"))
    }

    /// Check if a stage is complete
    pub fn is_complete(&self, stage: BootstrapStage) -> bool {
        self.get(stage).is_ok_and(|s| s.complete)
    }

    /// Get the current (next incomplete) stage
    ///
    /// Returns `None` when all stages are complete.
    pub fn current_stage(&self) -> Result<Option<BootstrapStage>> {
        for stage in BootstrapStage::all() {
            if !self.is_complete(*stage) {
                return Ok(Some(*stage));
            }
        }
        Ok(None)
    }

    /// Get artifact path for a completed stage
    pub fn get_artifact_path(&self, stage: BootstrapStage) -> Option<PathBuf> {
        self.get(stage).ok().and_then(|s| s.artifact_path.clone())
    }

    /// Mark a stage as complete
    pub fn mark_complete(
        &mut self,
        stage: BootstrapStage,
        artifact_path: impl AsRef<Path>,
    ) -> Result<()> {
        let state = self
            .stages
            .get_mut(&stage)
            .ok_or_else(|| anyhow::anyhow!("stage not found in tracker: {stage:?}"))?;
        state.complete = true;
        state.completed_at = Some(chrono::Utc::now());
        state.artifact_path = Some(artifact_path.as_ref().to_path_buf());
        state.error = None;

        self.save()
    }

    /// Mark a stage as failed
    pub fn mark_failed(&mut self, stage: BootstrapStage, error: impl Into<String>) -> Result<()> {
        let state = self
            .stages
            .get_mut(&stage)
            .ok_or_else(|| anyhow::anyhow!("stage not found in tracker: {stage:?}"))?;
        state.complete = false;
        state.error = Some(error.into());

        self.save()
    }

    /// Record build duration for a stage
    pub fn record_duration(&mut self, stage: BootstrapStage, duration_secs: u64) -> Result<()> {
        let state = self
            .stages
            .get_mut(&stage)
            .ok_or_else(|| anyhow::anyhow!("stage not found in tracker: {stage:?}"))?;
        state.duration_secs = Some(duration_secs);
        self.save()
    }

    /// Record a completed package within a stage.
    ///
    /// This enables per-package checkpointing so that a resumed build can
    /// skip packages that were already successfully built.
    pub fn mark_package_complete(&mut self, stage: BootstrapStage, package: &str) -> Result<()> {
        let state = self
            .stages
            .get_mut(&stage)
            .ok_or_else(|| anyhow::anyhow!("stage not found in tracker: {stage:?}"))?;
        if !state.completed_packages.contains(&package.to_string()) {
            state.completed_packages.push(package.to_string());
        }
        self.save()
    }

    /// Get list of completed packages for a stage.
    pub fn completed_packages(&self, stage: BootstrapStage) -> Vec<String> {
        self.stages
            .get(&stage)
            .map(|s| s.completed_packages.clone())
            .unwrap_or_default()
    }

    /// Reset a stage (and all subsequent stages)
    pub fn reset_from(&mut self, stage: BootstrapStage) -> Result<()> {
        let mut current = Some(stage);
        while let Some(s) = current {
            let state = self
                .stages
                .get_mut(&s)
                .ok_or_else(|| anyhow::anyhow!("stage not found in tracker: {s:?}"))?;
            *state = StageState::default();
            current = s.next();
        }
        self.save()
    }

    /// Get a summary of all stages
    pub fn summary(&self) -> Vec<(BootstrapStage, bool, Option<String>)> {
        BootstrapStage::all()
            .iter()
            .filter_map(|s| {
                let state = self.get(*s).ok()?;
                let status = if state.complete {
                    Some("complete".to_string())
                } else {
                    state.error.as_ref().map(|err| format!("failed: {}", err))
                };
                Some((*s, state.complete, status))
            })
            .collect()
    }

    /// Save state to disk atomically (write to temp file, then rename)
    fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self)?;
        let tmp_path = self.state_file.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content).context("Failed to write temporary state file")?;
        std::fs::rename(&tmp_path, &self.state_file)
            .context("Failed to atomically rename state file")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_ordering() {
        assert!(BootstrapStage::CrossTools < BootstrapStage::TempTools);
        assert!(BootstrapStage::TempTools < BootstrapStage::FinalSystem);
        assert!(BootstrapStage::FinalSystem < BootstrapStage::Tier2);
    }

    #[test]
    fn test_stage_next() {
        assert_eq!(
            BootstrapStage::CrossTools.next(),
            Some(BootstrapStage::TempTools)
        );
        assert_eq!(
            BootstrapStage::TempTools.next(),
            Some(BootstrapStage::FinalSystem)
        );
        assert_eq!(BootstrapStage::Tier2.next(), None);
    }

    #[test]
    fn test_stage_previous() {
        assert_eq!(BootstrapStage::CrossTools.previous(), None);
        assert_eq!(
            BootstrapStage::TempTools.previous(),
            Some(BootstrapStage::CrossTools)
        );
        assert_eq!(
            BootstrapStage::Tier2.previous(),
            Some(BootstrapStage::BootableImage)
        );
    }

    #[test]
    fn test_stage_manager_new() {
        let temp = tempfile::tempdir().unwrap();
        let manager = StageManager::new(temp.path()).unwrap();

        // All stages should exist and be incomplete
        for stage in BootstrapStage::all() {
            assert!(!manager.is_complete(*stage));
        }
    }

    #[test]
    fn test_stage_manager_mark_complete() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = StageManager::new(temp.path()).unwrap();

        manager
            .mark_complete(BootstrapStage::CrossTools, "/tools")
            .unwrap();

        assert!(manager.is_complete(BootstrapStage::CrossTools));
        assert_eq!(
            manager.get_artifact_path(BootstrapStage::CrossTools),
            Some(PathBuf::from("/tools"))
        );
    }

    #[test]
    fn test_stage_manager_persistence() {
        let temp = tempfile::tempdir().unwrap();

        // Create manager and mark a stage complete
        {
            let mut manager = StageManager::new(temp.path()).unwrap();
            manager
                .mark_complete(BootstrapStage::CrossTools, "/tools")
                .unwrap();
        }

        // Load again and verify
        {
            let manager = StageManager::new(temp.path()).unwrap();
            assert!(manager.is_complete(BootstrapStage::CrossTools));
        }
    }

    #[test]
    fn test_stage_manager_package_checkpointing() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = StageManager::new(temp.path()).unwrap();

        // Mark some packages complete in the FinalSystem stage
        manager
            .mark_package_complete(BootstrapStage::FinalSystem, "zlib")
            .unwrap();
        manager
            .mark_package_complete(BootstrapStage::FinalSystem, "ncurses")
            .unwrap();

        let completed = manager.completed_packages(BootstrapStage::FinalSystem);
        assert_eq!(completed.len(), 2);
        assert!(completed.contains(&"zlib".to_string()));
        assert!(completed.contains(&"ncurses".to_string()));

        // Other stages should have empty package lists
        let cross_pkgs = manager.completed_packages(BootstrapStage::CrossTools);
        assert!(cross_pkgs.is_empty());
    }

    #[test]
    fn test_stage_manager_package_checkpointing_persistence() {
        let temp = tempfile::tempdir().unwrap();

        {
            let mut manager = StageManager::new(temp.path()).unwrap();
            manager
                .mark_package_complete(BootstrapStage::FinalSystem, "bash")
                .unwrap();
        }

        // Reload and verify
        {
            let manager = StageManager::new(temp.path()).unwrap();
            let completed = manager.completed_packages(BootstrapStage::FinalSystem);
            assert_eq!(completed, vec!["bash"]);
        }
    }

    #[test]
    fn test_stage_manager_package_no_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = StageManager::new(temp.path()).unwrap();

        manager
            .mark_package_complete(BootstrapStage::FinalSystem, "zlib")
            .unwrap();
        manager
            .mark_package_complete(BootstrapStage::FinalSystem, "zlib")
            .unwrap();

        let completed = manager.completed_packages(BootstrapStage::FinalSystem);
        assert_eq!(completed.len(), 1);
    }

    #[test]
    fn test_stage_manager_reset() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = StageManager::new(temp.path()).unwrap();

        // Mark several stages complete
        manager
            .mark_complete(BootstrapStage::CrossTools, "/tools")
            .unwrap();
        manager
            .mark_complete(BootstrapStage::TempTools, "/temp-tools")
            .unwrap();
        manager
            .mark_complete(BootstrapStage::FinalSystem, "/system")
            .unwrap();

        // Reset from TempTools
        manager.reset_from(BootstrapStage::TempTools).unwrap();

        // CrossTools should still be complete
        assert!(manager.is_complete(BootstrapStage::CrossTools));
        // TempTools and later should be reset
        assert!(!manager.is_complete(BootstrapStage::TempTools));
        assert!(!manager.is_complete(BootstrapStage::FinalSystem));
    }
}
