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

pub fn load_packaging_record_by_id<T: DeserializeOwned>(
    dir: &Path,
    operation_id: &str,
) -> Result<Option<T>> {
    validate_operation_id(operation_id)?;
    let path = dir.join(format!("{operation_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(load_json_record(&path)?))
}

pub fn load_latest_failed_packaging_record(
    dir: &Path,
) -> Result<Option<conary_core::diagnostics::PackagingCommandOutput>> {
    let mut records = list_packaging_records(dir)?;
    records.reverse();
    for path in records {
        let record: conary_core::diagnostics::PackagingCommandOutput = load_json_record(&path)?;
        if record.status == conary_core::diagnostics::PackagingCommandStatus::Failed {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

fn validate_operation_id(operation_id: &str) -> Result<()> {
    if operation_id.is_empty()
        || operation_id.contains('/')
        || operation_id.contains('\\')
        || operation_id.contains("..")
        || operation_id.contains('\0')
        || !operation_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        anyhow::bail!("invalid packaging operation id {operation_id:?}");
    }
    Ok(())
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
        list_packaging_records, load_json_record, load_latest_failed_packaging_record,
        load_latest_packaging_record, load_packaging_record_by_id,
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

    #[test]
    fn load_packaging_record_by_id_rejects_unsafe_ids_and_reads_safe_ids() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Fixture {
            operation_id: String,
            status: String,
        }

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path().join("ops");
        write_packaging_record_unchecked(
            &dir,
            "publish-1",
            &Fixture {
                operation_id: "publish-1".to_string(),
                status: "failed".to_string(),
            },
        )
        .unwrap();

        let loaded = load_packaging_record_by_id::<Fixture>(&dir, "publish-1")
            .unwrap()
            .expect("record");
        assert_eq!(loaded.operation_id, "publish-1");

        assert!(load_packaging_record_by_id::<Fixture>(&dir, "../publish-1").is_err());
        assert!(load_packaging_record_by_id::<Fixture>(&dir, "publish/1").is_err());
    }

    #[test]
    fn load_latest_failed_packaging_record_skips_successful_records() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path().join("ops");

        let ok =
            conary_core::diagnostics::PackagingCommandOutput::succeeded("cook-1", "conary cook");
        let failed = conary_core::diagnostics::PackagingCommandOutput::failed(
            "publish-2",
            "conary publish",
            vec![conary_core::diagnostics::PackagingDiagnostic::error(
                conary_core::diagnostics::PackagingPhase::Publish,
                conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed,
                "gate failed",
            )],
        );

        write_packaging_record_unchecked(&dir, "cook-1", &ok).unwrap();
        write_packaging_record_unchecked(&dir, "publish-2", &failed).unwrap();

        let loaded = load_latest_failed_packaging_record(&dir)
            .unwrap()
            .expect("failed record");
        assert_eq!(loaded.operation_id, "publish-2");
    }
}
