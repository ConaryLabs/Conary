// apps/conary/src/commands/adopt/cas_capture.rs

use anyhow::{Result, anyhow};
use conary_core::filesystem::CasStore;
use conary_core::generation::metadata::is_excluded;

use super::FileInfoTuple;

const S_IFMT: i32 = 0o170000;
const S_IFREG: i32 = 0o100000;
const S_IFDIR: i32 = 0o040000;
const S_IFLNK: i32 = 0o120000;

pub(crate) fn compute_cas_backed_file_hash(
    file_path: &str,
    file_mode: i32,
    file_digest: Option<&str>,
    link_target: Option<&str>,
    cas: &CasStore,
) -> Result<String> {
    if is_excluded(file_path) {
        return Ok(file_digest
            .map(str::to_string)
            .unwrap_or_else(|| stable_placeholder("excluded", file_path)));
    }

    if let Some(target) = link_target.filter(|target| !target.is_empty()) {
        return Ok(CasStore::compute_symlink_hash(target));
    }

    match file_mode & S_IFMT {
        S_IFLNK => {
            let target = std::fs::read_link(file_path).map_err(|e| {
                anyhow!("{file_path}: symlink target is required and could not be read: {e}")
            })?;
            Ok(CasStore::compute_symlink_hash(&target.to_string_lossy()))
        }
        S_IFDIR => Ok(file_digest
            .map(str::to_string)
            .unwrap_or_else(|| stable_placeholder("directory", file_path))),
        S_IFREG | 0 => {
            let path = std::path::Path::new(file_path);
            let metadata = std::fs::metadata(path).map_err(|e| {
                anyhow!("{file_path}: regular file must be readable before CAS storage: {e}")
            })?;
            if !metadata.file_type().is_file() {
                return Err(anyhow!(
                    "{file_path}: regular file must be readable before CAS storage"
                ));
            }
            cas.store_file_copy_from_existing(path)
                .map_err(|e| anyhow!("{file_path}: regular file could not be stored in CAS: {e}"))
        }
        other => Err(anyhow!(
            "{file_path}: unsupported special file mode {other:o} for full adoption"
        )),
    }
}

pub(crate) fn prepare_cas_backed_package_files(
    package_name: &str,
    files: &[FileInfoTuple],
    cas: &CasStore,
) -> Result<Vec<(FileInfoTuple, String)>> {
    files
        .iter()
        .map(|file| {
            compute_cas_backed_file_hash(&file.0, file.2, file.3.as_deref(), file.6.as_deref(), cas)
                .map(|hash| (file.clone(), hash))
                .map_err(|error| {
                    anyhow!(
                        "package {package_name} has unresolved CAS-backed path {}: {error}",
                        file.0
                    )
                })
        })
        .collect()
}

fn stable_placeholder(prefix: &str, file_path: &str) -> String {
    format!("{prefix}-{}", file_path.replace('/', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::filesystem::CasStore;

    fn tempdir_in_target() -> tempfile::TempDir {
        std::fs::create_dir_all("target").unwrap();
        tempfile::Builder::new()
            .prefix("conary-cas-capture-")
            .tempdir_in("target")
            .unwrap()
    }

    fn file_tuple(
        path: &str,
        size: i64,
        mode: i32,
        digest: Option<&str>,
        link_target: Option<&str>,
    ) -> super::super::FileInfoTuple {
        (
            path.to_string(),
            size,
            mode,
            digest.map(str::to_string),
            Some("root".to_string()),
            Some("root".to_string()),
            link_target.map(str::to_string),
        )
    }

    fn assert_error_contains(error: anyhow::Error, snippets: &[&str]) {
        let error = error.to_string();
        for snippet in snippets {
            assert!(
                error.contains(snippet),
                "expected error to contain {snippet:?}; got {error}"
            );
        }
    }

    #[test]
    fn full_adoption_regular_file_requires_cas_storage() {
        let tmp = tempdir_in_target();
        let source = tmp.path().join("hello");
        std::fs::write(&source, b"hello").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let source = source.strip_prefix(cwd).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let hash = compute_cas_backed_file_hash(
            source.to_str().unwrap(),
            0o100644,
            Some("package-manager-digest"),
            None,
            &cas,
        )
        .unwrap();

        assert_ne!(hash, "package-manager-digest");
        assert_eq!(cas.retrieve(&hash).unwrap(), b"hello");
    }

    #[test]
    fn full_adoption_cas_survives_in_place_source_mutation() {
        let tmp = tempdir_in_target();
        let source = tmp.path().join("mutable-source");
        std::fs::write(&source, b"original bytes").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let source_arg = source.strip_prefix(cwd).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let hash = compute_cas_backed_file_hash(
            source_arg.to_str().unwrap(),
            0o100644,
            Some("package-manager-digest"),
            None,
            &cas,
        )
        .unwrap();

        std::fs::write(&source, b"mutated bytes").unwrap();

        assert_eq!(cas.retrieve(&hash).unwrap(), b"original bytes");
    }

    #[test]
    #[cfg(unix)]
    fn full_adoption_regular_file_uses_private_cas_inode() {
        use std::os::unix::fs::MetadataExt;

        let tmp = tempdir_in_target();
        let source = tmp.path().join("private-inode-source");
        std::fs::write(&source, b"private inode bytes").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let source_arg = source.strip_prefix(cwd).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let hash = compute_cas_backed_file_hash(
            source_arg.to_str().unwrap(),
            0o100644,
            Some("package-manager-digest"),
            None,
            &cas,
        )
        .unwrap();
        let cas_path = cas.hash_to_path(&hash).unwrap();

        assert_ne!(
            std::fs::metadata(&source).unwrap().ino(),
            std::fs::metadata(&cas_path).unwrap().ino(),
            "live full adoption must not share an inode with mutable source files"
        );
    }

    #[test]
    fn full_adoption_symlink_hashes_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let hash = compute_cas_backed_file_hash(
            "/usr/lib/libfoo.so",
            0o120777,
            Some("package-manager-digest"),
            Some("libfoo.so.1"),
            &cas,
        )
        .unwrap();

        assert_eq!(hash, CasStore::compute_symlink_hash("libfoo.so.1"));
    }

    #[test]
    fn full_adoption_directory_does_not_require_cas_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let hash = compute_cas_backed_file_hash(
            "/usr/share/doc",
            0o040755,
            Some("directory-digest"),
            None,
            &cas,
        )
        .unwrap();

        assert_eq!(hash, "directory-digest");
    }

    #[test]
    fn full_adoption_excluded_paths_do_not_block_on_special_or_missing_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let hash =
            compute_cas_backed_file_hash("/var/run/socket", 0o140777, None, None, &cas).unwrap();

        assert_eq!(hash, "excluded-_var_run_socket");
    }

    #[test]
    fn full_adoption_non_excluded_special_file_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let error =
            compute_cas_backed_file_hash("/etc/kmsg", 0o020600, None, None, &cas).unwrap_err();

        assert_error_contains(error, &["/etc/kmsg", "unsupported special file"]);
    }

    #[test]
    fn full_adoption_missing_regular_file_fails_package_preparation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let files = vec![file_tuple(
            "/usr/bin/missing",
            7,
            0o100755,
            Some("package-manager-digest"),
            None,
        )];

        let error = prepare_cas_backed_package_files("broken", &files, &cas).unwrap_err();

        assert_error_contains(
            error,
            &[
                "package broken",
                "/usr/bin/missing",
                "regular file must be readable",
            ],
        );
    }
}
