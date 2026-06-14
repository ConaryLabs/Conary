// apps/conary/src/commands/hermetic_state.rs

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use conary_core::ccs::CcsPackage;
use conary_core::packages::PackageFormat;
use conary_core::recipe::hermetic::HostBuildRecord;
use conary_core::recipe::{CookResult, Recipe};

const HOST_RECORDS_DIR: &str = "host-build-records";

#[derive(Debug, Default)]
pub(crate) struct HostBuildRecordLookup {
    pub(crate) record: Option<HostBuildRecord>,
    pub(crate) diagnostics: Vec<String>,
}

pub(crate) fn resolve_default_state_dir() -> Result<PathBuf> {
    resolve_state_dir_from_env(|key| std::env::var_os(key))
}

fn resolve_state_dir_from_env(var: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = non_empty_os(var("CONARY_HERMETIC_STATE_DIR")) {
        return Ok(PathBuf::from(path));
    }

    if let Some(path) = non_empty_os(var("XDG_STATE_HOME")) {
        return Ok(PathBuf::from(path).join("conary").join("hermetic"));
    }

    if let Some(path) = non_empty_os(var("HOME")) {
        return Ok(PathBuf::from(path)
            .join(".local")
            .join("state")
            .join("conary")
            .join("hermetic"));
    }

    bail!(
        "cannot determine hermetic state directory; set CONARY_HERMETIC_STATE_DIR, XDG_STATE_HOME, or HOME"
    )
}

fn non_empty_os(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

pub(crate) fn write_host_build_record_to_dir(
    state_dir: &Path,
    record: &HostBuildRecord,
) -> Result<PathBuf> {
    let records_dir = state_dir.join(HOST_RECORDS_DIR);
    fs::create_dir_all(&records_dir).with_context(|| {
        format!(
            "create host build record directory {}",
            records_dir.display()
        )
    })?;
    let path = records_dir.join(record_filename(record));
    let bytes = serde_json::to_vec_pretty(record)?;
    fs::write(&path, bytes)
        .with_context(|| format!("write host build record {}", path.display()))?;
    Ok(path)
}

pub(crate) fn load_latest_host_build_record_from_dir(
    state_dir: &Path,
    package_name: &str,
    package_version: &str,
    package_release: &str,
    architecture: Option<&str>,
) -> HostBuildRecordLookup {
    let records_dir = state_dir.join(HOST_RECORDS_DIR);
    let mut lookup = HostBuildRecordLookup::default();
    let entries = match fs::read_dir(&records_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            lookup.diagnostics.push(format!(
                "no host record directory found at {}",
                records_dir.display()
            ));
            return lookup;
        }
        Err(error) => {
            lookup.diagnostics.push(format!(
                "failed to read host record directory {}: {error}",
                records_dir.display()
            ));
            return lookup;
        }
    };

    let mut latest: Option<(i128, HostBuildRecord)> = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                lookup
                    .diagnostics
                    .push(format!("failed to read host record entry: {error}"));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                lookup.diagnostics.push(format!(
                    "failed to read host record {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        let record: HostBuildRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                lookup.diagnostics.push(format!(
                    "failed to parse host record {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if !record_matches(
            &record,
            package_name,
            package_version,
            package_release,
            architecture,
        ) {
            continue;
        }

        let sort_key = record_sort_key(&path, &record, &mut lookup.diagnostics);
        if latest
            .as_ref()
            .is_none_or(|(latest_key, _)| sort_key > *latest_key)
        {
            latest = Some((sort_key, record));
        }
    }

    lookup.record = latest.map(|(_, record)| record);
    if lookup.record.is_none() && lookup.diagnostics.is_empty() {
        lookup
            .diagnostics
            .push("no matching host record found".to_string());
    }
    lookup
}

pub(crate) fn load_latest_host_build_record_for_recipe(
    state_dir: &Path,
    recipe: &Recipe,
    architecture: Option<&str>,
) -> HostBuildRecordLookup {
    load_latest_host_build_record_from_dir(
        state_dir,
        &recipe.package.name,
        &recipe.package.version,
        &recipe.package.release,
        architecture,
    )
}

pub(crate) fn host_build_record_from_cook_result(
    recipe: &Recipe,
    result: &CookResult,
) -> Option<HostBuildRecord> {
    let provenance = result.provenance.as_ref()?;
    let output_merkle_root = provenance.merkle_root.clone()?;
    Some(HostBuildRecord {
        package_name: recipe.package.name.clone(),
        package_version: recipe.package.version.clone(),
        package_release: recipe.package.release.clone(),
        architecture: cooked_package_architecture(&result.package_path)
            .or_else(|| provenance.host_arch.clone()),
        output_merkle_root,
        diagnostic_input_key: provenance.recipe_hash.clone(),
        diagnostic_dna_hash: provenance.dna_hash.clone(),
        package_path: Some(result.package_path.to_string_lossy().to_string()),
        build_timestamp: provenance.build_timestamp.clone(),
    })
}

fn record_matches(
    record: &HostBuildRecord,
    package_name: &str,
    package_version: &str,
    package_release: &str,
    architecture: Option<&str>,
) -> bool {
    record.package_name == package_name
        && record.package_version == package_version
        && record.package_release == package_release
        && record.architecture.as_deref() == architecture
}

fn cooked_package_architecture(package_path: &Path) -> Option<String> {
    let package = CcsPackage::parse(&package_path.to_string_lossy()).ok()?;
    package
        .manifest()
        .package
        .platform
        .as_ref()
        .and_then(|platform| platform.arch.clone())
}

