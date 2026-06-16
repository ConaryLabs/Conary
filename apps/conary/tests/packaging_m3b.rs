// apps/conary/tests/packaging_m3b.rs

use std::process::{Command, Output};

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn mcp_packaging_help_parses_without_starting_server() {
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("mcp")
        .arg("packaging")
        .arg("--help")
        .output()
        .expect("run conary mcp packaging --help");

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "{stdout}");
    assert!(stdout.contains("packaging"), "{stdout}");
}
