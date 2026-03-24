// conary-core/src/derivation/compose.rs

//! EROFS composition -- merge multiple package outputs into a single image.
//!
//! After derivation builds produce [`OutputManifest`]s, this module composes
//! their file entries and symlinks into a unified set suitable for
//! [`build_erofs_image`]. Path conflicts are resolved with last-writer-wins
//! semantics (later manifests override earlier ones).

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::derivation::output::OutputManifest;
use crate::generation::builder::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image};

/// Errors that can occur during EROFS composition.
#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    /// No package outputs were provided for composition.
    #[error("empty composition: no package outputs to compose")]
    EmptyComposition,

    /// The EROFS builder returned an error.
    #[error("EROFS build error: {0}")]
    Erofs(String),

    /// An I/O operation failed.
    #[error("I/O error: {0}")]
    Io(String),
}

/// Merge files from multiple [`OutputManifest`]s into a single [`FileEntryRef`] list.
///
/// Files are keyed by absolute path in a [`BTreeMap`] for deterministic
/// iteration order. When multiple manifests contain the same path, the entry
/// from the *last* manifest in the slice wins (last-writer-wins).
///
/// Relative paths (those not starting with `/`) are converted to absolute by
/// prefixing with `/`.
#[must_use]
pub fn compose_file_entries(manifests: &[&OutputManifest]) -> Vec<FileEntryRef> {
    let mut merged: BTreeMap<String, FileEntryRef> = BTreeMap::new();

    for manifest in manifests {
        for file in &manifest.files {
            let abs_path = if file.path.starts_with('/') {
                file.path.clone()
            } else {
                format!("/{}", file.path)
            };

            merged.insert(
                abs_path.clone(),
                FileEntryRef {
                    path: abs_path,
                    sha256_hash: file.hash.clone(),
                    size: file.size,
                    permissions: file.mode,
                },
            );
        }
    }

    merged.into_values().collect()
}

/// Composed file entries and symlinks from multiple package outputs.
///
/// Produced by [`compose_entries`], this struct holds both files and symlinks
/// ready for [`build_erofs_image`].
#[derive(Debug, Clone)]
pub struct ComposedEntries {
    /// Merged file entries (deduplicated by path, last-writer-wins).
    pub files: Vec<FileEntryRef>,
    /// Merged symlink entries (deduplicated by path, last-writer-wins).
    pub symlinks: Vec<SymlinkEntryRef>,
}

/// Merge files AND symlinks from multiple [`OutputManifest`]s.
///
/// Both files and symlinks are keyed by absolute path in [`BTreeMap`]s for
/// deterministic iteration order. When multiple manifests contain the same
/// path, the entry from the *last* manifest wins (last-writer-wins).
///
/// Relative paths (those not starting with `/`) are converted to absolute by
/// prefixing with `/`.
#[must_use]
pub fn compose_entries(manifests: &[&OutputManifest]) -> ComposedEntries {
    let mut merged_files: BTreeMap<String, FileEntryRef> = BTreeMap::new();
    let mut merged_symlinks: BTreeMap<String, SymlinkEntryRef> = BTreeMap::new();

    for manifest in manifests {
        for file in &manifest.files {
            let abs_path = if file.path.starts_with('/') {
                file.path.clone()
            } else {
                format!("/{}", file.path)
            };

            merged_files.insert(
                abs_path.clone(),
                FileEntryRef {
                    path: abs_path,
                    sha256_hash: file.hash.clone(),
                    size: file.size,
                    permissions: file.mode,
                },
            );
        }

        for symlink in &manifest.symlinks {
            let abs_path = if symlink.path.starts_with('/') {
                symlink.path.clone()
            } else {
                format!("/{}", symlink.path)
            };

            merged_symlinks.insert(
                abs_path.clone(),
                SymlinkEntryRef {
                    path: abs_path,
                    target: symlink.target.clone(),
                },
            );
        }
    }

    ComposedEntries {
        files: merged_files.into_values().collect(),
        symlinks: merged_symlinks.into_values().collect(),
    }
}

/// Compose multiple package outputs into a single EROFS image.
///
/// Merges all file entries and symlinks via [`compose_entries`], then delegates
/// to [`build_erofs_image`] to produce the image at `output_dir/root.erofs`.
///
/// # Errors
///
/// - [`ComposeError::EmptyComposition`] if `manifests` is empty.
/// - [`ComposeError::Erofs`] if the EROFS builder fails.
pub fn compose_erofs(
    manifests: &[&OutputManifest],
    output_dir: &Path,
) -> Result<BuildResult, ComposeError> {
    if manifests.is_empty() {
        return Err(ComposeError::EmptyComposition);
    }

    let composed = compose_entries(manifests);

    build_erofs_image(&composed.files, &composed.symlinks, output_dir)
        .map_err(|e| ComposeError::Erofs(e.to_string()))
}

