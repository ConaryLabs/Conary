// crates/conary-core/src/bootstrap/build_helpers.rs

//! Shared build utilities for bootstrap builders
//!
//! Common operations shared across bootstrap phases: extract tarballs,
//! find source directories, expand environment variables, set up sandbox
//! environments, and run shell commands.
//!
//! Some functions are not yet called by the current phase implementations
//! but are retained for use when recipe-driven execution is wired end-to-end.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Determine the tar flag based on archive filename extension
pub fn tar_flag_for_archive(filename: &str) -> &'static str {
    if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
        "xJf"
    } else if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        "xzf"
    } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
        "xjf"
    } else if filename.ends_with(".tar.zst") || filename.ends_with(".tzst") {
        "--zstd -xf"
    } else {
        "xf"
    }
}

/// Extract a tar archive to a destination directory
///
/// If `strip_components` is true, strips the top-level directory from the
/// archive (useful for in-tree dependencies like GMP, MPFR, MPC).
pub fn extract_tar(archive: &Path, dest: &Path, strip_components: bool) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    let archive_str = archive
        .to_str()
        .ok_or_else(|| format!("archive path is not valid UTF-8: {}", archive.display()))?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| format!("dest path is not valid UTF-8: {}", dest.display()))?;
    let filename = archive
        .file_name()
        .ok_or_else(|| format!("archive path has no filename: {}", archive.display()))?
        .to_string_lossy();

    let flag = tar_flag_for_archive(&filename);

    let mut cmd = Command::new("tar");
    for part in flag.split_whitespace() {
        cmd.arg(part);
    }
    cmd.args([archive_str, "-C", dest_str]);
    if strip_components {
        cmd.arg("--strip-components=1");
    }
    cmd.arg("--no-same-owner");
    cmd.arg("--no-same-permissions");

    let output = cmd.output().map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tar extraction failed: {stderr}"));
    }

    Ok(())
}

/// Find the actual source directory after extraction
///
/// If the archive extracted into a single top-level directory, returns that
/// directory. Otherwise returns the extraction directory itself.
pub fn find_source_dir(dir: &Path) -> Result<PathBuf, std::io::Error> {
    let entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();

    if entries.len() == 1 {
        Ok(entries[0].path())
    } else {
        Ok(dir.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tar_flag_for_archive() {
        assert_eq!(tar_flag_for_archive("foo.tar.xz"), "xJf");
        assert_eq!(tar_flag_for_archive("foo.txz"), "xJf");
        assert_eq!(tar_flag_for_archive("foo.tar.gz"), "xzf");
        assert_eq!(tar_flag_for_archive("foo.tgz"), "xzf");
        assert_eq!(tar_flag_for_archive("foo.tar.bz2"), "xjf");
        assert_eq!(tar_flag_for_archive("foo.tbz2"), "xjf");
        assert_eq!(tar_flag_for_archive("foo.tar"), "xf");
        assert_eq!(tar_flag_for_archive("foo.tar.zst"), "--zstd -xf");
    }

    #[test]
    fn test_find_source_dir_single() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("foo-1.0")).unwrap();
        let result = find_source_dir(dir.path()).unwrap();
        assert_eq!(result, dir.path().join("foo-1.0"));
    }

    #[test]
    fn test_find_source_dir_multiple() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("b")).unwrap();
        let result = find_source_dir(dir.path()).unwrap();
        assert_eq!(result, dir.path());
    }
}
