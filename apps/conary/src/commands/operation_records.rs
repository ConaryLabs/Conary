// src/commands/operation_records.rs

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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

const PACKAGING_RECORD_RETENTION: usize = 50;

pub fn packaging_operations_dir_from_state_home(state_home: &Path) -> PathBuf {
    state_home
        .join("conary")
        .join("packaging")
        .join("operations")
}

#[allow(dead_code)]
pub fn default_packaging_operations_dir() -> Result<PathBuf> {
    if let Some(override_dir) = std::env::var_os("CONARY_PACKAGING_OPERATIONS_DIR") {
        return Ok(PathBuf::from(override_dir));
    }
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(packaging_operations_dir_from_state_home(&PathBuf::from(
            state_home,
        )));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("conary")
            .join("packaging")
            .join("operations"));
    }
    anyhow::bail!(
        "cannot determine packaging operation record directory; set XDG_STATE_HOME or HOME"
    )
}

#[allow(dead_code)]
pub(crate) fn write_packaging_record_unchecked<T: Serialize>(
    dir: &Path,
    operation_id: &str,
    value: &T,
) -> Result<PathBuf> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{operation_id}.json"));
    conary_core::filesystem::durable::write_json_atomic_with_mode(&path, value, 0o600)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    prune_packaging_records(dir, PACKAGING_RECORD_RETENTION)?;
    Ok(path)
}

#[allow(dead_code)]
pub fn list_packaging_records(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            records.push(path);
        }
    }
    records.sort();
    Ok(records)
}

#[allow(dead_code)]
pub fn load_latest_packaging_record<T: DeserializeOwned>(dir: &Path) -> Result<Option<T>> {
    let Some(path) = list_packaging_records(dir)?.pop() else {
        return Ok(None);
    };
    Ok(Some(load_json_record(&path)?))
}

fn prune_packaging_records(dir: &Path, keep: usize) -> Result<()> {
    let records = list_packaging_records(dir)?;
    if records.len() <= keep {
        return Ok(());
    }
    let remove_count = records.len() - keep;
    for path in records.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(())
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

    use super::{
        list_packaging_records, load_json_record, load_latest_packaging_record,
        packaging_operations_dir_from_state_home, takeover_operations_dir, write_json_record,
        write_packaging_record_unchecked,
    };

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

    #[test]
    fn packaging_operations_dir_uses_xdg_state_home() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir = packaging_operations_dir_from_state_home(temp.path());
        assert_eq!(dir, temp.path().join("conary/packaging/operations"));
    }

    #[test]
    fn write_packaging_record_uses_private_modes_and_prunes_old_records() {
        use std::os::unix::fs::PermissionsExt;

        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Fixture {
            schema_version: u16,
            operation_id: String,
        }

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path().join("ops");

        for index in 0..55 {
            write_packaging_record_unchecked(
                &dir,
                &format!("cook-{index:02}"),
                &Fixture {
                    schema_version: 1,
                    operation_id: format!("cook-{index:02}"),
                },
            )
            .unwrap();
        }

        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let latest = load_latest_packaging_record::<Fixture>(&dir)
            .unwrap()
            .unwrap();
        assert_eq!(latest.operation_id, "cook-54");
        let records = list_packaging_records(&dir).unwrap();
        assert_eq!(records.len(), 50);
        assert!(!dir.join("cook-00.json").exists());
        let mode = std::fs::metadata(dir.join("cook-54.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
