// apps/conary-test/build.rs

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CONARY_TEST_BUILD_TIMESTAMP");

    if let (Some(git_dir), Some(git_common_dir), Some(repo_root)) = (
        git_path(&["rev-parse", "--git-dir"]),
        git_path(&["rev-parse", "--git-common-dir"]),
        git_path(&["rev-parse", "--show-toplevel"]),
    ) {
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
    }

    let git_commit = git_output(&["rev-parse", "HEAD"]).unwrap_or_else(|| {
        emit_git_metadata_warning("rev-parse HEAD");
        "unknown".to_string()
    });
    let commit_timestamp = git_output(&["log", "-1", "--format=%cd"]).unwrap_or_else(|| {
        emit_git_metadata_warning("log -1 --format=%cd");
        "unknown".to_string()
    });

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

fn emit_git_metadata_warning(context: &str) {
    println!(
        "cargo:warning=conary-test build metadata falling back to 'unknown' because git {context} was unavailable"
    );
}

fn git_path(args: &[&str]) -> Option<PathBuf> {
    git_output(args).map(PathBuf::from)
}

fn git_output(args: &[&str]) -> Option<String> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set for conary-test build metadata");
    let output = Command::new("git")
        .current_dir(manifest_dir)
        .args(args)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8(output.stdout)
            .expect("git output for conary-test build metadata was not valid UTF-8")
            .trim()
            .to_owned(),
    )
}
