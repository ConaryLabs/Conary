// apps/remi/tests/cli_help.rs

use std::process::{Command, Output};

fn run_remi(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_remi"))
        .args(args)
        .output()
        .expect("failed to run remi")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_supported_public_targets_only(text: &str) {
    assert!(text.contains("fedora-44, ubuntu-26.04, arch"), "{text}");
    assert!(!text.to_lowercase().contains("debian"), "{text}");
}

#[test]
fn phase2_pruning_index_gen_help_lists_only_supported_public_targets() {
    let output = run_remi(&["index-gen", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_supported_public_targets_only(&stdout);
    assert!(stdout.contains("--source-profile"), "{stdout}");
    assert!(!stdout.contains("--distro"), "{stdout}");
}

#[test]
fn phase2_pruning_prewarm_help_lists_only_supported_public_targets() {
    let output = run_remi(&["prewarm", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert_supported_public_targets_only(&String::from_utf8_lossy(&output.stdout));
}

#[test]
fn phase2_pruning_conversion_benchmark_help_lists_only_supported_public_targets() {
    let output = run_remi(&["conversion-benchmark", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert_supported_public_targets_only(&String::from_utf8_lossy(&output.stdout));
}

#[test]
fn conversion_crawl_has_no_package_exclusion_or_sampling_controls() {
    let output = run_remi(&["conversion-crawl", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for required in ["--config", "--candidate", "--output", "--concurrency"] {
        assert!(stdout.contains(required), "missing {required}: {stdout}");
    }
    for forbidden in [
        "--distro",
        "--profile",
        "--max-packages",
        "--pattern",
        "--popularity-file",
        "--dry-run",
        "--exclude",
        "--db",
        "--catalog-dir",
        "--chunk-dir",
        "--cache-dir",
        "--repository-keys-dir",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "forbidden {forbidden}: {stdout}"
        );
    }
}

#[test]
fn promotion_activation_accepts_only_complete_evidence_inputs() {
    let output = run_remi(&["promotion-activate", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for required in ["--config", "--promotion-evidence", "--conversion-crawl"] {
        assert!(stdout.contains(required), "missing {required}: {stdout}");
    }
    for forbidden in ["--candidate", "--profile", "--distro", "--exclude"] {
        assert!(
            !stdout.contains(forbidden),
            "forbidden {forbidden}: {stdout}"
        );
    }
}

#[test]
fn promotion_proof_requires_every_exact_ordered_evidence_binding() {
    let output = run_remi(&["promotion-prove", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for required in [
        "--config",
        "--candidate",
        "--package-oracle",
        "--native-resolution",
        "--architecture",
        "--conversion-crawl",
        "--output-dir",
    ] {
        assert!(stdout.contains(required), "missing {required}: {stdout}");
    }
    for forbidden in ["--distro", "--exclude", "--skip", "--dry-run"] {
        assert!(
            !stdout.contains(forbidden),
            "forbidden {forbidden}: {stdout}"
        );
    }
}

#[test]
fn native_oracle_input_accepts_only_exact_candidate_and_output_bindings() {
    let output = run_remi(&["native-oracle-input", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for required in ["--db", "--catalog-dir", "--candidate", "--output-dir"] {
        assert!(stdout.contains(required), "missing {required}: {stdout}");
    }
    for forbidden in [
        "--distro",
        "--profile",
        "--exclude",
        "--skip",
        "--dry-run",
        "--max-packages",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "forbidden {forbidden}: {stdout}"
        );
    }
}
