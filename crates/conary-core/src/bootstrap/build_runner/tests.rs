// conary-core/src/bootstrap/build_runner/tests.rs

use super::*;
use crate::recipe::parse_recipe;
use std::path::Path;
use std::process::Command;

fn sha256_of(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().unwrap();
    assert!(output.status.success(), "sha256sum should succeed in tests");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn create_tarball(archive: &Path, top_level: &str, file_name: &str, contents: &str) {
    let tempdir = tempfile::tempdir().unwrap();
    let source_root = tempdir.path().join(top_level);
    fs::create_dir_all(&source_root).unwrap();
    fs::write(source_root.join(file_name), contents).unwrap();

    let output = Command::new("tar")
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(tempdir.path())
        .arg(top_level)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "test tarball creation should succeed"
    );
}

fn sha256_only_runner(sources_dir: &Path) -> PackageBuildRunner {
    PackageBuildRunner::new(sources_dir).with_checksum_contract(ChecksumContract::Sha256Only)
}

#[test]
fn test_build_runner_new() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());
    assert_eq!(runner.sources_dir, dir.path());
    assert_eq!(runner.checksum_contract, ChecksumContract::Supported);
}

#[test]
fn test_verify_checksum_placeholder_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());

    let file = dir.path().join("test.tar.gz");
    std::fs::write(&file, b"test").unwrap();

    let result = runner.verify_checksum("test", "VERIFY_BEFORE_BUILD", &file);
    assert!(result.is_err());
}

#[test]
fn test_verify_checksum_invalid_format() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());

    let file = dir.path().join("test.tar.gz");
    std::fs::write(&file, b"test").unwrap();

    let result = runner.verify_checksum("test", "nocolon", &file);
    assert!(result.is_err());
}

#[test]
fn test_verify_checksum_supported_md5_is_verified() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());

    let file = dir.path().join("test.tar.gz");
    std::fs::write(&file, b"").unwrap();

    let result = runner.verify_checksum("test", "md5:d41d8cd98f00b204e9800998ecf8427e", &file);
    assert!(result.is_ok());
}

#[test]
fn test_verify_checksum_unknown_algorithm_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());

    let file = dir.path().join("test.tar.gz");
    std::fs::write(&file, b"test").unwrap();

    let result = runner.verify_checksum(
        "test",
        "sha1:0000000000000000000000000000000000000000",
        &file,
    );
    assert!(result.is_err());
}

#[test]
fn test_verify_checksum_md5_rejected_by_sha256_only_contract() {
    let dir = tempfile::tempdir().unwrap();
    let runner = sha256_only_runner(dir.path());

    let file = dir.path().join("test.tar.gz");
    std::fs::write(&file, b"test").unwrap();

    let result = runner.verify_checksum("test", "md5:d41d8cd98f00b204e9800998ecf8427e", &file);
    assert!(result.is_err());
}

#[test]
fn test_gnu_fetch_candidates_adds_canonical_fallback_for_ftpmirror() {
    let candidates = gnu_fetch_candidates("https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz");
    assert_eq!(
        candidates,
        vec![
            "https://ftpmirror.gnu.org/bash/bash-5.3.tar.gz".to_string(),
            "https://ftp.gnu.org/gnu/bash/bash-5.3.tar.gz".to_string()
        ]
    );
}

#[test]
fn test_gnu_fetch_candidates_leaves_non_ftpmirror_urls_unchanged() {
    let candidates = gnu_fetch_candidates("https://example.invalid/src.tar.gz");
    assert_eq!(
        candidates,
        vec!["https://example.invalid/src.tar.gz".to_string()]
    );
}

