// conary-core/src/derivation/install.rs

//! Install derivation outputs from CAS into a live chroot sysroot.
//!
//! After a derivation is built and its outputs are stored in the
//! content-addressable store, `install_to_sysroot()` materialises those
//! outputs into a mutable sysroot directory. Files are hard-linked from CAS
//! when possible (same filesystem) and copied otherwise. Symlinks are
//! recreated verbatim. The function uses a last-writer-wins strategy: any
//! existing file or symlink at the destination is removed before writing.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{debug, warn};

use super::output::OutputManifest;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while installing derivation outputs into a sysroot.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// A filesystem I/O operation failed for a specific destination path.
    #[error("I/O error installing {path}: {source}")]
    Io {
        /// The destination path that triggered the error.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// A required CAS object was not present on disk.
    #[error("CAS object not found: {0}")]
    MissingCasObject(String),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install all files and symlinks from `manifest` into `sysroot`.
///
/// Files are sourced from `cas_dir`, which is expected to use the standard
/// two-level layout: `cas_dir/<hash[..2]>/<hash[2..]>`.
///
/// # Algorithm
///
/// For each file in `manifest.files`:
/// - Destination = `sysroot` / `file.path` (leading `/` stripped).
/// - Parent directories are created as needed.
/// - Any existing file at the destination is removed (last-writer-wins).
/// - The CAS object is hard-linked into the destination; if that fails
///   (e.g. cross-device), it is copied instead.
/// - Permissions are set from `file.mode`.
///
/// For each symlink in `manifest.symlinks`:
/// - Destination = `sysroot` / `symlink.path` (leading `/` stripped).
/// - Parent directories are created as needed.
/// - Any existing entry at the destination is removed.
/// - The symlink is created with the recorded target.
///
/// # Errors
///
/// Returns [`InstallError::MissingCasObject`] if a required CAS object is
/// absent.  Returns [`InstallError::Io`] on any filesystem operation failure.
pub fn install_to_sysroot(
    manifest: &OutputManifest,
    sysroot: &Path,
    cas_dir: &Path,
) -> Result<u64, InstallError> {
    let mut installed: u64 = 0;
    let mut created_dirs: HashSet<PathBuf> = HashSet::new();

    for file in &manifest.files {
        let dest = sysroot_path(sysroot, &file.path);

        // Ensure parent directory exists (skip if already created).
        if let Some(parent) = dest.parent()
            && created_dirs.insert(parent.to_path_buf())
        {
            fs::create_dir_all(parent).map_err(|e| InstallError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }

        // Remove any existing entry (last-writer-wins).
        remove_if_exists(&dest)?;

        // Locate the CAS object and link/copy it into the sysroot.
        let cas_path = cas_object_path(cas_dir, &file.hash);

        match fs::hard_link(&cas_path, &dest) {
            Ok(()) => {}
            Err(link_err) => {
                fs::copy(&cas_path, &dest).map_err(|copy_err| {
                    if copy_err.kind() == std::io::ErrorKind::NotFound
                        && link_err.kind() == std::io::ErrorKind::NotFound
                    {
                        InstallError::MissingCasObject(file.hash.clone())
                    } else {
                        InstallError::Io {
                            path: dest.display().to_string(),
                            source: copy_err,
                        }
                    }
                })?;
            }
        }

        // Apply mode bits.
        fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(file.mode))
            .map_err(|e| InstallError::Io {
                path: dest.display().to_string(),
                source: e,
            })?;

        debug!(path = %dest.display(), hash = %file.hash, "installed file");
        installed += 1;
    }

    for symlink in &manifest.symlinks {
        let dest = sysroot_path(sysroot, &symlink.path);

        // Ensure parent directory exists (skip if already created).
        if let Some(parent) = dest.parent()
            && created_dirs.insert(parent.to_path_buf())
        {
            fs::create_dir_all(parent).map_err(|e| InstallError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }

        // Remove any existing entry (last-writer-wins).
        remove_if_exists(&dest)?;

        unix_fs::symlink(&symlink.target, &dest).map_err(|e| InstallError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;

        debug!(path = %dest.display(), target = %symlink.target, "installed symlink");
        installed += 1;
    }

    Ok(installed)
}

/// Run `ldconfig` inside the sysroot if any shared libraries were installed.
///
/// Shared library detection: any file whose path ends with `.so` or contains
/// `.so.` is considered a shared library.
///
/// On success or failure, the outcome is logged via `tracing` and the function
/// returns without an error -- `ldconfig` is best-effort.
pub fn run_ldconfig_if_needed(manifest: &OutputManifest, sysroot: &Path) {
    let needs_ldconfig = manifest.files.iter().any(|f| {
        let p = f.path.as_str();
        p.ends_with(".so") || p.contains(".so.")
    });

    if !needs_ldconfig {
        return;
    }

    // Try /sbin/ldconfig first, then /usr/sbin/ldconfig.
    for ldconfig in &["/sbin/ldconfig", "/usr/sbin/ldconfig"] {
        let status = Command::new("chroot")
            .arg(sysroot)
            .arg(ldconfig)
            .status();

        match status {
            Ok(s) if s.success() => {
                debug!(sysroot = %sysroot.display(), ldconfig, "ldconfig succeeded");
                return;
            }
            Ok(s) => {
                warn!(
                    sysroot = %sysroot.display(),
                    ldconfig,
                    code = ?s.code(),
                    "ldconfig exited with non-zero status"
                );
            }
            Err(e) => {
                warn!(
                    sysroot = %sysroot.display(),
                    ldconfig,
                    error = %e,
                    "ldconfig could not be run"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Resolve a manifest-absolute path into a real filesystem path under `sysroot`.
///
/// Leading `/` is stripped so that `Path::join` does not discard the sysroot
/// prefix.
fn sysroot_path(sysroot: &Path, manifest_path: &str) -> PathBuf {
    let stripped = manifest_path.trim_start_matches('/');
    sysroot.join(stripped)
}

/// Return the CAS object path for `hash` under `cas_dir`.
fn cas_object_path(cas_dir: &Path, hash: &str) -> PathBuf {
    // Guard against unexpectedly short hashes.
    if hash.len() < 3 {
        return cas_dir.join(hash);
    }
    let (prefix, rest) = hash.split_at(2);
    cas_dir.join(prefix).join(rest)
}

/// Remove a file or symlink at `path` if it exists; succeed silently if absent.
fn remove_if_exists(path: &Path) -> Result<(), InstallError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(InstallError::Io {
            path: path.display().to_string(),
            source: e,
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;
    use crate::derivation::output::{OutputFile, OutputManifest, OutputSymlink};
    use tempfile::TempDir;

    /// Build a minimal [`OutputManifest`] with the supplied files and symlinks.
    fn make_manifest(
        files: Vec<OutputFile>,
        symlinks: Vec<OutputSymlink>,
    ) -> OutputManifest {
        OutputManifest {
            derivation_id: "test-derivation".to_owned(),
            output_hash: OutputManifest::compute_output_hash(&files, &symlinks),
            hash_version: 1,
            files,
            symlinks,
            build_duration_secs: 0,
            built_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    /// Write `content` into the CAS under `cas_dir` using the SHA-256 layout.
    /// Returns the hex hash string.
    fn write_cas_object(cas_dir: &Path, content: &[u8]) -> String {
        let h = hash::sha256(content);
        let (prefix, rest) = h.split_at(2);
        let dir = cas_dir.join(prefix);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(rest), content).unwrap();
        h
    }

    // ------------------------------------------------------------------

    #[test]
    fn test_install_creates_files_and_symlinks() {
        let cas = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();

        let content = b"hello world";
        let file_hash = write_cas_object(cas.path(), content);

        let files = vec![OutputFile {
            path: "/usr/bin/hello".to_owned(),
            hash: file_hash,
            size: content.len() as u64,
            mode: 0o755,
        }];
        let symlinks = vec![OutputSymlink {
            path: "/usr/bin/hi".to_owned(),
            target: "hello".to_owned(),
        }];
        let manifest = make_manifest(files, symlinks);

        let count = install_to_sysroot(&manifest, sysroot.path(), cas.path())
            .expect("install must succeed");
        assert_eq!(count, 2, "one file + one symlink = 2");

        // Verify file content.
        let dest_file = sysroot.path().join("usr/bin/hello");
        assert!(dest_file.exists(), "installed file must exist");
        assert_eq!(fs::read(&dest_file).unwrap(), content);

        // Verify symlink target.
        let dest_link = sysroot.path().join("usr/bin/hi");
        let target = fs::read_link(&dest_link).expect("symlink must exist");
        assert_eq!(target.to_str().unwrap(), "hello");
    }

    #[test]
    fn test_install_overwrites_existing_file() {
        let cas = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();

        // Pre-create a stale file at the destination.
        let dest_dir = sysroot.path().join("usr/bin");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("hello"), b"stale content").unwrap();

        let new_content = b"new content";
        let file_hash = write_cas_object(cas.path(), new_content);

        let files = vec![OutputFile {
            path: "/usr/bin/hello".to_owned(),
            hash: file_hash,
            size: new_content.len() as u64,
            mode: 0o644,
        }];
        let manifest = make_manifest(files, vec![]);

        install_to_sysroot(&manifest, sysroot.path(), cas.path())
            .expect("install over existing file must succeed");

        let installed = fs::read(sysroot.path().join("usr/bin/hello")).unwrap();
        assert_eq!(installed, new_content, "old content must be replaced");
    }

    #[test]
    fn test_install_errors_on_missing_cas_object() {
        let cas = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();

        // Reference a hash that was never written to CAS.
        let missing_hash = "a".repeat(64);
        let files = vec![OutputFile {
            path: "/usr/bin/ghost".to_owned(),
            hash: missing_hash.clone(),
            size: 0,
            mode: 0o755,
        }];
        let manifest = make_manifest(files, vec![]);

        let err = install_to_sysroot(&manifest, sysroot.path(), cas.path())
            .expect_err("must fail with missing CAS object");

        match err {
            InstallError::MissingCasObject(h) => {
                assert_eq!(h, missing_hash);
            }
            other => panic!("expected MissingCasObject, got: {other}"),
        }
    }
}
