// conary-core/src/generation/metadata.rs

//! Generation metadata types and path helpers.
//!
//! These live in conary-core so the transaction engine can create and
//! inspect generation metadata without depending on the CLI crate.

use crate::error::Result;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Name of the EROFS image file within a generation directory.
pub const EROFS_IMAGE_NAME: &str = "root.erofs";

/// Format identifier for composefs-based generations.
pub const GENERATION_FORMAT: &str = "composefs";

/// Name of the metadata JSON file within a generation directory.
pub const GENERATION_METADATA_FILE: &str = ".conary-gen.json";
/// Name of the detached signature for generation metadata.
pub const GENERATION_METADATA_SIGNATURE_FILE: &str = ".conary-gen.sig";

/// Name of the marker file used while a generation is still being built.
const GENERATION_PENDING_MARKER: &str = ".conary-gen.pending";
const GENERATION_METADATA_SIGNING_KEY_FILE: &str = "generation-metadata.private";
const GENERATION_METADATA_PUBLIC_KEY_FILE: &str = "generation-metadata.public";

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
    /// Whether this generation is ready for fs-verity-enforced composefs mounts.
    #[serde(default)]
    pub fsverity_enabled: bool,
    /// Hex-encoded fs-verity digest of the EROFS image itself
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erofs_verity_digest: Option<String>,
    /// SHA-256 of the exact on-disk `.conary-artifact.json` bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_manifest_sha256: Option<String>,
    pub created_at: String,
    pub package_count: i64,
    pub kernel_version: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GenerationMetadataSignature {
    algorithm: String,
    signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
}

impl GenerationMetadata {
    /// Write metadata to the generation metadata file inside the given generation directory.
    ///
    /// Uses a crash-safe temp-file + fsync + rename sequence so that a
    /// power loss cannot leave a truncated metadata file next to a valid
    /// `root.erofs`.
    pub fn write_to(&self, gen_dir: &Path) -> Result<()> {
        let (signing_key_path, _) = generation_metadata_key_paths();
        let signing_key = signing_key_path
            .exists()
            .then_some(signing_key_path.as_path());
        self.write_to_with_key_paths(gen_dir, signing_key)
    }

    fn write_to_with_key_paths(
        &self,
        gen_dir: &Path,
        signing_key_path: Option<&Path>,
    ) -> Result<()> {
        use std::io::Write;

        let path = gen_dir.join(GENERATION_METADATA_FILE);
        let tmp_path = gen_dir.join(".metadata.json.tmp");
        let json = serde_json::to_string_pretty(self)?;

        // Write to temp file, fsync, then atomically rename.
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp_path, &path)?;

        let signature_path = gen_dir.join(GENERATION_METADATA_SIGNATURE_FILE);
        match signing_key_path {
            Some(key_path) => write_generation_metadata_signature(self, key_path, &signature_path)?,
            None => {
                if signature_path.exists() {
                    std::fs::remove_file(&signature_path)?;
                }
            }
        }

        // fsync the parent directory to persist the rename.
        if let Ok(dir) = std::fs::File::open(gen_dir) {
            let _ = dir.sync_all();
        }

        Ok(())
    }

    /// Read metadata from the generation metadata file inside the given generation directory.
    pub fn read_from(gen_dir: &Path) -> Result<Self> {
        let (signing_key_path, public_key_path) = generation_metadata_key_paths();
        let signing_key = signing_key_path
            .exists()
            .then_some(signing_key_path.as_path());
        let public_key = public_key_path
            .exists()
            .then_some(public_key_path.as_path());
        Self::read_from_with_key_paths(gen_dir, public_key, signing_key)
    }

    fn read_from_with_key_paths(
        gen_dir: &Path,
        public_key_path: Option<&Path>,
        signing_key_path: Option<&Path>,
    ) -> Result<Self> {
        if is_generation_pending(gen_dir) {
            return Err(crate::error::Error::NotFound(format!(
                "generation at {} is still pending and has not committed metadata yet",
                gen_dir.display()
            )));
        }

        let path = gen_dir.join(GENERATION_METADATA_FILE);
        let json = std::fs::read_to_string(path)?;
        let metadata: Self = serde_json::from_str(&json)?;
        verify_generation_metadata_signature(
            &metadata,
            gen_dir,
            public_key_path,
            signing_key_path,
        )?;
        Ok(metadata)
    }
}

