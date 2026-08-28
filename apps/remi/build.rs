// remi/build.rs

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git_output(arguments: &[&str]) -> Option<String> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").ok()?;
    let output = Command::new("git")
        .current_dir(manifest_dir)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_path(arguments: &[&str]) -> Option<PathBuf> {
    git_output(arguments).map(PathBuf::from)
}

fn emit_optional_watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_dirty() -> Option<bool> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").ok()?;
    let output = Command::new("git")
        .current_dir(manifest_dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then_some(!output.stdout.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CONARY_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=CONARY_GIT_DIRTY");
    for git_name in ["HEAD", "logs/HEAD", "index"] {
        if let Some(path) = git_path(&["rev-parse", "--git-path", git_name]) {
            emit_optional_watch(&path);
        }
    }

    let commit = env::var("CONARY_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CONARY_GIT_COMMIT={commit}");

    let dirty =
        env::var("CONARY_GIT_DIRTY").unwrap_or_else(|_| git_dirty().unwrap_or(false).to_string());
    println!("cargo:rustc-env=CONARY_GIT_DIRTY={dirty}");
}
