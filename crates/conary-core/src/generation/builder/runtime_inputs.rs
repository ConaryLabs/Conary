// conary-core/src/generation/builder/runtime_inputs.rs

use crate::db::models::{FileEntry, InstallSource};
use crate::filesystem::CasStore;

use super::{FileEntryRef, SymlinkEntryRef, hex_to_digest};

const S_IFMT: i32 = 0o170000;
const S_IFREG: i32 = 0o100000;
const S_IFDIR: i32 = 0o040000;
const S_IFLNK: i32 = 0o120000;

#[derive(Debug, Clone)]
enum ValidatedRuntimeEntry {
    Regular(FileEntryRef),
    Symlink(SymlinkEntryRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEntryKind {
    Regular,
    Symlink { target: String },
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEntryProblem {
    MissingSymlinkTarget,
    UnsupportedFileType(i32),
}

pub(super) fn is_generation_input_source(source: InstallSource) -> bool {
    matches!(
        source,
        InstallSource::AdoptedFull
            | InstallSource::Taken
            | InstallSource::Repository
            | InstallSource::File
    )
}

fn classify_file_entry(file: &FileEntry) -> Result<RuntimeEntryKind, RuntimeEntryProblem> {
    if let Some(target) = file
        .symlink_target
        .as_deref()
        .filter(|target| !target.is_empty())
    {
        return Ok(RuntimeEntryKind::Symlink {
            target: target.to_string(),
        });
    }

    match file.permissions & S_IFMT {
        S_IFLNK => Err(RuntimeEntryProblem::MissingSymlinkTarget),
        S_IFDIR => Ok(RuntimeEntryKind::Directory),
        S_IFREG | 0 => Ok(RuntimeEntryKind::Regular),
        other => Err(RuntimeEntryProblem::UnsupportedFileType(other)),
    }
}

fn validate_runtime_file_entry(
    package_name: &str,
    file: &FileEntry,
) -> crate::Result<Option<ValidatedRuntimeEntry>> {
    let kind = classify_file_entry(file).map_err(|problem| {
        let detail = match problem {
            RuntimeEntryProblem::MissingSymlinkTarget => {
                "symlink entry is missing symlink_target".to_string()
            }
            RuntimeEntryProblem::UnsupportedFileType(mode) => {
                format!("unsupported special file mode {mode:o} for generation root")
            }
        };
        runtime_input_error(package_name, &file.path, detail)
    })?;

    match kind {
        RuntimeEntryKind::Regular => {
            hex_to_digest(&file.sha256_hash).map_err(|error| {
                runtime_input_error(
                    package_name,
                    &file.path,
                    format!("invalid SHA-256 digest for regular file: {error}"),
                )
            })?;

            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;

            Ok(Some(ValidatedRuntimeEntry::Regular(FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
                owner: file.owner.clone(),
                group_name: file.group_name.clone(),
            })))
        }
        RuntimeEntryKind::Symlink { target } => {
            let expected = CasStore::compute_symlink_hash(&target);
            if file.sha256_hash != expected {
                return Err(runtime_input_error(
                    package_name,
                    &file.path,
                    format!(
                        "symlink hash mismatch: expected {expected}, got {}",
                        file.sha256_hash
                    ),
                ));
            }

            Ok(Some(ValidatedRuntimeEntry::Symlink(SymlinkEntryRef {
                path: file.path.clone(),
                target,
            })))
        }
        RuntimeEntryKind::Directory => Ok(None),
    }
}

fn runtime_input_error(
    package_name: &str,
    path: &str,
    detail: impl std::fmt::Display,
) -> crate::Error {
    crate::Error::InvalidPath(format!(
        "exportable runtime generation is not self-contained: package {package_name} has unresolved CAS-backed path {path}: {detail}. Run conary system adopt --system --full for bulk adoption, conary system adopt <pkg> --full for a single package, or conary system takeover --up-to cas before building this generation."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::FileEntry;
    use crate::filesystem::CasStore;

    fn file_entry(path: &str, hash: &str, mode: i32, trove_id: i64) -> FileEntry {
        let mut entry = FileEntry::new(path.to_string(), hash.to_string(), 0, mode, trove_id);
        entry.owner = Some("root".to_string());
        entry.group_name = Some("root".to_string());
        entry
    }

    fn symlink_entry(path: &str, target: &str, hash: &str, mode: i32, trove_id: i64) -> FileEntry {
        let mut entry = file_entry(path, hash, mode, trove_id);
        entry.symlink_target = Some(target.to_string());
        entry
    }

    fn assert_error_contains<T: std::fmt::Debug>(result: crate::Result<T>, snippets: &[&str]) {
        let error = result.unwrap_err().to_string();
        for snippet in snippets {
            assert!(
                error.contains(snippet),
                "expected error to contain {snippet:?}; got {error}"
            );
        }
    }

    #[test]
    fn generation_input_source_classification_is_not_is_conary_owned() {
        assert!(!is_generation_input_source(InstallSource::AdoptedTrack));
        assert!(is_generation_input_source(InstallSource::AdoptedFull));
        assert!(is_generation_input_source(InstallSource::Taken));
        assert!(is_generation_input_source(InstallSource::Repository));
        assert!(is_generation_input_source(InstallSource::File));
    }

    #[test]
    fn non_empty_symlink_target_wins_over_mode_bits() {
        let target = "../lib/systemd/systemd";
        let hash = CasStore::compute_symlink_hash(target);
        let entry = symlink_entry("/usr/sbin/init", target, &hash, 0o100755, 1);

        assert_eq!(
            classify_file_entry(&entry).unwrap(),
            RuntimeEntryKind::Symlink {
                target: target.to_string()
            }
        );
    }

    #[test]
    fn symlink_mode_without_target_fails_with_package_path_and_remediation() {
        let entry = file_entry("/usr/lib/libfoo.so", &"a".repeat(64), 0o120777, 1);

        assert_eq!(
            classify_file_entry(&entry),
            Err(RuntimeEntryProblem::MissingSymlinkTarget)
        );
        assert_error_contains(
            validate_runtime_file_entry("glibc", &entry),
            &[
                "exportable runtime generation is not self-contained",
                "package glibc",
                "/usr/lib/libfoo.so",
                "symlink entry is missing symlink_target",
                "conary system adopt --system --full",
                "conary system takeover --up-to cas",
            ],
        );
    }

    #[test]
    fn directory_entries_bypass_digest_validation_and_are_not_erofs_inputs() {
        let entry = file_entry("/usr/share/doc", "directory-placeholder", 0o040755, 1);

        assert_eq!(
            classify_file_entry(&entry).unwrap(),
            RuntimeEntryKind::Directory
        );
        assert!(
            validate_runtime_file_entry("filesystem", &entry)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn bare_permission_only_mode_defaults_to_regular_file() {
        let entry = file_entry("/usr/bin/true", &"b".repeat(64), 0o755, 1);

        assert_eq!(
            classify_file_entry(&entry).unwrap(),
            RuntimeEntryKind::Regular
        );
        match validate_runtime_file_entry("coreutils", &entry)
            .unwrap()
            .unwrap()
        {
            ValidatedRuntimeEntry::Regular(file) => {
                assert_eq!(file.path, "/usr/bin/true");
                assert_eq!(file.permissions, 0o755);
            }
            other => panic!("expected regular entry, got {other:?}"),
        }
    }

    #[test]
    fn included_special_files_fail_clearly() {
        for (path, mode) in [
            ("/usr/lib/systemd/fifo", 0o010644),
            ("/etc/kmsg", 0o020600),
            ("/boot/socket", 0o140777),
        ] {
            let entry = file_entry(path, &"c".repeat(64), mode, 1);

            assert_error_contains(
                validate_runtime_file_entry("systemd", &entry),
                &[
                    "package systemd",
                    path,
                    "unsupported special file mode",
                    "conary system adopt <pkg> --full",
                ],
            );
        }
    }

    #[test]
    fn regular_file_invalid_digest_fails_through_shared_parser() {
        let entry = file_entry("/usr/bin/false", "not-a-sha256", 0o100755, 1);

        assert_error_contains(
            validate_runtime_file_entry("coreutils", &entry),
            &[
                "package coreutils",
                "/usr/bin/false",
                "invalid SHA-256 digest for regular file",
                "Expected 64-char hex digest",
            ],
        );
    }

    #[test]
    fn symlink_hash_must_match_target_hash() {
        let target = "libfoo.so.1";
        let hash = CasStore::compute_symlink_hash(target);
        let valid = symlink_entry("/usr/lib/libfoo.so", target, &hash, 0o120777, 1);

        match validate_runtime_file_entry("glibc", &valid)
            .unwrap()
            .unwrap()
        {
            ValidatedRuntimeEntry::Symlink(symlink) => {
                assert_eq!(symlink.path, "/usr/lib/libfoo.so");
                assert_eq!(symlink.target, target);
            }
            other => panic!("expected symlink entry, got {other:?}"),
        }

        let invalid = symlink_entry("/usr/lib/libfoo.so", target, &"d".repeat(64), 0o120777, 1);
        assert_error_contains(
            validate_runtime_file_entry("glibc", &invalid),
            &[
                "package glibc",
                "/usr/lib/libfoo.so",
                "symlink hash mismatch",
                &hash,
            ],
        );
    }
}
