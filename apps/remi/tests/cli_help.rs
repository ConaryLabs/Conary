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
    assert_supported_public_targets_only(&String::from_utf8_lossy(&output.stdout));
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