fn record_sort_key(path: &Path, record: &HostBuildRecord, diagnostics: &mut Vec<String>) -> i128 {
    if let Some(timestamp) = &record.build_timestamp {
        match DateTime::parse_from_rfc3339(timestamp) {
            Ok(parsed) => return parsed.with_timezone(&Utc).timestamp_millis() as i128 * 1_000_000,
            Err(error) => diagnostics.push(format!(
                "host record {} has malformed build_timestamp {timestamp:?}: {error}",
                path.display()
            )),
        }
    }

    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(system_time_nanos_since_epoch)
        .unwrap_or(0)
}

fn system_time_nanos_since_epoch(time: SystemTime) -> Option<i128> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_nanos() as i128)
}

fn record_filename(record: &HostBuildRecord) -> String {
    let arch = record.architecture.as_deref().unwrap_or("noarch");
    let stamp = record
        .build_timestamp
        .as_deref()
        .map(sanitize_filename)
        .unwrap_or_else(now_suffix);
    format!(
        "{}-{}-{}-{}-{stamp}.json",
        sanitize_filename(&record.package_name),
        sanitize_filename(&record.package_version),
        sanitize_filename(&record.package_release),
        sanitize_filename(arch)
    )
}

fn now_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn sanitize_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::recipe::hermetic::HostBuildRecord;

    fn record(version: &str, arch: Option<&str>, merkle: &str) -> HostBuildRecord {
        HostBuildRecord {
            package_name: "pkg".to_string(),
            package_version: version.to_string(),
            package_release: "1".to_string(),
            architecture: arch.map(str::to_string),
            output_merkle_root: merkle.to_string(),
            diagnostic_input_key: Some("sha256:input".to_string()),
            diagnostic_dna_hash: Some("sha256:dna".to_string()),
            package_path: Some("/tmp/pkg.ccs".to_string()),
            build_timestamp: None,
        }
    }

    #[test]
    fn explicit_state_dir_write_read_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let mut host = record("1.0", Some("x86_64"), "sha256:host");
        host.build_timestamp = Some("2026-06-14T10:00:00Z".to_string());

        write_host_build_record_to_dir(temp.path(), &host).unwrap();
        let lookup =
            load_latest_host_build_record_from_dir(temp.path(), "pkg", "1.0", "1", Some("x86_64"));

        assert!(lookup.diagnostics.is_empty());
        assert_eq!(lookup.record, Some(host));
    }

    #[test]
    fn latest_matching_record_wins_by_timestamp() {
        let temp = tempfile::tempdir().unwrap();
        let mut older = record("1.0", Some("x86_64"), "sha256:old");
        older.build_timestamp = Some("2026-06-14T10:00:00Z".to_string());
        let mut newer = record("1.0", Some("x86_64"), "sha256:new");
        newer.build_timestamp = Some("2026-06-14T11:00:00Z".to_string());

        write_host_build_record_to_dir(temp.path(), &older).unwrap();
        write_host_build_record_to_dir(temp.path(), &newer).unwrap();
        let lookup =
            load_latest_host_build_record_from_dir(temp.path(), "pkg", "1.0", "1", Some("x86_64"));

        assert_eq!(lookup.record.unwrap().output_merkle_root, "sha256:new");
    }

    #[test]
    fn malformed_records_are_diagnostics_only() {
        let temp = tempfile::tempdir().unwrap();
        let records_dir = temp.path().join("host-build-records");
        std::fs::create_dir_all(&records_dir).unwrap();
        std::fs::write(records_dir.join("bad.json"), b"not json").unwrap();

        let lookup = load_latest_host_build_record_from_dir(temp.path(), "pkg", "1.0", "1", None);

        assert!(lookup.record.is_none());
        assert!(lookup.diagnostics.iter().any(|d| d.contains("bad.json")));
    }

    #[test]
    fn architecture_participates_in_match_key() {
        let temp = tempfile::tempdir().unwrap();
        write_host_build_record_to_dir(
            temp.path(),
            &record("1.0", Some("aarch64"), "sha256:wrong-arch"),
        )
        .unwrap();
        write_host_build_record_to_dir(
            temp.path(),
            &record("1.0", Some("x86_64"), "sha256:right-arch"),
        )
        .unwrap();

        let lookup =
            load_latest_host_build_record_from_dir(temp.path(), "pkg", "1.0", "1", Some("x86_64"));

        assert_eq!(
            lookup.record.unwrap().output_merkle_root,
            "sha256:right-arch"
        );
    }

    #[test]
    fn state_dir_resolution_prefers_explicit_env_then_xdg_then_home() {
        let explicit = resolve_state_dir_from_env(|key| match key {
            "CONARY_HERMETIC_STATE_DIR" => Some("/state/explicit".into()),
            "XDG_STATE_HOME" => Some("/state/xdg".into()),
            "HOME" => Some("/home/test".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(explicit, PathBuf::from("/state/explicit"));

        let xdg = resolve_state_dir_from_env(|key| match key {
            "XDG_STATE_HOME" => Some("/state/xdg".into()),
            "HOME" => Some("/home/test".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(xdg, PathBuf::from("/state/xdg/conary/hermetic"));

        let home = resolve_state_dir_from_env(|key| match key {
            "HOME" => Some("/home/test".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            home,
            PathBuf::from("/home/test/.local/state/conary/hermetic")
        );
    }
}
