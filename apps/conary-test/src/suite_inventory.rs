// conary-test/src/suite_inventory.rs
//! Suite manifest inventory for local conary-test agent resources.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use conary_agent_contract::{
    InspectResult, OperationEnvelope, OperationStatus, RiskLevel, test_suites,
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuiteInventory {
    pub manifest_dir: String,
    pub dir_exists: bool,
    pub toml_files: usize,
    pub parsed: usize,
    pub failed: usize,
    pub suites: Vec<SuiteInventoryEntry>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuiteInventoryEntry {
    pub id: String,
    pub name: String,
    pub phase: u32,
    pub test_count: usize,
    pub requires_container_runtime: bool,
    pub requires_qemu: bool,
    pub qemu_only: bool,
}

pub fn inspect_manifest_dir(manifest_dir: &Path) -> InspectResult {
    let inventory = read_suite_inventory(manifest_dir);
    let mut envelope = OperationEnvelope::new(
        "conary-test.suites.inspect",
        OperationStatus::Ok,
        RiskLevel::ReadOnly,
        "Known conary-test suite manifests inspected",
    );
    envelope.subject = Some(test_suites());

    if !inventory.dir_exists {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is missing: {}",
            manifest_dir.display()
        ));
    } else if inventory
        .errors
        .iter()
        .any(|error| error.contains("unreadable"))
    {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is unreadable: {}",
            manifest_dir.display()
        ));
    } else if inventory.parsed == 0 {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "no parseable test manifests found in {}",
            manifest_dir.display()
        ));
    } else if inventory.failed > 0 {
        envelope.status = OperationStatus::Partial;
        envelope.warnings.push(format!(
            "{} test manifest(s) failed to parse in {}",
            inventory.failed,
            manifest_dir.display()
        ));
    }

    let data = serde_json::to_value(&inventory).expect("suite inventory should serialize to JSON");
    InspectResult::new(envelope).with_data(data)
}

