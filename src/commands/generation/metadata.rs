// src/commands/generation/metadata.rs
//! Generation metadata types and path helpers

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directories excluded from generation trees
pub const EXCLUDED_DIRS: &[&str] = &[
    "home", "proc", "sys", "dev", "run", "tmp", "mnt", "media", "var/lib",
];

/// Standard root-level symlinks (source -> target)
pub const ROOT_SYMLINKS: &[(&str, &str)] = &[
    ("bin", "usr/bin"),
    ("lib", "usr/lib"),
    ("lib64", "usr/lib64"),
    ("sbin", "usr/sbin"),
];

/// Metadata for a single generation snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetadata {
    pub generation: i64,
    pub created_at: String,
    pub package_count: i64,
    pub kernel_version: Option<String>,
    pub summary: String,
}

impl GenerationMetadata {
    /// Write metadata to `.conary-gen.json` inside the given generation directory
    pub fn write_to(&self, gen_dir: &Path) -> Result<()> {
        let path = gen_dir.join(".conary-gen.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Read metadata from `.conary-gen.json` inside the given generation directory
    pub fn read_from(gen_dir: &Path) -> Result<Self> {
        let path = gen_dir.join(".conary-gen.json");
        let json = std::fs::read_to_string(path)?;
        let metadata: Self = serde_json::from_str(&json)?;
        Ok(metadata)
    }
}

/// Returns the base directory for all generations: `/conary/generations`
pub fn generations_dir() -> PathBuf {
    PathBuf::from("/conary/generations")
}

/// Returns the directory for a specific generation: `/conary/generations/{number}`
pub fn generation_path(number: i64) -> PathBuf {
    PathBuf::from(format!("/conary/generations/{number}"))
}

/// Returns the symlink pointing to the current active generation: `/conary/current`
pub fn current_link() -> PathBuf {
    PathBuf::from("/conary/current")
}

/// Returns the directory for GC roots: `/conary/gc-roots`
pub fn gc_roots_dir() -> PathBuf {
    PathBuf::from("/conary/gc-roots")
}

/// Detect kernel version(s) by scanning `gen_dir/usr/lib/modules/` for subdirectories
pub fn detect_kernel_version(gen_dir: &Path) -> Option<String> {
    let modules_dir = gen_dir.join("usr/lib/modules");
    let entries = std::fs::read_dir(modules_dir).ok()?;

    for entry in entries.flatten() {
        if entry.file_type().ok()?.is_dir() {
            return Some(entry.file_name().to_string_lossy().into_owned());
        }
    }

    None
}

/// Check if a path should be excluded from generation trees
///
/// Strips a leading `/` before comparing against `EXCLUDED_DIRS`.
pub fn is_excluded(path: &str) -> bool {
    let path = path.strip_prefix('/').unwrap_or(path);
    EXCLUDED_DIRS
        .iter()
        .any(|dir| path == *dir || path.starts_with(&format!("{dir}/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_metadata_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let metadata = GenerationMetadata {
            generation: 42,
            created_at: "2026-03-04T12:00:00Z".to_string(),
            package_count: 150,
            kernel_version: Some("6.12.1-arch1-1".to_string()),
            summary: "installed vim".to_string(),
        };

        metadata.write_to(tmp.path()).unwrap();
        let loaded = GenerationMetadata::read_from(tmp.path()).unwrap();

        assert_eq!(loaded.generation, 42);
        assert_eq!(loaded.created_at, "2026-03-04T12:00:00Z");
        assert_eq!(loaded.package_count, 150);
        assert_eq!(loaded.kernel_version.as_deref(), Some("6.12.1-arch1-1"));
        assert_eq!(loaded.summary, "installed vim");
    }

    #[test]
    fn test_excluded_paths() {
        // These should be excluded
        assert!(is_excluded("home"));
        assert!(is_excluded("/home"));
        assert!(is_excluded("home/peter"));
        assert!(is_excluded("proc"));
        assert!(is_excluded("/proc/cpuinfo"));
        assert!(is_excluded("var/lib"));
        assert!(is_excluded("/var/lib/dpkg"));
        assert!(is_excluded("sys"));
        assert!(is_excluded("dev"));

        // These should NOT be excluded
        assert!(!is_excluded("usr"));
        assert!(!is_excluded("etc"));
        assert!(!is_excluded("var/cache"));
        assert!(!is_excluded("/usr/bin"));
    }

    #[test]
    fn test_generation_paths() {
        assert_eq!(generations_dir(), PathBuf::from("/conary/generations"));
        assert_eq!(
            generation_path(1),
            PathBuf::from("/conary/generations/1")
        );
        assert_eq!(
            generation_path(42),
            PathBuf::from("/conary/generations/42")
        );
        assert_eq!(current_link(), PathBuf::from("/conary/current"));
        assert_eq!(gc_roots_dir(), PathBuf::from("/conary/gc-roots"));
    }
}
