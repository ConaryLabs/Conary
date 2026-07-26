// conary-core/src/derivation/capture.rs

//! Output capture: preserve a complete DESTDIR payload tree in CAS.
//!
//! After a derivation build populates a DESTDIR tree, `capture_output()` stores
//! every regular file in CAS and records exact typed authority for every POSIX
//! node.

use std::path::Path;

use chrono::Utc;

use crate::filesystem::CasStore;
use crate::generation::root_manifest::scan_payload_tree;

use super::output::OutputManifest;

/// Errors that can occur during output capture.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// Filesystem I/O failed (reading DESTDIR, reading symlinks, etc.).
    #[error("I/O error: {0}")]
    Io(String),
    /// Storing content in CAS failed.
    #[error("CAS error: {0}")]
    Cas(String),
    /// The captured payload tree did not satisfy the output contract.
    #[error("manifest error: {0}")]
    Manifest(String),
}

/// Walk `destdir`, ingest regular contents into `cas`, and return an exact
/// current-format [`OutputManifest`].
///
/// # Errors
///
/// Returns [`CaptureError::Io`] on filesystem failures and [`CaptureError::Cas`]
/// if CAS ingestion fails.
pub fn capture_output(
    destdir: &Path,
    cas: &CasStore,
    derivation_id: &str,
    package_name: &str,
    package_version: &str,
    build_duration_secs: u64,
) -> Result<OutputManifest, CaptureError> {
    let (root, entries) = scan_payload_tree(destdir, cas, derivation_id)
        .map_err(|error| CaptureError::Manifest(error.to_string()))?;
    let built_at = Utc::now().to_rfc3339();
    OutputManifest::new(
        derivation_id,
        package_name,
        package_version,
        root,
        entries,
        build_duration_secs,
        built_at,
    )
    .map_err(|error| CaptureError::Manifest(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;
    use crate::derivation::test_helpers::helpers::test_cas;
    use crate::payload::PayloadNodeKind;

    fn capture_test_output(
        destdir: &Path,
        cas: &CasStore,
        derivation_id: &str,
        duration: u64,
    ) -> OutputManifest {
        capture_output(destdir, cas, derivation_id, "test-package", "1.0", duration)
            .expect("capture must succeed")
    }

    #[test]
    fn captures_files_to_cas() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(destdir.join("usr/bin")).unwrap();

        let file_path = destdir.join("usr/bin/hello");
        std::fs::write(&file_path, b"#!/bin/sh\necho hello\n").unwrap();
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cas = test_cas(tmp.path());
        let manifest = capture_test_output(&destdir, &cas, &"d".repeat(64), 5);
        let file = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "/usr/bin/hello")
            .unwrap();
        assert_eq!(file.content.as_ref().unwrap().size, 21);
        assert_eq!(file.node.source.mode & 0o777, 0o755);

        // Verify CAS actually has the content.
        assert!(
            cas.exists(&file.content.as_ref().unwrap().sha256),
            "CAS must contain the stored file"
        );
    }

    #[test]
    fn captures_symlinks() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(destdir.join("usr/lib")).unwrap();

        // Create a real file and a symlink to it.
        std::fs::write(destdir.join("usr/lib/libfoo.so.1.0"), b"ELF").unwrap();
        std::os::unix::fs::symlink("libfoo.so.1.0", destdir.join("usr/lib/libfoo.so")).unwrap();

        let cas = test_cas(tmp.path());
        let manifest = capture_test_output(&destdir, &cas, &"d".repeat(64), 3);
        let link = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "/usr/lib/libfoo.so")
            .unwrap();
        assert!(matches!(
            &link.node.source.kind,
            PayloadNodeKind::Symlink { target } if target == "libfoo.so.1.0"
        ));
    }

    #[test]
    fn output_hash_is_64_char_hex() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(destdir.join("usr/bin")).unwrap();
        std::fs::write(destdir.join("usr/bin/tool"), b"binary").unwrap();

        let cas = test_cas(tmp.path());
        let manifest = capture_test_output(&destdir, &cas, &"d".repeat(64), 1);

        assert_eq!(manifest.output_hash.len(), 64);
        assert!(
            manifest.output_hash.chars().all(|c| c.is_ascii_hexdigit()),
            "output_hash must be valid hex"
        );
    }

    #[test]
    fn empty_destdir_produces_empty_manifest() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(&destdir).unwrap();

        let cas = test_cas(tmp.path());
        let manifest = capture_test_output(&destdir, &cas, &"d".repeat(64), 0);

        assert!(manifest.entries.is_empty());
        assert_eq!(manifest.derivation_id, "d".repeat(64));
        assert_eq!(manifest.build_duration_secs, 0);
        // Even an empty manifest has a deterministic output hash (64-char hex).
        assert_eq!(manifest.output_hash.len(), 64);
    }

    #[test]
    fn derivation_id_and_duration_are_recorded() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(&destdir).unwrap();

        let cas = test_cas(tmp.path());
        let drv_id = "a]".repeat(0) + &"f".repeat(64);
        let manifest = capture_test_output(&destdir, &cas, &drv_id, 42);

        assert_eq!(manifest.derivation_id, drv_id);
        assert_eq!(manifest.build_duration_secs, 42);
        assert!(!manifest.built_at.is_empty(), "built_at must be set");
    }
}
