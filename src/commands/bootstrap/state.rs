// src/commands/bootstrap/state.rs

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::commands::operation_records::{
    bootstrap_operations_dir, load_json_record, write_json_record,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapRunRecord {
    pub id: String,
    pub work_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub recipe_dir: PathBuf,
    pub seed_id: String,
    pub up_to: Option<String>,
    pub only: Vec<String>,
    pub cascade: bool,
    pub derivation_db_path: PathBuf,
    pub output_dir: PathBuf,
    pub generation_dir: Option<PathBuf>,
    pub profile_hash: Option<String>,
    pub completed_successfully: bool,
}

impl BootstrapRunRecord {
    #[must_use]
    pub fn started(
        id: String,
        work_dir: PathBuf,
        manifest_path: PathBuf,
        recipe_dir: PathBuf,
        seed_id: String,
    ) -> Self {
        let op_dir = bootstrap_operations_dir(&work_dir).join(&id);
        Self {
            id,
            work_dir,
            manifest_path,
            recipe_dir,
            seed_id,
            up_to: None,
            only: Vec::new(),
            cascade: false,
            derivation_db_path: op_dir.join("derivations.db"),
            output_dir: op_dir.join("output"),
            generation_dir: None,
            profile_hash: None,
            completed_successfully: false,
        }
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        bootstrap_operations_dir(&self.work_dir).join(format!("{}.json", self.id))
    }

    pub fn save(&self) -> Result<()> {
        write_json_record(&self.path(), self)
    }

    pub fn load(path: &Path) -> Result<Self> {
        load_json_record(path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapLatestPointer {
    pub operation_id: String,
    pub record_path: PathBuf,
}

impl BootstrapLatestPointer {
    #[must_use]
    pub fn new(operation_id: impl Into<String>, record_path: PathBuf) -> Self {
        Self {
            operation_id: operation_id.into(),
            record_path,
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_record(path, self)
    }

    pub fn load(path: &Path) -> Result<Self> {
        load_json_record(path)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::BootstrapRunRecord;
    use crate::commands::operation_records::{load_json_record, write_json_record};

    fn round_trip_record(record: &BootstrapRunRecord) -> anyhow::Result<BootstrapRunRecord> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("record.json");
        write_json_record(&path, record)?;
        load_json_record(&path)
    }

    #[test]
    fn test_bootstrap_run_record_round_trips_json() {
        let mut record = BootstrapRunRecord::started(
            "op-123".into(),
            PathBuf::from("/tmp/work"),
            PathBuf::from("/tmp/system.toml"),
            PathBuf::from("/tmp/recipes"),
            "seed-abc".into(),
        );
        record.derivation_db_path =
            PathBuf::from("/tmp/work/operations/op-123/derivations.db");
        record.output_dir = PathBuf::from("/tmp/work/operations/op-123/output");
        record.completed_successfully = true;
        let loaded = round_trip_record(&record).expect("round trip");
        assert_eq!(loaded.seed_id, "seed-abc");
        assert!(loaded.completed_successfully);
    }
}