fn generation_metadata_key_paths() -> (PathBuf, PathBuf) {
    let keyring_dir = crate::db::paths::keyring_dir("/var/lib/conary/conary.db");
    (
        keyring_dir.join(GENERATION_METADATA_SIGNING_KEY_FILE),
        keyring_dir.join(GENERATION_METADATA_PUBLIC_KEY_FILE),
    )
}

fn canonical_metadata_bytes(metadata: &GenerationMetadata) -> Result<Vec<u8>> {
    crate::json::canonical_json(metadata).map_err(|e| {
        crate::error::Error::ParseError(format!("Failed to canonicalize generation metadata: {e}"))
    })
}

fn load_generation_verifying_key(
    public_key_path: Option<&Path>,
    signing_key_path: Option<&Path>,
) -> Result<Option<VerifyingKey>> {
    if let Some(path) = public_key_path {
        let public_key_b64 = crate::ccs::signing::load_public_key(path)?;
        let public_key_bytes = BASE64.decode(public_key_b64).map_err(|e| {
            crate::error::Error::ParseError(format!(
                "Invalid base64 in generation metadata public key {}: {e}",
                path.display()
            ))
        })?;
        let public_key: [u8; 32] = public_key_bytes.try_into().map_err(|_| {
            crate::error::Error::ParseError(format!(
                "Invalid generation metadata public key length in {}",
                path.display()
            ))
        })?;
        return VerifyingKey::from_bytes(&public_key)
            .map(Some)
            .map_err(|e| {
                crate::error::Error::ParseError(format!(
                    "Invalid generation metadata public key {}: {e}",
                    path.display()
                ))
            });
    }

    if let Some(path) = signing_key_path {
        let keypair = crate::ccs::signing::SigningKeyPair::load_from_file(path)?;
        return Ok(Some(keypair.verifying_key()));
    }

    Ok(None)
}

fn write_generation_metadata_signature(
    metadata: &GenerationMetadata,
    signing_key_path: &Path,
    signature_path: &Path,
) -> Result<()> {
    use std::io::Write;

    let keypair = crate::ccs::signing::SigningKeyPair::load_from_file(signing_key_path)?;
    let canonical = canonical_metadata_bytes(metadata)?;
    let signature = keypair.signing_key().sign(&canonical);
    let signature_doc = GenerationMetadataSignature {
        algorithm: "ed25519".to_string(),
        signature: BASE64.encode(signature.to_bytes()),
        key_id: keypair.key_id().map(ToOwned::to_owned),
    };

    let tmp_path = signature_path.with_extension("sig.tmp");
    let json = serde_json::to_string_pretty(&signature_doc)?;
    let mut file = std::fs::File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp_path, signature_path)?;
    Ok(())
}

fn verify_generation_metadata_signature(
    metadata: &GenerationMetadata,
    gen_dir: &Path,
    public_key_path: Option<&Path>,
    signing_key_path: Option<&Path>,
) -> Result<()> {
    let signature_path = gen_dir.join(GENERATION_METADATA_SIGNATURE_FILE);
    let verifying_key = load_generation_verifying_key(public_key_path, signing_key_path)?;

    match (signature_path.exists(), verifying_key) {
        (false, None) => Ok(()),
        (false, Some(_)) => Err(crate::error::Error::TrustError(format!(
            "Generation metadata at {} is unsigned despite a configured verification key",
            gen_dir.display()
        ))),
        (true, None) => Err(crate::error::Error::TrustError(format!(
            "Generation metadata at {} has a signature but no verification key is configured",
            gen_dir.display()
        ))),
        (true, Some(verifying_key)) => {
            let sig_json = std::fs::read_to_string(&signature_path)?;
            let signature_doc: GenerationMetadataSignature = serde_json::from_str(&sig_json)?;
            if signature_doc.algorithm != "ed25519" {
                return Err(crate::error::Error::TrustError(format!(
                    "Unsupported generation metadata signature algorithm '{}'",
                    signature_doc.algorithm
                )));
            }

            let sig_bytes = BASE64.decode(signature_doc.signature).map_err(|e| {
                crate::error::Error::ParseError(format!(
                    "Invalid base64 in generation metadata signature {}: {e}",
                    signature_path.display()
                ))
            })?;
            let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
                crate::error::Error::ParseError(format!(
                    "Invalid generation metadata signature {}: {e}",
                    signature_path.display()
                ))
            })?;
            let canonical = canonical_metadata_bytes(metadata)?;
            verifying_key
                .verify_strict(&canonical, &signature)
                .map_err(|e| {
                    crate::error::Error::TrustError(format!(
                        "Generation metadata signature verification failed for {}: {e}",
                        gen_dir.display()
                    ))
                })
        }
    }
}

