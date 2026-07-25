// conary-core/src/derivation/install.rs

//! Install derivation outputs from CAS into a live chroot sysroot.
//!
//! After a derivation is built and its outputs are stored in the
//! content-addressable store, `install_to_sysroot()` materialises those
//! outputs into a mutable sysroot directory. Files are hard-linked from CAS
//! when possible (same filesystem) and copied otherwise. Symlinks are
//! recreated verbatim. The function uses a last-writer-wins strategy: any
//! existing file or symlink at the destination is removed before writing.

use std::path::Path;

use tracing::debug;

use super::output::OutputManifest;
use crate::filesystem::CasStore;
use crate::generation::root_manifest::overlay_payload_entries;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while installing derivation outputs into a sysroot.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The exact output manifest failed validation.
    #[error("invalid output manifest: {0}")]
    InvalidManifest(String),
    /// The CAS could not be opened.
    #[error("CAS error: {0}")]
    Cas(String),
    /// Exact payload materialization failed.
    #[error("payload materialization failed: {0}")]
    Materialization(String),
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
    manifest
        .validate()
        .map_err(|error| InstallError::InvalidManifest(error.to_string()))?;
    let cas = CasStore::new(cas_dir).map_err(|error| InstallError::Cas(error.to_string()))?;
    let installed = overlay_payload_entries(&manifest.entries, &cas, sysroot)
        .map_err(|error| InstallError::Materialization(error.to_string()))?;
    debug!(
        root = %sysroot.display(),
        entries = installed,
        "installed exact derivation payload"
    );
    Ok(installed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs as unix_fs;
    use std::path::PathBuf;

    use super::*;
    use crate::generation::root_manifest::GenerationRootEntry;
    use crate::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
        ResolvedPayloadNode,
    };
    use tempfile::TempDir;

    fn node(kind: PayloadNodeKind, permissions: u32) -> ResolvedPayloadNode {
        let mode_type = match kind {
            PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. } => libc::S_IFREG,
            PayloadNodeKind::Directory => libc::S_IFDIR,
            PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
            PayloadNodeKind::BlockDevice { .. } => libc::S_IFBLK,
            PayloadNodeKind::CharacterDevice { .. } => libc::S_IFCHR,
            PayloadNodeKind::Fifo => libc::S_IFIFO,
            PayloadNodeKind::Socket => libc::S_IFSOCK,
        };
        let uid = u64::from(unsafe { libc::geteuid() });
        let gid = u64::from(unsafe { libc::getegid() });
        ResolvedPayloadNode {
            source: PayloadNode {
                kind,
                mode: mode_type | permissions,
                user: PayloadIdentity::Numeric { id: uid },
                group: PayloadIdentity::Numeric { id: gid },
                mtime: PayloadTimestamp::UNIX_EPOCH,
                xattrs: BTreeMap::new(),
            },
            uid,
            gid,
        }
    }

    fn directory(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: node(PayloadNodeKind::Directory, 0o755),
            content: None,
        }
    }

    fn regular(path: &str, hash: String, size: u64, mode: u32) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                mode,
            ),
            content: Some(PayloadContentAuthority { sha256: hash, size }),
        }
    }

    fn symlink(path: &str, target: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: node(
                PayloadNodeKind::Symlink {
                    target: target.to_string(),
                },
                0o777,
            ),
            content: None,
        }
    }

    fn make_manifest(entries: Vec<GenerationRootEntry>) -> OutputManifest {
        OutputManifest::new(
            "test-derivation",
            "test-package",
            "1.0",
            node(PayloadNodeKind::Directory, 0o755),
            entries,
            0,
            "2026-01-01T00:00:00Z".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn test_install_creates_files_and_symlinks() {
        let cas_dir = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();
        let cas = CasStore::new(cas_dir.path()).unwrap();

        let content = b"hello world";
        let file_hash = cas.store(content).unwrap();
        let manifest = make_manifest(vec![
            directory("/usr"),
            directory("/usr/bin"),
            regular("/usr/bin/hello", file_hash, content.len() as u64, 0o755),
            symlink("/usr/bin/hi", "hello"),
        ]);

        let count = install_to_sysroot(&manifest, sysroot.path(), cas_dir.path())
            .expect("install must succeed");
        assert_eq!(count, 4);

        let dest_file = sysroot.path().join("usr/bin/hello");
        assert_eq!(fs::read(&dest_file).unwrap(), content);
        let dest_link = sysroot.path().join("usr/bin/hi");
        let target = fs::read_link(&dest_link).expect("symlink must exist");
        assert_eq!(target.to_str().unwrap(), "hello");
    }

    #[test]
    fn test_install_overwrites_existing_file() {
        let cas_dir = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();
        let cas = CasStore::new(cas_dir.path()).unwrap();

        let dest_dir = sysroot.path().join("usr/bin");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("hello"), b"stale content").unwrap();

        let new_content = b"new content";
        let file_hash = cas.store(new_content).unwrap();
        let manifest = make_manifest(vec![
            directory("/usr"),
            directory("/usr/bin"),
            regular("/usr/bin/hello", file_hash, new_content.len() as u64, 0o644),
        ]);

        install_to_sysroot(&manifest, sysroot.path(), cas_dir.path())
            .expect("install over existing file must succeed");

        let installed = fs::read(sysroot.path().join("usr/bin/hello")).unwrap();
        assert_eq!(installed, new_content, "old content must be replaced");
    }

    #[test]
    fn test_install_replaces_existing_dangling_symlink_destination() {
        let cas_dir = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();

        let link_dir = sysroot.path().join("usr/lib/environment.d");
        fs::create_dir_all(&link_dir).unwrap();
        unix_fs::symlink(
            "../../../etc/environment",
            link_dir.join("99-environment.conf"),
        )
        .unwrap();

        let manifest = make_manifest(vec![
            directory("/usr"),
            directory("/usr/lib"),
            directory("/usr/lib/environment.d"),
            symlink(
                "/usr/lib/environment.d/99-environment.conf",
                "../../../etc/environment",
            ),
        ]);

        let count = install_to_sysroot(&manifest, sysroot.path(), cas_dir.path())
            .expect("install must replace a final dangling symlink");
        assert_eq!(count, 4);
        assert_eq!(
            fs::read_link(link_dir.join("99-environment.conf")).unwrap(),
            PathBuf::from("../../../etc/environment")
        );
    }

    #[test]
    fn test_install_replaces_symlink_ancestor_without_escaping() {
        let cas_dir = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let cas = CasStore::new(cas_dir.path()).unwrap();

        unix_fs::symlink(outside.path(), sysroot.path().join("usr")).unwrap();
        let content = b"outside write must not happen";
        let file_hash = cas.store(content).unwrap();
        let manifest = make_manifest(vec![
            directory("/usr"),
            directory("/usr/bin"),
            regular("/usr/bin/hello", file_hash, content.len() as u64, 0o755),
        ]);

        install_to_sysroot(&manifest, sysroot.path(), cas_dir.path()).unwrap();
        assert!(!outside.path().join("bin/hello").exists());
        assert_eq!(
            fs::read(sysroot.path().join("usr/bin/hello")).unwrap(),
            content
        );
    }

    #[test]
    fn test_install_errors_on_missing_cas_object() {
        let cas_dir = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();

        let manifest = make_manifest(vec![
            directory("/usr"),
            directory("/usr/bin"),
            regular("/usr/bin/ghost", "a".repeat(64), 0, 0o755),
        ]);

        let err = install_to_sysroot(&manifest, sysroot.path(), cas_dir.path())
            .expect_err("must fail with missing CAS object");
        assert!(matches!(err, InstallError::Materialization(_)));
    }

    #[test]
    fn shared_library_path_does_not_infer_ldconfig_mutation() {
        let cas_dir = TempDir::new().unwrap();
        let sysroot = TempDir::new().unwrap();
        let cas = CasStore::new(cas_dir.path()).unwrap();
        let content = b"not really a shared object";
        let file_hash = cas.store(content).unwrap();
        let manifest = make_manifest(vec![
            directory("/usr"),
            directory("/usr/lib"),
            regular(
                "/usr/lib/libexample.so",
                file_hash,
                content.len() as u64,
                0o644,
            ),
        ]);

        install_to_sysroot(&manifest, sysroot.path(), cas_dir.path())
            .expect("exact payload materialization must succeed");

        assert_eq!(
            fs::read(sysroot.path().join("usr/lib/libexample.so")).unwrap(),
            content
        );
        assert!(
            !sysroot.path().join("etc/ld.so.cache").exists(),
            "payload names alone must not schedule target-root lifecycle mutation"
        );
    }
}
