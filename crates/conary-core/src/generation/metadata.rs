// crates/conary-core/src/generation/metadata.rs

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationMetadata {
    pub generation: i64,
    /// Current generation storage format. Only `composefs` is valid.
    pub format: String,
    /// Size of the EROFS image in bytes (composefs format only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erofs_size: Option<i64>,
    /// Number of CAS objects referenced by the EROFS image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_objects_referenced: Option<i64>,
    /// Whether this generation is ready for fs-verity-enforced composefs mounts.
    pub fsverity_enabled: bool,
    /// Hex-encoded fs-verity digest of the EROFS image itself
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erofs_verity_digest: Option<String>,
    /// SHA-256 of the exact on-disk `.conary-artifact.json` bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_manifest_sha256: Option<String>,
    /// Number of regular files in the image carrying `security.capability`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_capability_xattr_count: Option<i64>,
    pub created_at: String,
    pub package_count: i64,
    pub kernel_version: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        self.validate_current_contract()?;
        let path = gen_dir.join(GENERATION_METADATA_FILE);
        crate::filesystem::durable::write_json_atomic(&path, self)?;

        let signature_path = gen_dir.join(GENERATION_METADATA_SIGNATURE_FILE);
        match signing_key_path {
            Some(key_path) => write_generation_metadata_signature(self, key_path, &signature_path)?,
            None => remove_generation_metadata_signature(&signature_path)?,
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
        metadata.validate_current_contract()?;
        verify_generation_metadata_signature(
            &metadata,
            gen_dir,
            public_key_path,
            signing_key_path,
        )?;
        Ok(metadata)
    }

    fn validate_current_contract(&self) -> Result<()> {
        if self.format != GENERATION_FORMAT {
            return Err(crate::error::Error::ParseError(format!(
                "generation {} uses unsupported format {:?}; current metadata requires {:?}",
                self.generation, self.format, GENERATION_FORMAT
            )));
        }
        Ok(())
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
    let keypair = crate::ccs::signing::SigningKeyPair::load_from_file(signing_key_path)?;
    let canonical = canonical_metadata_bytes(metadata)?;
    let signature = keypair.signing_key().sign(&canonical);
    let signature_doc = GenerationMetadataSignature {
        algorithm: "ed25519".to_string(),
        signature: BASE64.encode(signature.to_bytes()),
        key_id: keypair.key_id().map(ToOwned::to_owned),
    };

    crate::filesystem::durable::write_json_atomic(signature_path, &signature_doc)
}

fn remove_generation_metadata_signature(signature_path: &Path) -> Result<()> {
    match crate::filesystem::durable::remove_file_and_sync_parent(signature_path) {
        Ok(()) => Ok(()),
        Err(crate::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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
    let path = generation_pending_marker_path(gen_dir);
    crate::filesystem::durable::write_file_atomic(&path, b"pending\n")
}

/// Remove the pending marker after a generation fully commits.
pub fn clear_generation_pending(gen_dir: &Path) -> Result<()> {
    let path = generation_pending_marker_path(gen_dir);
    match crate::filesystem::durable::remove_file_and_sync_parent(&path) {
        Ok(()) => Ok(()),
        Err(crate::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Returns the base directory for all generations: `/conary/generations`
#[must_use]
pub fn generations_dir() -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().generations_dir()
}

/// Returns the directory for a specific generation: `/conary/generations/{number}`
#[must_use]
pub fn generation_path(number: i64) -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().generation_path(number)
}

/// Returns the symlink pointing to the current active generation: `/conary/current`
#[must_use]
pub fn current_link() -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().current_link()
}

/// Returns the directory for GC roots: `/conary/gc-roots`
#[must_use]
pub fn gc_roots_dir() -> PathBuf {
    crate::runtime_root::ConaryRuntimeRoot::default().gc_roots_dir()
}

/// Resolve one exact kernel release from `gen_dir/usr/lib/modules/`.
///
/// Used by generation boot-asset staging and image export to discover the
/// kernel in a deployed sysroot. Multiple releases require an explicit
/// selection instead of filesystem iteration order deciding which kernel boots.
pub fn detect_kernel_version(gen_dir: &Path) -> crate::Result<Option<String>> {
    let modules_dir = gen_dir.join("usr/lib/modules");
    let entries = match std::fs::read_dir(&modules_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut releases = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let release = entry.file_name().into_string().map_err(|_| {
                crate::Error::InvalidPath(format!(
                    "kernel module directory under {} is not UTF-8",
                    modules_dir.display()
                ))
            })?;
            if release.is_empty() || release.contains(['/', '\\', '\0']) {
                return Err(crate::Error::InvalidPath(format!(
                    "kernel module release {release:?} is invalid"
                )));
            }
            releases.push(release);
        }
    }
    releases.sort();
    releases.dedup();
    match releases.as_slice() {
        [] => Ok(None),
        [release] => Ok(Some(release.clone())),
        _ => Err(crate::Error::InvalidPath(format!(
            "sysroot {} contains multiple kernel module releases {:?}; select one exact kernel before building the image",
            gen_dir.display(),
            releases
        ))),
    }
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
mod tests;
