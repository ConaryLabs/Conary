// src/commands/operation_records.rs

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

pub fn write_json_record<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    conary_core::filesystem::durable::write_json_atomic(path, value)
        .map_err(|error| anyhow::anyhow!("{error}"))
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

#[must_use]
pub fn new_operation_id(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    format!("{prefix}-{millis}-{pid}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{load_json_record, takeover_operations_dir, write_json_record};

    #[test]
    fn test_takeover_operations_dir_uses_db_dir() {
        let dir = takeover_operations_dir("/tmp/conary-test/conary.db");
        assert_eq!(dir, PathBuf::from("/tmp/conary-test/takeover/operations"));
    }

    #[test]
    fn operation_record_write_leaves_no_tmp_file() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Fixture {
            value: String,
        }

        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("record.json");
        write_json_record(
            &path,
            &Fixture {
                value: "ok".to_string(),
            },
        )
        .unwrap();

        assert!(!temp.path().join("record.json.tmp").exists());
        let loaded: Fixture = load_json_record(&path).unwrap();
        assert_eq!(loaded.value, "ok");
    }
}