/// Compute the SHA-256 hash of an EROFS image file.
///
/// The returned hex string can be used as `build_env_hash` in derivation
/// inputs, providing a content-addressed identifier for the composed image.
///
/// # Errors
///
/// - [`ComposeError::Io`] if the file cannot be read.
pub fn erofs_image_hash(image_path: &Path) -> Result<String, ComposeError> {
    let bytes = std::fs::read(image_path)
        .map_err(|e| ComposeError::Io(format!("{}: {e}", image_path.display())))?;

    Ok(hex::encode(Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::output::{OutputFile, OutputManifest, OutputSymlink};

    /// Build a minimal `OutputManifest` with the given files and no symlinks.
    fn manifest_with_files(files: Vec<OutputFile>) -> OutputManifest {
        OutputManifest {
            derivation_id: "d".repeat(64),
            output_hash: "e".repeat(64),
            hash_version: 1,
            files,
            symlinks: vec![],
            build_duration_secs: 1,
            built_at: "2026-03-19T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn compose_merges_files_from_multiple_outputs() {
        let m1 = manifest_with_files(vec![OutputFile {
            path: "/usr/bin/hello".to_owned(),
            hash: "a".repeat(64),
            size: 100,
            mode: 0o755,
        }]);

        let m2 = manifest_with_files(vec![OutputFile {
            path: "/usr/lib/libfoo.so".to_owned(),
            hash: "b".repeat(64),
            size: 200,
            mode: 0o644,
        }]);

        let entries = compose_file_entries(&[&m1, &m2]);

        assert_eq!(entries.len(), 2);

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/usr/bin/hello"));
        assert!(paths.contains(&"/usr/lib/libfoo.so"));
    }

    #[test]
    fn last_writer_wins_on_path_conflicts() {
        let m1 = manifest_with_files(vec![OutputFile {
            path: "/usr/bin/hello".to_owned(),
            hash: "a".repeat(64),
            size: 100,
            mode: 0o755,
        }]);

        let m2 = manifest_with_files(vec![OutputFile {
            path: "/usr/bin/hello".to_owned(),
            hash: "b".repeat(64),
            size: 200,
            mode: 0o644,
        }]);

        let entries = compose_file_entries(&[&m1, &m2]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha256_hash, "b".repeat(64));
        assert_eq!(entries[0].size, 200);
        assert_eq!(entries[0].permissions, 0o644);
    }

    #[test]
    fn relative_paths_become_absolute() {
        let m = manifest_with_files(vec![
            OutputFile {
                path: "usr/bin/hello".to_owned(),
                hash: "a".repeat(64),
                size: 100,
                mode: 0o755,
            },
            OutputFile {
                path: "/usr/lib/libfoo.so".to_owned(),
                hash: "b".repeat(64),
                size: 200,
                mode: 0o644,
            },
        ]);

        let entries = compose_file_entries(&[&m]);

        for entry in &entries {
            assert!(
                entry.path.starts_with('/'),
                "path '{}' should be absolute",
                entry.path
            );
        }
    }

    #[test]
    fn deterministic_output_order() {
        let m = manifest_with_files(vec![
            OutputFile {
                path: "/z/last".to_owned(),
                hash: "c".repeat(64),
                size: 300,
                mode: 0o644,
            },
            OutputFile {
                path: "/a/first".to_owned(),
                hash: "a".repeat(64),
                size: 100,
                mode: 0o755,
            },
            OutputFile {
                path: "/m/middle".to_owned(),
                hash: "b".repeat(64),
                size: 200,
                mode: 0o644,
            },
        ]);

        let entries = compose_file_entries(&[&m]);

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/a/first", "/m/middle", "/z/last"]);

        // Run again to confirm determinism.
        let entries2 = compose_file_entries(&[&m]);
        let paths2: Vec<&str> = entries2.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, paths2);
    }

    #[test]
    fn empty_composition_returns_error() {
        let empty: Vec<&OutputManifest> = vec![];
        let result = compose_erofs(&empty, Path::new("/nonexistent"));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ComposeError::EmptyComposition),
            "expected EmptyComposition, got: {err}"
        );
    }

    #[test]
    fn compose_file_entries_with_no_manifests_returns_empty() {
        let entries = compose_file_entries(&[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn erofs_image_hash_missing_file() {
        let result = erofs_image_hash(Path::new("/nonexistent/file.erofs"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ComposeError::Io(_)));
    }

    #[test]
    fn erofs_image_hash_produces_valid_hex() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.erofs");
        std::fs::write(&file, b"fake erofs content").unwrap();

        let hash = erofs_image_hash(&file).unwrap();

        assert_eq!(hash.len(), 64, "SHA-256 hex should be 64 chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be valid hex"
        );
    }

    // ---------------------------------------------------------------
    // Symlink composition tests
    // ---------------------------------------------------------------

    /// Build a minimal `OutputManifest` with the given files and symlinks.
    fn manifest_with_files_and_symlinks(
        files: Vec<OutputFile>,
        symlinks: Vec<OutputSymlink>,
    ) -> OutputManifest {
        OutputManifest {
            derivation_id: "d".repeat(64),
            output_hash: "e".repeat(64),
            hash_version: 1,
            files,
            symlinks,
            build_duration_secs: 1,
            built_at: "2026-03-19T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn compose_includes_symlinks_from_manifests() {
        let m1 = manifest_with_files_and_symlinks(
            vec![OutputFile {
                path: "/usr/lib/libfoo.so".to_owned(),
                hash: "a".repeat(64),
                size: 4096,
                mode: 0o644,
            }],
            vec![OutputSymlink {
                path: "/usr/lib/libfoo.so.1".to_owned(),
                target: "libfoo.so".to_owned(),
            }],
        );

        let m2 = manifest_with_files_and_symlinks(
            vec![OutputFile {
                path: "/usr/lib/libbar.so".to_owned(),
                hash: "b".repeat(64),
                size: 2048,
                mode: 0o644,
            }],
            vec![OutputSymlink {
                path: "/usr/lib/libbar.so.2".to_owned(),
                target: "libbar.so".to_owned(),
            }],
        );

        let composed = compose_entries(&[&m1, &m2]);

        assert_eq!(composed.files.len(), 2, "should have 2 files");
        assert_eq!(composed.symlinks.len(), 2, "should have 2 symlinks");

        let symlink_paths: Vec<&str> = composed.symlinks.iter().map(|s| s.path.as_str()).collect();
        assert!(symlink_paths.contains(&"/usr/lib/libfoo.so.1"));
        assert!(symlink_paths.contains(&"/usr/lib/libbar.so.2"));

        let foo_symlink = composed
            .symlinks
            .iter()
            .find(|s| s.path == "/usr/lib/libfoo.so.1")
            .unwrap();
        assert_eq!(foo_symlink.target, "libfoo.so");
    }

    #[test]
    fn symlink_last_writer_wins() {
        let m1 = manifest_with_files_and_symlinks(
            vec![],
            vec![OutputSymlink {
                path: "/usr/lib/libfoo.so.1".to_owned(),
                target: "libfoo.so.1.0".to_owned(),
            }],
        );

        let m2 = manifest_with_files_and_symlinks(
            vec![],
            vec![OutputSymlink {
                path: "/usr/lib/libfoo.so.1".to_owned(),
                target: "libfoo.so.1.1".to_owned(),
            }],
        );

        let composed = compose_entries(&[&m1, &m2]);

        assert_eq!(composed.symlinks.len(), 1, "should deduplicate by path");
        assert_eq!(
            composed.symlinks[0].target, "libfoo.so.1.1",
            "last manifest should win"
        );
    }

    #[test]
    fn compose_entries_relative_symlink_paths_become_absolute() {
        let m = manifest_with_files_and_symlinks(
            vec![],
            vec![OutputSymlink {
                path: "usr/lib/libfoo.so.1".to_owned(),
                target: "libfoo.so".to_owned(),
            }],
        );

        let composed = compose_entries(&[&m]);

        assert_eq!(composed.symlinks.len(), 1);
        assert!(
            composed.symlinks[0].path.starts_with('/'),
            "symlink path '{}' should be absolute",
            composed.symlinks[0].path
        );
    }

    #[test]
    fn compose_entries_with_no_manifests_returns_empty() {
        let composed = compose_entries(&[]);
        assert!(composed.files.is_empty());
        assert!(composed.symlinks.is_empty());
    }
}
