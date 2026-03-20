// conary-core/src/derivation/capture.rs

//! Output capture: walk a DESTDIR and ingest every file into CAS.
//!
//! After a derivation build populates a DESTDIR tree, `capture_output()` walks
//! it, stores every regular file in the content-addressable store, records
//! symlinks, and produces an [`OutputManifest`] whose `output_hash` uniquely
//! identifies the build result.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use chrono::Utc;
use walkdir::WalkDir;

use crate::filesystem::CasStore;

use super::output::{OutputFile, OutputManifest, OutputSymlink};

/// Errors that can occur during output capture.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// Filesystem I/O failed (reading DESTDIR, reading symlinks, etc.).
    #[error("I/O error: {0}")]
    Io(String),
    /// Storing content in CAS failed.
    #[error("CAS error: {0}")]
    Cas(String),
}

/// Walk `destdir`, ingest every regular file into `cas`, record symlinks, and
/// return a complete [`OutputManifest`].
///
/// Directories are skipped (they are implicit from file paths). The manifest's
/// `output_hash` is computed via [`OutputManifest::compute_output_hash()`].
///
/// # Errors
///
/// Returns [`CaptureError::Io`] on filesystem failures and [`CaptureError::Cas`]
/// if CAS ingestion fails.
pub fn capture_output(
    destdir: &Path,
    cas: &CasStore,
    derivation_id: &str,
    build_duration_secs: u64,
) -> Result<OutputManifest, CaptureError> {
    let mut files = Vec::new();
    let mut symlinks = Vec::new();

    for entry in WalkDir::new(destdir).follow_links(false) {
        let entry = entry.map_err(|e| CaptureError::Io(e.to_string()))?;
        let path = entry.path();

        // Compute the relative path within the DESTDIR.
        let rel_path = path
            .strip_prefix(destdir)
            .map_err(|e| CaptureError::Io(e.to_string()))?;

        // Skip the root directory itself.
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let file_type = entry.file_type();

        if file_type.is_symlink() {
            let target = std::fs::read_link(path)
                .map_err(|e| CaptureError::Io(format!("{}: {e}", path.display())))?;
            symlinks.push(OutputSymlink {
                path: format!("/{}", rel_path.display()),
                target: target.to_string_lossy().into_owned(),
            });
        } else if file_type.is_file() {
            let content = std::fs::read(path)
                .map_err(|e| CaptureError::Io(format!("{}: {e}", path.display())))?;
            let metadata = entry
                .metadata()
                .map_err(|e| CaptureError::Io(format!("{}: {e}", path.display())))?;
            let hash = cas
                .store(&content)
                .map_err(|e| CaptureError::Cas(e.to_string()))?;
            files.push(OutputFile {
                path: format!("/{}", rel_path.display()),
                hash,
                size: metadata.len(),
                mode: metadata.mode(),
            });
        }
        // Directories are skipped (implicit from file paths).
    }

    let output_hash = OutputManifest::compute_output_hash(&files, &symlinks);
    let built_at = Utc::now().to_rfc3339();

    Ok(OutputManifest {
        derivation_id: derivation_id.to_owned(),
        output_hash,
        files,
        symlinks,
        build_duration_secs,
        built_at,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;
    use crate::derivation::test_helpers::helpers::test_cas;

    #[test]
    fn captures_files_to_cas() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(destdir.join("usr/bin")).unwrap();

        let file_path = destdir.join("usr/bin/hello");
        std::fs::write(&file_path, b"#!/bin/sh\necho hello\n").unwrap();
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cas = test_cas(tmp.path());
        let manifest =
            capture_output(&destdir, &cas, &"d".repeat(64), 5).expect("capture must succeed");

        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "/usr/bin/hello");
        assert_eq!(manifest.files[0].size, 21);
        assert_eq!(manifest.files[0].mode & 0o777, 0o755);

        // Verify CAS actually has the content.
        assert!(
            cas.exists(&manifest.files[0].hash),
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
        let manifest =
            capture_output(&destdir, &cas, &"d".repeat(64), 3).expect("capture must succeed");

        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.symlinks.len(), 1);
        assert_eq!(manifest.symlinks[0].path, "/usr/lib/libfoo.so");
        assert_eq!(manifest.symlinks[0].target, "libfoo.so.1.0");
    }

    #[test]
    fn output_hash_is_64_char_hex() {
        let tmp = TempDir::new().unwrap();
        let destdir = tmp.path().join("destdir");
        std::fs::create_dir_all(destdir.join("usr/bin")).unwrap();
        std::fs::write(destdir.join("usr/bin/tool"), b"binary").unwrap();

        let cas = test_cas(tmp.path());
        let manifest =
            capture_output(&destdir, &cas, &"d".repeat(64), 1).expect("capture must succeed");

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
        let manifest =
            capture_output(&destdir, &cas, &"d".repeat(64), 0).expect("capture must succeed");

        assert!(manifest.files.is_empty());
        assert!(manifest.symlinks.is_empty());
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
        let manifest =
            capture_output(&destdir, &cas, &drv_id, 42).expect("capture must succeed");

        assert_eq!(manifest.derivation_id, drv_id);
        assert_eq!(manifest.build_duration_secs, 42);
        assert!(!manifest.built_at.is_empty(), "built_at must be set");
    }
}