#[test]
fn test_stage_additional_sources_preserves_raw_archive_in_package_root() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());
    let package_root = dir.path().join("pkg");
    let src_dir = package_root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let cached_archive = dir.path().join("dep-1.0.tar.gz");
    create_tarball(&cached_archive, "dep-1.0", "README", "hello\n");
    let digest = sha256_of(&cached_archive);
    let recipe = parse_recipe(&format!(
        r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.invalid/test-1.0.tar.gz"
checksum = "sha256:{digest}"

[[source.additional]]
url = "https://example.invalid/dep-1.0.tar.gz"
checksum = "sha256:{digest}"

[build]
install = "true"
"#
    ))
    .unwrap();

    runner
        .stage_additional_sources("test", &recipe, &package_root, &src_dir)
        .unwrap();

    assert!(
        package_root.join("dep-1.0.tar.gz").exists(),
        "raw additional archive should remain staged next to the package root"
    );
    assert!(
        src_dir.join("README").exists(),
        "extracting an additional source should populate the source tree"
    );
}

#[test]
fn test_stage_additional_sources_skips_extraction_when_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());
    let package_root = dir.path().join("pkg");
    let src_dir = package_root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let cached_archive = dir.path().join("tzdata.tar.gz");
    create_tarball(&cached_archive, "tzdata", "zone.tab", "UTC\n");
    let digest = sha256_of(&cached_archive);
    let recipe = parse_recipe(&format!(
        r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.invalid/test-1.0.tar.gz"
checksum = "sha256:{digest}"

[[source.additional]]
url = "https://example.invalid/tzdata.tar.gz"
checksum = "sha256:{digest}"
extract = false

[build]
install = "true"
"#
    ))
    .unwrap();

    runner
        .stage_additional_sources("test", &recipe, &package_root, &src_dir)
        .unwrap();

    assert!(package_root.join("tzdata.tar.gz").exists());
    assert!(
        !src_dir.join("zone.tab").exists(),
        "extract = false should keep the raw archive only"
    );
}

#[test]
fn test_stage_and_apply_patches_copies_remote_patch_then_applies_it() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());
    let package_root = dir.path().join("pkg");
    let src_dir = package_root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("hello.txt"), "before\n").unwrap();

    let patch_content = "\
--- a/hello.txt\n\
+++ b/hello.txt\n\
@@ -1 +1 @@\n\
-before\n\
+after\n";
    let cached_patch = dir.path().join("fix.patch");
    fs::write(&cached_patch, patch_content).unwrap();
    let digest = sha256_of(&cached_patch);
    let recipe = parse_recipe(&format!(
        r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.invalid/test-1.0.tar.gz"
checksum = "sha256:{digest}"

[patches]
files = [
  {{ file = "https://example.invalid/fix.patch", checksum = "sha256:{digest}", strip = 1 }},
]

[build]
install = "true"
"#
    ))
    .unwrap();

    runner
        .stage_and_apply_patches("test", &recipe, &package_root, &src_dir)
        .unwrap();

    assert!(
        package_root.join("patches/fix.patch").exists(),
        "remote patch should be staged into the package-local patches directory"
    );
    assert_eq!(
        fs::read_to_string(src_dir.join("hello.txt")).unwrap(),
        "after\n"
    );
}

#[test]
fn test_prepare_build_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());

    let (src_dir, build_dir) = runner.prepare_build_dirs(dir.path(), "test-pkg").unwrap();
    assert!(src_dir.exists());
    assert!(build_dir.exists());
    assert!(src_dir.ends_with("src"));
    assert!(build_dir.ends_with("build"));
}

#[test]
fn test_prepare_build_dirs_cleans_previous() {
    let dir = tempfile::tempdir().unwrap();
    let runner = PackageBuildRunner::new(dir.path());

    // Create a file in the build dir
    let build_base = dir.path().join("build").join("test-pkg");
    std::fs::create_dir_all(&build_base).unwrap();
    std::fs::write(build_base.join("old-file"), b"old").unwrap();

    let (src_dir, _) = runner.prepare_build_dirs(dir.path(), "test-pkg").unwrap();
    assert!(src_dir.exists());
    assert!(!build_base.join("old-file").exists());
}
