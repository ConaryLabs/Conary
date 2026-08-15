// remi/build.rs

use std::env;
use std::process::Command;

fn git_output(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CONARY_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=CONARY_GIT_DIRTY");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");

    let commit = env::var("CONARY_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CONARY_GIT_COMMIT={commit}");

    let dirty = env::var("CONARY_GIT_DIRTY").unwrap_or_else(|_| {
        Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=no"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| !output.stdout.is_empty())
            .to_string()
    });
    println!("cargo:rustc-env=CONARY_GIT_DIRTY={dirty}");
}
