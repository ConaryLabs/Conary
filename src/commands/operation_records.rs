// src/commands/operation_records.rs

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationEnvelope<T> {
    pub id: String,
    pub kind: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub payload: T,
}

pub fn write_json_record<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn load_json_record<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

pub fn takeover_operations_dir(db_path: &str) -> PathBuf {
    conary_core::db::paths::db_dir(db_path)
        .join("takeover")
        .join("operations")
}

pub fn bootstrap_operations_dir(work_dir: &Path) -> PathBuf {
    work_dir.join("operations")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::takeover_operations_dir;

    #[test]
    fn test_takeover_operations_dir_uses_db_dir() {
        let dir = takeover_operations_dir("/tmp/conary-test/conary.db");
        assert_eq!(dir, PathBuf::from("/tmp/conary-test/takeover/operations"));
    }
}
