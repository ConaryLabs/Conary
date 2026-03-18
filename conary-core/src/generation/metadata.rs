// conary-core/src/generation/metadata.rs

//! Generation metadata types and path helpers.
//!
//! These live in conary-core so the transaction engine can create and
//! inspect generation metadata without depending on the CLI crate.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directories excluded from generation trees.
///
/// These are runtime, user, or virtual filesystem directories that should
/// never be captured into an immutable generation image.
pub const EXCLUDED_DIRS: &[&str] = &[
    "var", "tmp", "run", "home", "root", "srv", "opt", "proc", "sys", "dev", "mnt", "media",
];

/// Standard root-level symlinks (source -> target).
///
/// These are the usr-merge symlinks that every generation should contain
/// so that `/bin`, `/lib`, `/lib64`, and `/sbin` resolve into `/usr/`.
pub const ROOT_SYMLINKS: &[(&str, &str)] = &[
    ("bin", "usr/bin"),
    ("lib", "usr/lib"),
    ("lib64", "usr/lib64"),
    ("sbin", "usr/sbin"),
];

/// Metadata for a single generation snapshot.
///
/// Serialized to `.conary-gen.json` inside each generation directory.
/// Fields added over time use `serde(default)` / `skip_serializing_if`
/// so older metadata files deserialize without errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetadata {
    pub generation: i64,
    /// "composefs" or "reflink" (for backwards compat with older generations)
    #[serde(default)]
    pub format: String,
    /// Size of the EROFS image in bytes (composefs format only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erofs_size: Option<i64>,
    /// Number of CAS objects referenced by the EROFS image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_objects_referenced: Option<i64>,
    /// Whether fs-verity is enabled on CAS objects
    #[serde(default)]
    pub fsverity_enabled: bool,
    /// Hex-encoded fs-verity digest of the EROFS image itself
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erofs_verity_digest: Option<String>,
    pub created_at: String,
    pub package_count: i64,
    pub kernel_version: Option<String>,
    pub summary: String,
}

impl GenerationMetadata {
    /// Write metadata to `.conary-gen.json` inside the given generation directory.
    pub fn write_to(&self, gen_dir: &Path) -> Result<()> {
        let path = gen_dir.join(".conary-gen.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Read metadata from `.conary-gen.json` inside the given generation directory.
    pub fn read_from(gen_dir: &Path) -> Result<Self> {
        let path = gen_dir.join(".conary-gen.json");
        let json = std::fs::read_to_string(path)?;
        let metadata: Self = serde_json::from_str(&json)?;
        Ok(metadata)
    }
}

/// Returns the base directory for all generations: `/conary/generations`
#[must_use]
pub fn generations_dir() -> PathBuf {
    PathBuf::from("/conary/generations")
}

/// Returns the directory for a specific generation: `/conary/generations/{number}`
#[must_use]
pub fn generation_path(number: i64) -> PathBuf {
    PathBuf::from(format!("/conary/generations/{number}"))
}

/// Returns the symlink pointing to the current active generation: `/conary/current`
#[must_use]
pub fn current_link() -> PathBuf {
    PathBuf::from("/conary/current")
}

/// Returns the directory for GC roots: `/conary/gc-roots`
#[must_use]
pub fn gc_roots_dir() -> PathBuf {
    PathBuf::from("/conary/gc-roots")
}

/// Detect kernel version(s) by scanning `gen_dir/usr/lib/modules/` for subdirectories.
///
/// Used when a generation has a deployed file tree (reflink format).
/// For composefs generations, use `detect_kernel_version_from_db` in the builder instead.
#[allow(dead_code)] // Retained for future reflink-format generation support
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

/// Check if a path should be excluded from generation trees.
///
/// Strips a leading `/` before comparing against `EXCLUDED_DIRS`.
#[must_use]
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
            format: "composefs".to_string(),
            erofs_size: Some(1_048_576),
            cas_objects_referenced: Some(320),
            fsverity_enabled: true,
            erofs_verity_digest: Some("abc123def456".to_string()),
            created_at: "2026-03-04T12:00:00Z".to_string(),
            package_count: 150,
            kernel_version: Some("6.12.1-arch1-1".to_string()),
            summary: "installed vim".to_string(),
        };

        metadata.write_to(tmp.path()).unwrap();
        let loaded = GenerationMetadata::read_from(tmp.path()).unwrap();

