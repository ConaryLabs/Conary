// conary-test/src/config/tests/native_corpus/evidence.rs

use super::conary_fixture_path;

#[test]
fn corpus_evidence_writer_binds_request_update_and_dependency_to_exact_digests() {
    let directory = tempfile::tempdir().expect("create evidence test directory");
    let request_artifact = directory.path().join("request.rpm");
    let update_artifact = directory.path().join("update.rpm");
    let dependency_artifact = directory.path().join("dependency.ccs");
    std::fs::write(&request_artifact, b"exact native request").unwrap();
    std::fs::write(&update_artifact, b"exact native update").unwrap();
    std::fs::write(&dependency_artifact, b"exact signed dependency").unwrap();

    let request_manifest = directory.path().join("request-manifest.json");
    let update_manifest = directory.path().join("update-manifest.json");
    let dependency_manifest = directory.path().join("dependency-manifest.json");
    for (manifest, artifact, target) in [
        (&request_manifest, &request_artifact, "rpm"),
        (&update_manifest, &update_artifact, "rpm"),
        (&dependency_manifest, &dependency_artifact, "rpm"),
    ] {
        std::fs::write(
            manifest,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "artifact_path": artifact,
                "builder_target": target,
                "sha256": conary_core::hash::sha256(&std::fs::read(artifact).unwrap()),
            }))
            .unwrap(),
        )
        .unwrap();
    }

    let evidence_path = directory.path().join("evidence.json");
    let script = conary_fixture_path("native/write-corpus-evidence.py");
    let output = std::process::Command::new("python3")
        .arg(&script)
        .args([
            request_manifest.as_os_str(),
            evidence_path.as_os_str(),
            "fedora-44".as_ref(),
            "rpm".as_ref(),
            "phase4-daily-driver-corpus".as_ref(),
            "1.0.0-1".as_ref(),
            "x86_64".as_ref(),
            "resolution".as_ref(),
            "installation".as_ref(),
            "update".as_ref(),
            "--update-fixture-manifest".as_ref(),
            update_manifest.as_os_str(),
            "--update-name".as_ref(),
            "phase4-daily-driver-corpus".as_ref(),
            "--update-version".as_ref(),
            "1.0.1-1".as_ref(),
            "--update-architecture".as_ref(),
            "x86_64".as_ref(),
            "--dependency-fixture-manifest".as_ref(),
            dependency_manifest.as_os_str(),
            "--dependency-name".as_ref(),
            "phase4-repository-fixture".as_ref(),
            "--dependency-version".as_ref(),
            "1.0.0-1".as_ref(),
            "--dependency-architecture".as_ref(),
            "x86_64".as_ref(),
        ])
        .output()
        .expect("run evidence writer");
    assert!(
        output.status.success(),
        "evidence writer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
    assert_eq!(
        evidence["completed_stages"],
        serde_json::json!(["resolution", "installation", "update"])
    );
    let artifacts = evidence["source_artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 3);
    assert_eq!(artifacts[0]["role"], "install_request");
    assert_eq!(artifacts[1]["role"], "update_request");
    assert_eq!(artifacts[2]["role"], "install_dependency");
    assert_eq!(
        artifacts[0]["digest"],
        conary_core::hash::sha256(&std::fs::read(&request_artifact).unwrap())
    );
    assert_eq!(
        artifacts[1]["digest"],
        conary_core::hash::sha256(&std::fs::read(&update_artifact).unwrap())
    );
    assert_eq!(
        artifacts[2]["digest"],
        conary_core::hash::sha256(&std::fs::read(&dependency_artifact).unwrap())
    );

    std::fs::write(&dependency_artifact, b"mutated dependency").unwrap();
    let contradiction = std::process::Command::new("python3")
        .arg(script)
        .args([
            request_manifest.as_os_str(),
            evidence_path.as_os_str(),
            "fedora-44".as_ref(),
            "rpm".as_ref(),
            "request".as_ref(),
            "1".as_ref(),
            "x86_64".as_ref(),
            "installation".as_ref(),
            "--dependency-fixture-manifest".as_ref(),
            dependency_manifest.as_os_str(),
            "--dependency-name".as_ref(),
            "dependency".as_ref(),
            "--dependency-version".as_ref(),
            "1".as_ref(),
            "--dependency-architecture".as_ref(),
            "x86_64".as_ref(),
        ])
        .output()
        .expect("run contradictory evidence writer");
    assert!(!contradiction.status.success());
    assert!(
        String::from_utf8_lossy(&contradiction.stderr)
            .contains("artifact digest contradicts its build manifest")
    );
}
