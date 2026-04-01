// tests/bootstrap_workflow.rs

use std::path::{Path, PathBuf};
use std::process::Command;

use conary_core::db::schema::migrate;
use conary_core::derivation::{DerivationIndex, DerivationRecord};
use rusqlite::Connection;

fn write_run_workdir(
    root: &Path,
    operation_id: &str,
    seed_id: &str,
    builds: &[(&str, &str)],
) -> PathBuf {
    let work_dir = root.to_path_buf();
    let operations_dir = work_dir.join("operations");
    let op_dir = operations_dir.join(operation_id);
    let output_dir = op_dir.join("output");
    let generation_dir = output_dir.join("generations").join("1");
    std::fs::create_dir_all(&generation_dir).unwrap();

    let db_path = op_dir.join("derivations.db");
    let conn = Connection::open(&db_path).unwrap();
    migrate(&conn).unwrap();
    let index = DerivationIndex::new(&conn);
    for (package, output_hash) in builds {
        index
            .insert(&DerivationRecord {
                derivation_id: format!("{package}-{seed_id}"),
                output_hash: (*output_hash).to_string(),
                package_name: (*package).to_string(),
                package_version: "1.0.0".to_string(),
                manifest_cas_hash: format!("manifest-{package}-{seed_id}"),
                stage: Some("system".to_string()),
                build_env_hash: Some(seed_id.to_string()),
                built_at: "2026-03-31T00:00:00Z".to_string(),
                build_duration_secs: 1,
                trust_level: 2,
                provenance_cas_hash: None,
                reproducible: None,
            })
            .unwrap();
    }

    let record_path = operations_dir.join(format!("{operation_id}.json"));
    std::fs::create_dir_all(&operations_dir).unwrap();
    std::fs::write(
        &record_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": operation_id,
            "work_dir": work_dir,
            "manifest_path": work_dir.join("system.toml"),
            "recipe_dir": work_dir.join("recipes"),
            "seed_id": seed_id,
            "up_to": null,
            "only": [],
            "cascade": false,
            "derivation_db_path": db_path,
            "output_dir": output_dir,
            "generation_dir": generation_dir,
            "profile_hash": "profile-abc",
            "completed_successfully": true,
            "failure_reason": null
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        operations_dir.join("latest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "operation_id": operation_id,
            "record_path": record_path
        }))
        .unwrap(),
    )
    .unwrap();

    work_dir
}

fn write_seed_dir(path: &Path, erofs_contents: &[u8], distro: &str, with_cas: bool) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join("seed.erofs"), erofs_contents).unwrap();
    let seed_id = conary_core::derivation::erofs_image_hash(&path.join("seed.erofs")).unwrap();
    std::fs::write(
        path.join("seed.toml"),
        format!(
            "seed_id = \"{seed_id}\"\nsource = \"adopted\"\norigin_distro = \"{distro}\"\npackages = []\ntarget_triple = \"x86_64\"\nverified_by = []\n"
        ),
    )
    .unwrap();
    if with_cas {
        std::fs::create_dir_all(path.join("cas")).unwrap();
    }
}

#[test]
fn bootstrap_verify_convergence_reports_success_for_completed_runs() {
    let temp = tempfile::tempdir().unwrap();
    let run_a = write_run_workdir(
        &temp.path().join("run-a"),
        "op-a",
        "seed-a",
        &[("bash", "aaa")],
    );
    let run_b = write_run_workdir(
        &temp.path().join("run-b"),
        "op-b",
        "seed-b",
        &[("bash", "aaa")],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "bootstrap",
            "verify-convergence",
            "--run-a",
            run_a.to_str().unwrap(),
            "--run-b",
            run_b.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "verify-convergence failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Compared 1 packages"));
    assert!(stdout.contains("[COMPLETE] All compared packages converged."));
}

#[test]
fn bootstrap_verify_convergence_fails_when_runs_share_no_packages() {
    let temp = tempfile::tempdir().unwrap();
    let run_a = write_run_workdir(
        &temp.path().join("run-a"),
        "op-a",
        "seed-a",
        &[("bash", "aaa")],
    );
    let run_b = write_run_workdir(
        &temp.path().join("run-b"),
        "op-b",
        "seed-b",
        &[("sed", "bbb")],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "bootstrap",
            "verify-convergence",
            "--run-a",
            run_a.to_str().unwrap(),
            "--run-b",
            run_b.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No comparable packages found"));
}

#[test]
fn bootstrap_diff_seeds_reports_hash_and_metadata_changes() {
    let temp = tempfile::tempdir().unwrap();
    let seed_a = temp.path().join("seed-a");
    let seed_b = temp.path().join("seed-b");
    write_seed_dir(&seed_a, b"seed-a", "fedora", true);
    write_seed_dir(&seed_b, b"seed-b", "arch", false);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args([
            "bootstrap",
            "diff-seeds",
            seed_a.to_str().unwrap(),
            seed_b.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "diff-seeds failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("origin_distro"));
    assert!(stdout.contains("seed.erofs: content hash differs"));
    assert!(stdout.contains("cas: present in A only"));
}
