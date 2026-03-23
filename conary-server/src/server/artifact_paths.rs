// conary-server/src/server/artifact_paths.rs
//! Shared artifact path utilities for admin upload and public serving handlers.
//!
//! Both `handlers/admin/artifacts.rs` (upload) and `handlers/artifacts.rs`
//! (public read) need identical path sanitization and root resolution.
//! This module provides the shared implementation.

use crate::server::ServerState;
use std::path::{Path, PathBuf};

/// Which artifact tree to resolve against.
pub enum ArtifactRoot {
    Fixtures,
    Artifacts,
}

/// Derive the top-level storage directory from server config.
///
/// Since `chunk_dir` is `{storage_root}/chunks`, we go one level up
/// to reach the storage root.
pub fn storage_root(state: &ServerState) -> &Path {
    state
        .config
        .chunk_dir
        .parent()
        .unwrap_or(&state.config.chunk_dir)
}

/// Resolve the full artifact root directory for the given tree.
pub fn artifact_root(state: &ServerState, root: ArtifactRoot) -> PathBuf {
    match root {
        ArtifactRoot::Fixtures => storage_root(state).join("test-fixtures"),
        ArtifactRoot::Artifacts => storage_root(state).join("test-artifacts"),
    }
}

/// Validate and normalize a user-supplied relative path.
///
/// Each segment must be non-empty, not `.` or `..`, free of null bytes,
/// and contain only `[a-zA-Z0-9._-]`. The combined path must not be empty.
pub fn sanitize_relative_path(path: &str) -> Result<PathBuf, &'static str> {
    let mut relative = PathBuf::new();
    for segment in path.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('\0')
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err("Invalid artifact path");
        }
        relative.push(segment);
    }

    if relative.as_os_str().is_empty() {
        return Err("Artifact path must not be empty");
    }

    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_valid_paths() {
        assert_eq!(
            sanitize_relative_path("foo/bar.ccs").unwrap(),
            PathBuf::from("foo/bar.ccs")
        );
        assert_eq!(
            sanitize_relative_path("a-b_c.1").unwrap(),
            PathBuf::from("a-b_c.1")
        );
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize_relative_path("../secret").is_err());
        assert!(sanitize_relative_path("foo/../../bar").is_err());
    }

    #[test]
    fn sanitize_rejects_empty() {
        assert!(sanitize_relative_path("").is_err());
        assert!(sanitize_relative_path("foo//bar").is_err());
    }

    #[test]
    fn sanitize_rejects_dot() {
        assert!(sanitize_relative_path(".").is_err());
        assert!(sanitize_relative_path("foo/.").is_err());
    }

    #[test]
    fn sanitize_rejects_null() {
        assert!(sanitize_relative_path("foo\0bar").is_err());
    }

    #[test]
    fn sanitize_rejects_special_chars() {
        assert!(sanitize_relative_path("foo/b@r").is_err());
        assert!(sanitize_relative_path("foo/b r").is_err());
    }
}