/// Returns the pending marker path inside a generation directory.
#[must_use]
pub fn generation_pending_marker_path(gen_dir: &Path) -> PathBuf {
    gen_dir.join(GENERATION_PENDING_MARKER)
}

/// Return true when the generation directory is marked as incomplete.
#[must_use]
pub fn is_generation_pending(gen_dir: &Path) -> bool {
    generation_pending_marker_path(gen_dir).exists()
}

/// Mark a generation directory as pending using a durable temp-file + rename sequence.
pub fn mark_generation_pending(gen_dir: &Path) -> Result<()> {
    use std::io::Write;

    let path = generation_pending_marker_path(gen_dir);
    let tmp_path = gen_dir.join(".pending.tmp");
    let mut file = std::fs::File::create(&tmp_path)?;
    file.write_all(b"pending\n")?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&tmp_path, &path)?;
    sync_generation_dir(gen_dir);
    Ok(())
}

/// Remove the pending marker after a generation fully commits.
pub fn clear_generation_pending(gen_dir: &Path) -> Result<()> {
    let path = generation_pending_marker_path(gen_dir);
    if path.exists() {
        std::fs::remove_file(&path)?;
        sync_generation_dir(gen_dir);
    }
    Ok(())
}

fn sync_generation_dir(gen_dir: &Path) {
    if let Ok(dir) = std::fs::File::open(gen_dir) {
        let _ = dir.sync_all();
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

/// Read the running kernel version from `/proc/sys/kernel/osrelease`.
///
/// Falls back to `None` if the file cannot be read (e.g. in tests or containers
/// without a mounted procfs).
pub(crate) fn running_kernel_version() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Detect kernel version(s) by scanning `gen_dir/usr/lib/modules/` for subdirectories.
///
/// Used when a generation has a deployed file tree (reflink format) and
/// by the bootstrap image builder to discover the kernel in the sysroot.
/// For composefs generations, use `detect_kernel_version_from_troves` in the builder instead.
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
    EXCLUDED_DIRS.iter().any(|dir| {
        path == *dir || (path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::signing::SigningKeyPair;
    use tempfile::TempDir;

    #[test]
    fn test_metadata_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let metadata = GenerationMetadata {
            generation: 42,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(1_048_576),
            cas_objects_referenced: Some(320),
            fsverity_enabled: true,
            erofs_verity_digest: Some("abc123def456".to_string()),
            artifact_manifest_sha256: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            ),
            created_at: "2026-03-04T12:00:00Z".to_string(),
            package_count: 150,
            kernel_version: Some("6.12.1-arch1-1".to_string()),
            summary: "installed vim".to_string(),
        };

        metadata.write_to_with_key_paths(tmp.path(), None).unwrap();
        let loaded = GenerationMetadata::read_from_with_key_paths(tmp.path(), None, None).unwrap();

        assert_eq!(loaded.generation, 42);
        assert_eq!(loaded.format, GENERATION_FORMAT);
        assert_eq!(loaded.erofs_size, Some(1_048_576));
        assert_eq!(loaded.cas_objects_referenced, Some(320));
        assert!(loaded.fsverity_enabled);
        assert_eq!(loaded.erofs_verity_digest.as_deref(), Some("abc123def456"));
        assert_eq!(
            loaded.artifact_manifest_sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
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
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(512_000),
            cas_objects_referenced: Some(100),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            artifact_manifest_sha256: None,
            created_at: "2026-03-17T10:00:00Z".to_string(),
            package_count: 80,
            kernel_version: None,
            summary: "baseline".to_string(),
        };

        metadata.write_to_with_key_paths(tmp.path(), None).unwrap();

        // Verify the JSON does not contain erofs_verity_digest when None
        let json = std::fs::read_to_string(tmp.path().join(GENERATION_METADATA_FILE)).unwrap();
        assert!(
            !json.contains("erofs_verity_digest"),
            "erofs_verity_digest=None should be skipped in serialization"
        );

        let loaded = GenerationMetadata::read_from_with_key_paths(tmp.path(), None, None).unwrap();
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
        std::fs::write(tmp.path().join(GENERATION_METADATA_FILE), old_json).unwrap();

        let loaded = GenerationMetadata::read_from_with_key_paths(tmp.path(), None, None).unwrap();
        assert_eq!(loaded.generation, 10);
        assert_eq!(loaded.format, ""); // serde(default) gives empty string
        assert_eq!(loaded.erofs_size, None);
        assert_eq!(loaded.cas_objects_referenced, None);
        assert!(!loaded.fsverity_enabled); // serde(default) gives false
        assert_eq!(loaded.erofs_verity_digest, None);
        assert_eq!(loaded.artifact_manifest_sha256, None);
        assert_eq!(loaded.summary, "old generation");
    }

    #[test]
    fn test_pending_marker_roundtrip() {
        let tmp = TempDir::new().unwrap();

        assert!(!is_generation_pending(tmp.path()));

        mark_generation_pending(tmp.path()).unwrap();
        assert!(is_generation_pending(tmp.path()));
        assert!(generation_pending_marker_path(tmp.path()).exists());

        clear_generation_pending(tmp.path()).unwrap();
        assert!(!is_generation_pending(tmp.path()));
    }

    #[test]
    fn test_read_from_rejects_pending_generation() {
        let tmp = TempDir::new().unwrap();
        mark_generation_pending(tmp.path()).unwrap();

        let err = GenerationMetadata::read_from(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("still pending"));
    }

    fn generate_test_metadata_keys(dir: &TempDir) -> (PathBuf, PathBuf) {
        let private_path = dir.path().join("generation-metadata.private");
        let public_path = dir.path().join("generation-metadata.public");
        let keypair = SigningKeyPair::generate().with_key_id("test-generation-key");
        keypair.save_to_files(&private_path, &public_path).unwrap();
        (private_path, public_path)
    }

    #[test]
    fn test_metadata_signature_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let key_dir = TempDir::new().unwrap();
        let (private_key, public_key) = generate_test_metadata_keys(&key_dir);
        let metadata = GenerationMetadata {
            generation: 12,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(2048),
            cas_objects_referenced: Some(9),
            fsverity_enabled: true,
            erofs_verity_digest: Some("abcd".to_string()),
            artifact_manifest_sha256: None,
            created_at: "2026-03-27T12:00:00Z".to_string(),
            package_count: 3,
            kernel_version: Some("6.13.0".to_string()),
            summary: "signed".to_string(),
        };

        metadata
            .write_to_with_key_paths(tmp.path(), Some(&private_key))
            .unwrap();
        let loaded =
            GenerationMetadata::read_from_with_key_paths(tmp.path(), Some(&public_key), None)
                .unwrap();

        assert_eq!(loaded.summary, "signed");
        assert!(tmp.path().join(GENERATION_METADATA_SIGNATURE_FILE).exists());
    }

    #[test]
    fn test_metadata_signature_rejects_tampering() {
        let tmp = TempDir::new().unwrap();
        let key_dir = TempDir::new().unwrap();
        let (private_key, public_key) = generate_test_metadata_keys(&key_dir);
        let metadata = GenerationMetadata {
            generation: 5,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(10),
            cas_objects_referenced: Some(2),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            artifact_manifest_sha256: None,
            created_at: "2026-03-27T12:00:00Z".to_string(),
            package_count: 1,
            kernel_version: None,
            summary: "original".to_string(),
        };

        metadata
            .write_to_with_key_paths(tmp.path(), Some(&private_key))
            .unwrap();

        let tampered = GenerationMetadata {
            summary: "tampered".to_string(),
            ..metadata.clone()
        };
        std::fs::write(
            tmp.path().join(GENERATION_METADATA_FILE),
            serde_json::to_string_pretty(&tampered).unwrap(),
        )
        .unwrap();

        let err = GenerationMetadata::read_from_with_key_paths(tmp.path(), Some(&public_key), None)
            .unwrap_err();
        assert!(err.to_string().contains("signature verification failed"));
    }

    #[test]
    fn test_metadata_requires_signature_when_verification_key_present() {
        let tmp = TempDir::new().unwrap();
        let key_dir = TempDir::new().unwrap();
        let (_private_key, public_key) = generate_test_metadata_keys(&key_dir);
        let metadata = GenerationMetadata {
            generation: 8,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(128),
            cas_objects_referenced: Some(4),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            artifact_manifest_sha256: None,
            created_at: "2026-03-27T12:00:00Z".to_string(),
            package_count: 2,
            kernel_version: None,
            summary: "unsigned".to_string(),
        };

        metadata.write_to_with_key_paths(tmp.path(), None).unwrap();

        let err = GenerationMetadata::read_from_with_key_paths(tmp.path(), Some(&public_key), None)
            .unwrap_err();
        assert!(err.to_string().contains("unsigned"));
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