pub fn read_suite_inventory(manifest_dir: &Path) -> SuiteInventory {
    let dir_exists = manifest_dir.is_dir();
    let mut inventory = SuiteInventory {
        manifest_dir: manifest_dir.display().to_string(),
        dir_exists,
        toml_files: 0,
        parsed: 0,
        failed: 0,
        suites: Vec::new(),
        errors: Vec::new(),
    };

    if !dir_exists {
        return inventory;
    }

    if directory_has_no_read_bits(manifest_dir) {
        inventory.errors.push(format!(
            "{}: manifest directory is unreadable",
            manifest_dir.display()
        ));
        return inventory;
    }

    let entries = match std::fs::read_dir(manifest_dir) {
        Ok(entries) => entries,
        Err(error) => {
            inventory.errors.push(format!(
                "{}: manifest directory is unreadable: {error}",
                manifest_dir.display()
            ));
            return inventory;
        }
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    for path in paths {
        inventory.toml_files += 1;
        let id = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string();

        match crate::config::load_manifest(&path) {
            Ok(manifest) => {
                inventory.parsed += 1;
                let requires_qemu = manifest_requires_qemu(&manifest);
                let qemu_only = manifest.is_qemu_only();
                inventory.suites.push(SuiteInventoryEntry {
                    id,
                    name: manifest.suite.name,
                    phase: manifest.suite.phase,
                    test_count: manifest.test.len(),
                    requires_container_runtime: !qemu_only,
                    requires_qemu,
                    qemu_only,
                });
            }
            Err(error) => {
                inventory.failed += 1;
                let file = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("<unknown>");
                inventory.errors.push(format!("{file}: {error}"));
            }
        }
    }

    inventory
        .suites
        .sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

fn manifest_requires_qemu(manifest: &crate::config::TestManifest) -> bool {
    manifest
        .suite
        .setup
        .iter()
        .any(|step| step.qemu_boot.is_some())
        || manifest
            .test
            .iter()
            .any(|test| test.step.iter().any(|step| step.qemu_boot.is_some()))
}

#[cfg(unix)]
fn directory_has_no_read_bits(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    // Early-exit for directories intentionally made unreadable in tests. When
    // read bits are present, the caller falls through to read_dir so OS errors
    // still become inventory state instead of transport errors.
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o444 == 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn directory_has_no_read_bits(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use conary_agent_contract::{OperationStatus, RiskLevel};
    use tempfile::tempdir;

    use super::*;

    fn write_manifest(dir: &Path, file_name: &str, body: &str) {
        std::fs::write(dir.join(file_name), body).unwrap();
    }

    fn container_manifest(name: &str, phase: u32) -> String {
        format!(
            r#"
[suite]
name = "{name}"
phase = {phase}

[[test]]
id = "T01"
name = "container_test"
description = "Container test"
timeout = 10

[[test.step]]
run = "true"
"#
        )
    }

    fn qemu_manifest(name: &str, phase: u32) -> String {
        format!(
            r#"
[suite]
name = "{name}"
phase = {phase}

[[test]]
id = "TQEMU"
name = "qemu_test"
description = "QEMU test"
timeout = 10

[[test.step]]
[test.step.qemu_boot]
image = "unused"
local_image_path = "/tmp/missing.qcow2"
commands = ["true"]
"#
        )
    }

    #[test]
    fn inventory_reads_valid_manifests_and_flags() {
        let root = tempdir().unwrap();
        write_manifest(
            root.path(),
            "phase2-container.toml",
            &container_manifest("container", 2),
        );
        write_manifest(root.path(), "phase3-qemu.toml", &qemu_manifest("qemu", 3));

        let inspect = inspect_manifest_dir(root.path());
        let suites = inspect.data["suites"].as_array().unwrap();

        assert_eq!(inspect.envelope.operation, "conary-test.suites.inspect");
        assert_eq!(inspect.envelope.status, OperationStatus::Ok);
        assert_eq!(inspect.envelope.risk, RiskLevel::ReadOnly);
        assert_eq!(
            inspect.envelope.subject.as_ref().unwrap().uri,
            "conary-test://suites"
        );
        assert_eq!(inspect.data["dir_exists"], true);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 2);
        assert_eq!(inspect.data["failed"], 0);
        assert_eq!(suites.len(), 2);
        assert_eq!(suites[0]["id"], "phase2-container");
        assert_eq!(suites[0]["requires_container_runtime"], true);
        assert_eq!(suites[0]["requires_qemu"], false);
        assert_eq!(suites[0]["qemu_only"], false);
        assert_eq!(suites[1]["id"], "phase3-qemu");
        assert_eq!(suites[1]["requires_container_runtime"], false);
        assert_eq!(suites[1]["requires_qemu"], true);
        assert_eq!(suites[1]["qemu_only"], true);
    }

    #[test]
    fn inventory_sorts_suites_by_id() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "z-suite.toml", &container_manifest("z", 1));
        write_manifest(root.path(), "a-suite.toml", &container_manifest("a", 1));

        let inspect = inspect_manifest_dir(root.path());
        let suites = inspect.data["suites"].as_array().unwrap();

        assert_eq!(suites[0]["id"], "a-suite");
        assert_eq!(suites[1]["id"], "z-suite");
    }

    #[test]
    fn invalid_toml_is_partial_when_one_manifest_parses() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "good.toml", &container_manifest("good", 1));
        write_manifest(root.path(), "bad.toml", "not = [valid");

        let inspect = inspect_manifest_dir(root.path());

        assert_eq!(inspect.envelope.status, OperationStatus::Partial);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 1);
        assert_eq!(inspect.data["failed"], 1);
        assert!(
            inspect.data["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("bad.toml"))
        );
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("failed to parse"))
        );
    }

    #[test]
    fn all_invalid_toml_is_unavailable_when_no_manifest_parses() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "bad-a.toml", "not = [valid");
        write_manifest(root.path(), "bad-b.toml", "also = [broken");

        let inspect = inspect_manifest_dir(root.path());

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 0);
        assert_eq!(inspect.data["failed"], 2);
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("no parseable test manifests"))
        );
    }

    #[test]
    fn missing_manifest_dir_is_unavailable() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing");

        let inspect = inspect_manifest_dir(&missing);

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["dir_exists"], false);
        assert_eq!(inspect.data["parsed"], 0);
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory is missing"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_manifest_dir_is_unavailable_with_error_data() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let original_permissions = std::fs::metadata(&blocked).unwrap().permissions();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let inspect = inspect_manifest_dir(&blocked);

        std::fs::set_permissions(&blocked, original_permissions).unwrap();

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["dir_exists"], true);
        assert!(
            inspect.data["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("unreadable"))
        );
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("unreadable"))
        );
    }
}
