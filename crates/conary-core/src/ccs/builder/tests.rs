// conary-core/src/ccs/builder/tests.rs

use super::*;
use crate::payload::PayloadNodeKind;
use std::io::Write;
use tempfile::TempDir;

fn create_test_manifest() -> CcsManifest {
    CcsManifest::new_minimal("test-package", "1.0.0")
}

#[test]
fn builder_empty_dir() {
    let source = TempDir::new().unwrap();
    let result = CcsBuilder::new(create_test_manifest(), source.path())
        .unwrap()
        .build()
        .unwrap();
    assert!(result.files.is_empty());
    assert!(result.payloads.is_empty());
}

#[cfg(unix)]
#[test]
fn builder_rejects_an_incomplete_source_walk() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let source = TempDir::new().unwrap();
    let unreadable = source.path().join("unreadable");
    fs::create_dir(&unreadable).unwrap();
    fs::write(unreadable.join("hidden"), b"exact authority").unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();
    let result = CcsBuilder::new(create_test_manifest(), source.path())
        .unwrap()
        .build();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("failed to scan complete CCS source authority")
    );
}

#[cfg(unix)]
#[test]
fn builder_rejects_non_utf8_payload_paths_without_renaming_them() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let source = TempDir::new().unwrap();
    fs::write(
        source
            .path()
            .join(OsString::from_vec(vec![b'p', b'a', b'y', 0xff])),
        b"exact bytes",
    )
    .unwrap();
    let error = CcsBuilder::new(create_test_manifest(), source.path())
        .unwrap()
        .build()
        .unwrap_err();
    assert!(error.to_string().contains("is not valid UTF-8"));
}

#[test]
fn invalid_policy_configuration_fails_builder_construction() {
    let source = TempDir::new().unwrap();
    let mut manifest = create_test_manifest();
    manifest.policy.reject_paths = vec!["[".to_string()];
    let error = match CcsBuilder::new(manifest, source.path()) {
        Ok(_) => panic!("invalid policy configuration must fail"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        BuilderError::PolicyConfiguration(PolicyError::Config(_))
    ));
}

#[test]
fn builder_streams_regular_source_authority() {
    let source = TempDir::new().unwrap();
    fs::create_dir_all(source.path().join("usr/bin")).unwrap();
    fs::write(source.path().join("usr/bin/myapp"), b"#!/bin/sh\necho hi\n").unwrap();
    let result = CcsBuilder::new(create_test_manifest(), source.path())
        .unwrap()
        .build()
        .unwrap();
    let entry = result
        .files
        .iter()
        .find(|entry| entry.path == "/usr/bin/myapp")
        .unwrap();
    assert_eq!(
        entry.content.as_ref().unwrap().sha256,
        crate::hash::sha256(b"#!/bin/sh\necho hi\n")
    );
    assert!(
        result
            .payload(&entry.path)
            .unwrap()
            .source()
            .unwrap()
            .path()
            .is_some()
    );
}

#[test]
fn builder_streams_policy_replacement_and_rehashes_it() {
    let source = TempDir::new().unwrap();
    fs::create_dir_all(source.path().join("usr/bin")).unwrap();
    fs::write(
        source.path().join("usr/bin/tool"),
        b"#!/usr/bin/env python\nprint('hello')\n",
    )
    .unwrap();
    let mut manifest = create_test_manifest();
    manifest.policy.fix_shebangs.insert(
        "/usr/bin/env python".to_string(),
        "/usr/bin/python3".to_string(),
    );
    let result = CcsBuilder::new(manifest, source.path())
        .unwrap()
        .build()
        .unwrap();
    let entry = result
        .files
        .iter()
        .find(|entry| entry.path == "/usr/bin/tool")
        .unwrap();
    assert_eq!(
        entry.content.as_ref().unwrap().sha256,
        crate::hash::sha256(b"#!/usr/bin/python3\nprint('hello')\n")
    );
    let mut actual = Vec::new();
    result
        .open_file(entry)
        .unwrap()
        .read_to_end(&mut actual)
        .unwrap();
    assert_eq!(actual, b"#!/usr/bin/python3\nprint('hello')\n");
}

