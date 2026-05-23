// conary-test/src/bootstrap.rs
//! Local developer bootstrap inspection for conary-test.

use std::path::{Path, PathBuf};

use conary_agent_contract::{
    EvidenceItem, EvidenceKind, InspectResult, OperationEnvelope, OperationStatus, RiskLevel,
    local_bootstrap_status,
};

pub fn inspect_default() -> InspectResult {
    let root = crate::paths::project_dir()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let manifests = std::env::var_os("CONARY_TEST_MANIFESTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("apps/conary/tests/integration/remi/manifests"));

    inspect_with_paths(&root, &manifests)
}

pub fn inspect_with_paths(root: &Path, manifest_dir: &Path) -> InspectResult {
    let mut envelope = OperationEnvelope::new(
        "conary-test.bootstrap.inspect",
        OperationStatus::Ok,
        RiskLevel::ReadOnly,
        "Local Conary developer bootstrap prerequisites inspected",
    );
    envelope.subject = Some(local_bootstrap_status());

    let cargo_ok = command_available("cargo");
    let podman_ok = command_available("podman");
    let docker_ok = command_available("docker");
    let qemu_ok = command_available("qemu-system-x86_64");
    let kvm_ok = Path::new("/dev/kvm").exists();
    let manifest_dir_ok = manifest_dir.is_dir();
    let container_runtime_ok = podman_ok || docker_ok;

    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Check,
        summary: format!("cargo available: {cargo_ok}"),
        uri: None,
        path: None,
        id: Some("cargo".to_string()),
        command: Some(vec!["cargo".to_string(), "--version".to_string()]),
        exit_code: None,
        metadata: Default::default(),
    });

    if !cargo_ok {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("cargo is required for local Conary development".to_string());
    }

    if !manifest_dir_ok {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is missing: {}",
            manifest_dir.display()
        ));
    }

    if !container_runtime_ok {
        if envelope.status == OperationStatus::Ok {
            envelope.status = OperationStatus::Partial;
        }
        envelope.warnings.push(
            "Podman or Docker is required before container smoke validation can run".to_string(),
        );
    }

    if !qemu_ok || !kvm_ok {
        envelope
            .warnings
            .push("QEMU/KVM is unavailable; non-QEMU bootstrap checks can still run".to_string());
    }

    let data = serde_json::json!({
        "project_root": root.display().to_string(),
        "manifest_dir": manifest_dir.display().to_string(),
        "required": {
            "cargo": cargo_ok,
            "container_runtime": container_runtime_ok,
            "manifest_dir": manifest_dir_ok,
        },
        "optional": {
            "podman": podman_ok,
            "docker": docker_ok,
            "qemu_system_x86_64": qemu_ok,
            "dev_kvm": kvm_ok,
        },
        "default_smoke_candidate": {
            "suite": "phase1-core",
            "distro": "fedora44",
            "requires_container_runtime": true,
            "requires_qemu": false,
        },
    });

    InspectResult::new(envelope).with_data(data)
}

fn command_available(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn inspect_reports_missing_manifest_dir_without_success() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing-manifests");
        let report = inspect_with_paths(root.path(), &missing);

        assert_ne!(
            report.envelope.status,
            conary_agent_contract::OperationStatus::Ok
        );
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory"))
        );
    }

    #[test]
    fn inspect_uses_local_bootstrap_subject_uri() {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();

        let report = inspect_with_paths(root.path(), &manifests);
        assert_eq!(
            report.envelope.subject.unwrap().uri,
            "conary-local://bootstrap/status"
        );
    }
}
