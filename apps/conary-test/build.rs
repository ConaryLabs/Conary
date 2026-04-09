// apps/conary-test/build.rs

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CONARY_TEST_BUILD_TIMESTAMP");

    let git_dir = git_path(&["rev-parse", "--git-dir"]);
    let git_common_dir = git_path(&["rev-parse", "--git-common-dir"]);
    let repo_root = git_path(&["rev-parse", "--show-toplevel"]);
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join(".git").display()
    );
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    println!("cargo:rerun-if-changed={}", git_dir.join("refs").display());
    emit_optional_watch(&git_dir.join("packed-refs"));
    println!(
        "cargo:rerun-if-changed={}",
        git_common_dir.join("refs").display()
    );
    emit_optional_watch(&git_common_dir.join("packed-refs"));

    let git_commit = git_output(&["rev-parse", "HEAD"]);
    let commit_timestamp = git_output(&["log", "-1", "--format=%cd"]);

    println!("cargo:rustc-env=CONARY_TEST_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=CONARY_TEST_COMMIT_TIMESTAMP={commit_timestamp}");

    if let Ok(build_timestamp) = env::var("CONARY_TEST_BUILD_TIMESTAMP")
        && !build_timestamp.is_empty()
    {
        println!("cargo:rustc-env=CONARY_TEST_BUILD_TIMESTAMP={build_timestamp}");
    }
}

fn emit_optional_watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_path(args: &[&str]) -> PathBuf {
    PathBuf::from(git_output(args))
}

fn git_output(args: &[&str]) -> String {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set for conary-test build metadata");
    let output = Command::new("git")
        .current_dir(manifest_dir)
        .args(args)
        .output()
        .expect("failed to run git for conary-test build metadata");

    if !output.status.success() {
        panic!(
            "git {:?} failed for conary-test build metadata: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .expect("git output for conary-test build metadata was not valid UTF-8")
        .trim()
        .to_owned()
}