#[test]
fn chunking_visits_reader_without_retaining_chunk_bytes() {
    let source = TempDir::new().unwrap();
    let bytes = vec![0x5a; MIN_CHUNK_SIZE as usize * 8];
    fs::write(source.path().join("large"), &bytes).unwrap();
    let result = CcsBuilder::new(create_test_manifest(), source.path())
        .unwrap()
        .with_chunking()
        .build()
        .unwrap();
    let entry = result
        .files
        .iter()
        .find(|entry| entry.path == "/large")
        .unwrap();
    assert!(!entry.chunks.as_ref().unwrap().is_empty());
    assert_eq!(result.payloads.len(), 1);
    assert_eq!(result.chunk_stats.as_ref().unwrap().chunked_files, 1);
}

#[test]
fn builder_preserves_explicit_component_assignment() {
    let source = TempDir::new().unwrap();
    fs::write(source.path().join("helper"), b"helper").unwrap();
    let mut manifest = create_test_manifest();
    manifest
        .components
        .files
        .insert("/helper".to_string(), "lib".to_string());
    let result = CcsBuilder::new(manifest, source.path())
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(result.files[0].component, "lib");
}

#[cfg(unix)]
#[test]
fn builder_preserves_symlink_authority_without_content_source() {
    let source = TempDir::new().unwrap();
    fs::write(source.path().join("target"), b"payload").unwrap();
    std::os::unix::fs::symlink("target", source.path().join("link")).unwrap();
    let result = CcsBuilder::new(create_test_manifest(), source.path())
        .unwrap()
        .build()
        .unwrap();
    let link = result
        .files
        .iter()
        .find(|entry| entry.path == "/link")
        .unwrap();
    assert_eq!(
        link.node.kind,
        PayloadNodeKind::Symlink {
            target: "target".to_string()
        }
    );
    assert!(result.payload("/link").unwrap().source().is_none());
}

#[test]
fn large_sparse_file_authoring_peak_rss_is_bounded() {
    const CHILD_ENV: &str = "CONARY_ISSUE_135_RSS_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ccs::builder::tests::large_sparse_file_authoring_peak_rss_is_bounded",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        std::io::stderr().write_all(&output.stderr).unwrap();
        assert!(output.status.success(), "large-file RSS child failed");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("ISSUE135_VM_HWM_KIB="),
            "large-file RSS child did not report VmHWM"
        );
        return;
    }

    const DECLARED_SIZE: u64 = 2 * 1024 * 1024 * 1024 + 1;
    const RSS_LIMIT_KIB: u64 = 256 * 1024;
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("target"));
    fs::create_dir_all(&target_root).unwrap();
    let workspace = tempfile::Builder::new()
        .prefix("issue-135-rss-")
        .tempdir_in(&target_root)
        .unwrap();
    let source = workspace.path().join("source");
    fs::create_dir(&source).unwrap();
    let large = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(source.join("large-zero-file"))
        .unwrap();
    large.set_len(DECLARED_SIZE).unwrap();

    let result = CcsBuilder::new(create_test_manifest(), &source)
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(result.total_size, DECLARED_SIZE);
    assert_eq!(
        result.files[0].content.as_ref().unwrap().size,
        DECLARED_SIZE
    );
    assert!(
        result.payloads[0]
            .source()
            .and_then(ReopenablePayload::path)
            .is_some()
    );

    let high_water_kib = vm_hwm_kib().unwrap();
    println!("ISSUE135_VM_HWM_KIB={high_water_kib}");
    assert!(
        high_water_kib < RSS_LIMIT_KIB,
        "VmHWM {high_water_kib} KiB exceeded fixed {RSS_LIMIT_KIB} KiB bound"
    );
}

fn vm_hwm_kib() -> Option<u64> {
    let mut status = String::new();
    fs::File::open("/proc/self/status")
        .ok()?
        .read_to_string(&mut status)
        .ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}