        assert_eq!(loaded.generation, 42);
        assert_eq!(loaded.format, "composefs");
        assert_eq!(loaded.erofs_size, Some(1_048_576));
        assert_eq!(loaded.cas_objects_referenced, Some(320));
        assert!(loaded.fsverity_enabled);
        assert_eq!(loaded.erofs_verity_digest.as_deref(), Some("abc123def456"));
        assert_eq!(loaded.created_at, "2026-03-04T12:00:00Z");
        assert_eq!(loaded.package_count, 150);
        assert_eq!(loaded.kernel_version.as_deref(), Some("6.12.1-arch1-1"));
        assert_eq!(loaded.summary, "installed vim");
    }

    #[test]
    fn test_metadata_roundtrip_no_verity_digest() {
        let tmp = TempDir::new().unwrap();
        let metadata = GenerationMetadata {
            generation: 7,
            format: "composefs".to_string(),
            erofs_size: Some(512_000),
            cas_objects_referenced: Some(100),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            created_at: "2026-03-17T10:00:00Z".to_string(),
            package_count: 80,
            kernel_version: None,
            summary: "baseline".to_string(),
        };

        metadata.write_to(tmp.path()).unwrap();

        // Verify the JSON does not contain erofs_verity_digest when None
        let json = std::fs::read_to_string(tmp.path().join(".conary-gen.json")).unwrap();
        assert!(
            !json.contains("erofs_verity_digest"),
            "erofs_verity_digest=None should be skipped in serialization"
        );

        let loaded = GenerationMetadata::read_from(tmp.path()).unwrap();
        assert_eq!(loaded.erofs_verity_digest, None);
    }

    #[test]
    fn test_metadata_backwards_compat() {
        // Old-format metadata without composefs fields should deserialize fine
        let tmp = TempDir::new().unwrap();
        let old_json = r#"{
            "generation": 10,
            "created_at": "2026-01-01T00:00:00Z",
            "package_count": 50,
            "kernel_version": null,
            "summary": "old generation"
        }"#;
        std::fs::write(tmp.path().join(".conary-gen.json"), old_json).unwrap();

        let loaded = GenerationMetadata::read_from(tmp.path()).unwrap();
        assert_eq!(loaded.generation, 10);
        assert_eq!(loaded.format, ""); // serde(default) gives empty string
        assert_eq!(loaded.erofs_size, None);
        assert_eq!(loaded.cas_objects_referenced, None);
        assert!(!loaded.fsverity_enabled); // serde(default) gives false
        assert_eq!(loaded.erofs_verity_digest, None);
        assert_eq!(loaded.summary, "old generation");
    }

    #[test]
    fn test_excluded_paths() {
        // These should be excluded (updated EXCLUDED_DIRS includes full "var")
        assert!(is_excluded("home"));
        assert!(is_excluded("/home"));
        assert!(is_excluded("home/peter"));
        assert!(is_excluded("proc"));
        assert!(is_excluded("/proc/cpuinfo"));
        assert!(is_excluded("var"));
        assert!(is_excluded("var/lib"));
        assert!(is_excluded("/var/lib/dpkg"));
        assert!(is_excluded("var/cache"));
        assert!(is_excluded("/var/cache/apt"));
        assert!(is_excluded("sys"));
        assert!(is_excluded("dev"));
        assert!(is_excluded("root"));
        assert!(is_excluded("root/.bashrc"));
        assert!(is_excluded("srv"));
        assert!(is_excluded("srv/http"));
        assert!(is_excluded("opt"));
        assert!(is_excluded("opt/cuda"));
        assert!(is_excluded("tmp"));
        assert!(is_excluded("run"));
        assert!(is_excluded("mnt"));
        assert!(is_excluded("media"));

        // These should NOT be excluded
        assert!(!is_excluded("usr"));
        assert!(!is_excluded("etc"));
        assert!(!is_excluded("/usr/bin"));
        assert!(!is_excluded("boot"));
    }

    #[test]
    fn test_generation_paths() {
        assert_eq!(generations_dir(), PathBuf::from("/conary/generations"));
        assert_eq!(generation_path(1), PathBuf::from("/conary/generations/1"));
        assert_eq!(generation_path(42), PathBuf::from("/conary/generations/42"));
        assert_eq!(current_link(), PathBuf::from("/conary/current"));
        assert_eq!(gc_roots_dir(), PathBuf::from("/conary/gc-roots"));
    }
}
