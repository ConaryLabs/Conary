// crates/conary-core/src/repository/catalog/portable_integrity.rs

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::contract::{CatalogArtifactV1, validate_sha256};

pub const PORTABLE_CHUNK_MANIFEST_SCHEMA_V1: u16 = 1;
pub const PORTABLE_CHUNK_SIZE_V1: u32 = 64 * 1024;

const PORTABLE_CHUNK_MANIFEST_MAGIC_V1: [u8; 8] = *b"CNRYPCM1";
const PORTABLE_CHUNK_HASH_ALGORITHM_V1: u16 = 1;
const PORTABLE_CHUNK_MANIFEST_HEADER_SIZE_V1: u64 = 64;
const SHA256_SIZE: u64 = 32;
const CHUNK_HASH_DOMAIN_V1: &[u8] = b"ConaryPortableCatalogChunkV1\0";
static TEMP_MANIFEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub type PortableIntegrityResult<T> = std::result::Result<T, PortableIntegrityError>;

#[derive(Debug, Error)]
pub enum PortableIntegrityError {
    #[error("portable catalog integrity I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid portable catalog integrity path: {0}")]
    InvalidPath(String),

    #[error("portable catalog integrity path is not a regular file: {0}")]
    NotRegularFile(PathBuf),

    #[error("portable catalog integrity path changed while opening: {0}")]
    PathChanged(PathBuf),

    #[error("portable catalog manifest already exists: {0}")]
    AlreadyExists(PathBuf),

    #[error("{field} must be exactly 64 lowercase hexadecimal characters")]
    InvalidSha256 { field: &'static str },

    #[error("portable catalog manifest is too short for its 64-byte header: {actual} bytes")]
    HeaderTruncated { actual: u64 },

    #[error("portable catalog manifest has an invalid magic value")]
    InvalidMagic,

    #[error("portable catalog manifest schema {actual} is unsupported; expected {expected}")]
    UnsupportedSchema { expected: u16, actual: u16 },

    #[error("portable catalog chunk hash algorithm {actual} is unsupported; expected {expected}")]
    UnsupportedHashAlgorithm { expected: u16, actual: u16 },

    #[error("portable catalog chunk size is {actual}; expected {expected}")]
    UnexpectedChunkSize { expected: u32, actual: u32 },

    #[error("portable catalog size is {actual}; expected {expected}")]
    CatalogSizeMismatch { expected: u64, actual: u64 },

    #[error("portable catalog artifact SHA-256 is {actual}; expected {expected}")]
    ArtifactDigestMismatch { expected: String, actual: String },

    #[error("portable catalog chunk count is {actual}; expected {expected}")]
    ChunkCountMismatch { expected: u64, actual: u64 },

    #[error("portable catalog manifest length arithmetic overflowed")]
    ManifestLengthOverflow,

    #[error("portable catalog manifest length is {actual}; expected {expected}")]
    ManifestLengthMismatch { expected: u64, actual: u64 },

    #[error("portable catalog manifest allocation for {size} bytes is unavailable")]
    ManifestAllocationUnavailable { size: u64 },

    #[error("portable catalog manifest attestation size is {actual}; expected {expected}")]
    ManifestAttestationSizeMismatch { expected: u64, actual: u64 },

    #[error("portable catalog manifest SHA-256 is {actual}; expected {expected}")]
    ManifestDigestMismatch { expected: String, actual: String },

    #[error("portable catalog chunk index {index} is outside count {chunk_count}")]
    ChunkIndexOutOfRange { index: u64, chunk_count: u64 },

    #[error("portable catalog chunk {index} length is {actual}; expected {expected}")]
    ChunkLengthMismatch {
        index: u64,
        expected: u32,
        actual: u64,
    },

    #[error("portable catalog chunk {index} SHA-256 is {actual}; expected {expected}")]
    ChunkDigestMismatch {
        index: u64,
        expected: String,
        actual: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableManifestAttestationV1 {
    pub sha256: String,
    pub size: u64,
}

impl PortableManifestAttestationV1 {
    pub fn validate(&self) -> PortableIntegrityResult<()> {
        validate_canonical_sha256(&self.sha256, "portable catalog manifest SHA-256").map(|_| ())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortableChunkRangeV1 {
    pub offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableChunkManifestV1 {
    catalog_size: u64,
    artifact_sha256: [u8; 32],
    chunk_sha256: Vec<[u8; 32]>,
}

impl PortableChunkManifestV1 {
    /// Build chunk authority from the exact open catalog descriptor.
    ///
    /// The ordinary whole-artifact SHA-256 is computed in the same pass as the
    /// chunk digests and must equal `expected_artifact`. This prevents a caller
    /// from registering chunk authority for bytes that changed after an earlier
    /// catalog verification.
    pub fn build(
        catalog: &File,
        expected_artifact: &CatalogArtifactV1,
    ) -> PortableIntegrityResult<Self> {
        let expected_digest =
            validate_canonical_sha256(&expected_artifact.sha256, "catalog artifact SHA-256")?;
        let metadata = catalog.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(PortableIntegrityError::NotRegularFile(PathBuf::from(
                "<catalog-fd>",
            )));
        }
        if metadata.len() != expected_artifact.size {
            return Err(PortableIntegrityError::CatalogSizeMismatch {
                expected: expected_artifact.size,
                actual: metadata.len(),
            });
        }

        let chunk_count = portable_chunk_count_v1(expected_artifact.size)?;
        let chunk_capacity = usize::try_from(chunk_count)
            .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
        let mut chunk_sha256 = Vec::new();
        chunk_sha256
            .try_reserve_exact(chunk_capacity)
            .map_err(|_| PortableIntegrityError::ManifestAllocationUnavailable {
                size: portable_manifest_size_v1(chunk_count).unwrap_or(u64::MAX),
            })?;
        let mut buffer = vec![0_u8; PORTABLE_CHUNK_SIZE_V1 as usize];
        let mut artifact_hasher = Sha256::new();

        for index in 0..chunk_count {
            let range = chunk_range_for_size(expected_artifact.size, chunk_count, index)?;
            let bytes = &mut buffer[..range.length as usize];
            read_exact_at(catalog, bytes, range.offset)?;
            artifact_hasher.update(&*bytes);
            chunk_sha256.push(portable_chunk_digest_v1(index, bytes));
        }

        let final_metadata = catalog.metadata()?;
        if !final_metadata.file_type().is_file() || final_metadata.len() != expected_artifact.size {
            return Err(PortableIntegrityError::CatalogSizeMismatch {
                expected: expected_artifact.size,
                actual: final_metadata.len(),
            });
        }
        let actual_digest: [u8; 32] = artifact_hasher.finalize().into();
        if actual_digest != expected_digest {
            return Err(PortableIntegrityError::ArtifactDigestMismatch {
                expected: hex::encode(expected_digest),
                actual: hex::encode(actual_digest),
            });
        }

        Ok(Self {
            catalog_size: expected_artifact.size,
            artifact_sha256: expected_digest,
            chunk_sha256,
        })
    }

    /// Decode only after checking the exact manifest artifact attestation.
    pub fn decode_attested(
        bytes: &[u8],
        expected_manifest: &PortableManifestAttestationV1,
        expected_artifact: &CatalogArtifactV1,
    ) -> PortableIntegrityResult<Self> {
        expected_manifest.validate()?;
        let expected_chunk_count = portable_chunk_count_v1(expected_artifact.size)?;
        let expected_manifest_size = portable_manifest_size_v1(expected_chunk_count)?;
        if expected_manifest.size != expected_manifest_size {
            return Err(PortableIntegrityError::ManifestAttestationSizeMismatch {
                expected: expected_manifest_size,
                actual: expected_manifest.size,
            });
        }
        let actual_size = u64::try_from(bytes.len())
            .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
        if actual_size != expected_manifest.size {
            return Err(PortableIntegrityError::ManifestLengthMismatch {
                expected: expected_manifest.size,
                actual: actual_size,
            });
        }
        let actual_sha256 = crate::hash::sha256(bytes);
        if actual_sha256 != expected_manifest.sha256 {
            return Err(PortableIntegrityError::ManifestDigestMismatch {
                expected: expected_manifest.sha256.clone(),
                actual: actual_sha256,
            });
        }
        Self::decode(bytes, expected_artifact)
    }

    fn decode(
        bytes: &[u8],
        expected_artifact: &CatalogArtifactV1,
    ) -> PortableIntegrityResult<Self> {
        let actual_length = u64::try_from(bytes.len())
            .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
        if bytes.len() < PORTABLE_CHUNK_MANIFEST_HEADER_SIZE_V1 as usize {
            return Err(PortableIntegrityError::HeaderTruncated {
                actual: actual_length,
            });
        }
        if bytes[..8] != PORTABLE_CHUNK_MANIFEST_MAGIC_V1 {
            return Err(PortableIntegrityError::InvalidMagic);
        }
        let schema = u16::from_le_bytes(bytes[8..10].try_into().expect("header length checked"));
        if schema != PORTABLE_CHUNK_MANIFEST_SCHEMA_V1 {
            return Err(PortableIntegrityError::UnsupportedSchema {
                expected: PORTABLE_CHUNK_MANIFEST_SCHEMA_V1,
                actual: schema,
            });
        }
        let algorithm =
            u16::from_le_bytes(bytes[10..12].try_into().expect("header length checked"));
        if algorithm != PORTABLE_CHUNK_HASH_ALGORITHM_V1 {
            return Err(PortableIntegrityError::UnsupportedHashAlgorithm {
                expected: PORTABLE_CHUNK_HASH_ALGORITHM_V1,
                actual: algorithm,
            });
        }
        let chunk_size =
            u32::from_le_bytes(bytes[12..16].try_into().expect("header length checked"));
        if chunk_size != PORTABLE_CHUNK_SIZE_V1 {
            return Err(PortableIntegrityError::UnexpectedChunkSize {
                expected: PORTABLE_CHUNK_SIZE_V1,
                actual: chunk_size,
            });
        }
        let catalog_size =
            u64::from_le_bytes(bytes[16..24].try_into().expect("header length checked"));
        if catalog_size != expected_artifact.size {
            return Err(PortableIntegrityError::CatalogSizeMismatch {
                expected: expected_artifact.size,
                actual: catalog_size,
            });
        }
        let chunk_count =
            u64::from_le_bytes(bytes[24..32].try_into().expect("header length checked"));
        let expected_chunk_count = portable_chunk_count_v1(catalog_size)?;
        if chunk_count != expected_chunk_count {
            return Err(PortableIntegrityError::ChunkCountMismatch {
                expected: expected_chunk_count,
                actual: chunk_count,
            });
        }
        let expected_digest =
            validate_canonical_sha256(&expected_artifact.sha256, "catalog artifact SHA-256")?;
        let header_digest: [u8; 32] = bytes[32..64].try_into().expect("header length checked");
        if header_digest != expected_digest {
            return Err(PortableIntegrityError::ArtifactDigestMismatch {
                expected: hex::encode(expected_digest),
                actual: hex::encode(header_digest),
            });
        }

        let expected_length = portable_manifest_size_v1(chunk_count)?;
        if actual_length != expected_length {
            return Err(PortableIntegrityError::ManifestLengthMismatch {
                expected: expected_length,
                actual: actual_length,
            });
        }
        let chunk_capacity = usize::try_from(chunk_count)
            .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
        let mut chunk_sha256 = Vec::new();
        chunk_sha256
            .try_reserve_exact(chunk_capacity)
            .map_err(|_| PortableIntegrityError::ManifestAllocationUnavailable {
                size: expected_length,
            })?;
        let (digests, remainder) =
            bytes[PORTABLE_CHUNK_MANIFEST_HEADER_SIZE_V1 as usize..].as_chunks::<32>();
        debug_assert!(remainder.is_empty());
        chunk_sha256.extend_from_slice(digests);

        Ok(Self {
            catalog_size,
            artifact_sha256: expected_digest,
            chunk_sha256,
        })
    }

    pub fn encode(&self) -> PortableIntegrityResult<Vec<u8>> {
        let chunk_count = self.chunk_count();
        let expected_chunk_count = portable_chunk_count_v1(self.catalog_size)?;
        if chunk_count != expected_chunk_count {
            return Err(PortableIntegrityError::ChunkCountMismatch {
                expected: expected_chunk_count,
                actual: chunk_count,
            });
        }
        let encoded_size = portable_manifest_size_v1(chunk_count)?;
        let encoded_capacity = usize::try_from(encoded_size)
            .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(encoded_capacity).map_err(|_| {
            PortableIntegrityError::ManifestAllocationUnavailable { size: encoded_size }
        })?;
        bytes.extend_from_slice(&PORTABLE_CHUNK_MANIFEST_MAGIC_V1);
        bytes.extend_from_slice(&PORTABLE_CHUNK_MANIFEST_SCHEMA_V1.to_le_bytes());
        bytes.extend_from_slice(&PORTABLE_CHUNK_HASH_ALGORITHM_V1.to_le_bytes());
        bytes.extend_from_slice(&PORTABLE_CHUNK_SIZE_V1.to_le_bytes());
        bytes.extend_from_slice(&self.catalog_size.to_le_bytes());
        bytes.extend_from_slice(&chunk_count.to_le_bytes());
        bytes.extend_from_slice(&self.artifact_sha256);
        for digest in &self.chunk_sha256 {
            bytes.extend_from_slice(digest);
        }
        debug_assert_eq!(bytes.len(), encoded_capacity);
        Ok(bytes)
    }

    pub fn attestation(&self) -> PortableIntegrityResult<PortableManifestAttestationV1> {
        let bytes = self.encode()?;
        Ok(attestation_for_bytes(&bytes))
    }

    pub const fn catalog_size(&self) -> u64 {
        self.catalog_size
    }

    pub const fn chunk_size(&self) -> u32 {
        PORTABLE_CHUNK_SIZE_V1
    }

    pub fn chunk_count(&self) -> u64 {
        u64::try_from(self.chunk_sha256.len())
            .expect("a portable manifest vector length always fits u64")
    }

    pub fn artifact_sha256(&self) -> String {
        hex::encode(self.artifact_sha256)
    }

    pub fn chunk_range(&self, index: u64) -> PortableIntegrityResult<PortableChunkRangeV1> {
        chunk_range_for_size(self.catalog_size, self.chunk_count(), index)
    }

    /// Verify bytes already read by a caller, so a VFS can serve those same
    /// authenticated bytes without a check-then-reread race.
    pub fn verify_chunk_bytes(&self, index: u64, bytes: &[u8]) -> PortableIntegrityResult<()> {
        let range = self.chunk_range(index)?;
        let actual_length = u64::try_from(bytes.len())
            .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
        if actual_length != u64::from(range.length) {
            return Err(PortableIntegrityError::ChunkLengthMismatch {
                index,
                expected: range.length,
                actual: actual_length,
            });
        }
        let actual = portable_chunk_digest_v1(index, bytes);
        let expected = self.chunk_sha256[index as usize];
        if actual != expected {
            return Err(PortableIntegrityError::ChunkDigestMismatch {
                index,
                expected: hex::encode(expected),
                actual: hex::encode(actual),
            });
        }
        Ok(())
    }

    /// Read and return the exact bytes authenticated for one catalog chunk.
    pub fn read_verified_chunk(
        &self,
        catalog: &File,
        index: u64,
    ) -> PortableIntegrityResult<Vec<u8>> {
        let range = self.chunk_range(index)?;
        let mut bytes = vec![0_u8; range.length as usize];
        read_exact_at(catalog, &mut bytes, range.offset)?;
        self.verify_chunk_bytes(index, &bytes)?;
        Ok(bytes)
    }
}

/// Exact number of 64 KiB chunks needed for `catalog_size` bytes.
pub fn portable_chunk_count_v1(catalog_size: u64) -> PortableIntegrityResult<u64> {
    let chunk_size = u64::from(PORTABLE_CHUNK_SIZE_V1);
    let complete = catalog_size / chunk_size;
    complete
        .checked_add(u64::from(!catalog_size.is_multiple_of(chunk_size)))
        .ok_or(PortableIntegrityError::ManifestLengthOverflow)
}

/// Exact encoded manifest size for a declared chunk count.
pub fn portable_manifest_size_v1(chunk_count: u64) -> PortableIntegrityResult<u64> {
    chunk_count
        .checked_mul(SHA256_SIZE)
        .and_then(|body| body.checked_add(PORTABLE_CHUNK_MANIFEST_HEADER_SIZE_V1))
        .ok_or(PortableIntegrityError::ManifestLengthOverflow)
}

/// Atomically publish private, durable manifest bytes without replacing a path.
pub fn write_portable_chunk_manifest_v1(
    path: &Path,
    manifest: &PortableChunkManifestV1,
) -> PortableIntegrityResult<PortableManifestAttestationV1> {
    let bytes = manifest.encode()?;
    let attestation = attestation_for_bytes(&bytes);
    let parent = require_real_parent(path)?;
    let (temp_path, mut temp_file) = create_private_temp_manifest(&parent)?;
    let temp_guard = TemporaryManifest::new(temp_path.clone());
    temp_file.write_all(&bytes)?;
    temp_file.sync_all()?;

    match fs::hard_link(&temp_path, path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(PortableIntegrityError::AlreadyExists(path.to_path_buf()));
        }
        Err(error) => return Err(error.into()),
    }

    let target_metadata = fs::symlink_metadata(path)?;
    if !target_metadata.file_type().is_file() {
        return Err(PortableIntegrityError::NotRegularFile(path.to_path_buf()));
    }
    #[cfg(unix)]
    {
        let temp_metadata = temp_file.metadata()?;
        if temp_metadata.dev() != target_metadata.dev()
            || temp_metadata.ino() != target_metadata.ino()
        {
            return Err(PortableIntegrityError::PathChanged(path.to_path_buf()));
        }
    }
    fs::remove_file(&temp_path)?;
    temp_guard.disarm();
    File::open(&parent)?.sync_all()?;
    Ok(attestation)
}

/// Open without following symlinks, then decode exact attested manifest bytes.
pub fn read_portable_chunk_manifest_v1(
    path: &Path,
    expected_manifest: &PortableManifestAttestationV1,
    expected_artifact: &CatalogArtifactV1,
) -> PortableIntegrityResult<PortableChunkManifestV1> {
    expected_manifest.validate()?;
    let mut file = open_regular_nofollow(path)?;
    let metadata = file.metadata()?;
    if metadata.len() != expected_manifest.size {
        return Err(PortableIntegrityError::ManifestLengthMismatch {
            expected: expected_manifest.size,
            actual: metadata.len(),
        });
    }
    let capacity = usize::try_from(expected_manifest.size)
        .map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|_| {
        PortableIntegrityError::ManifestAllocationUnavailable {
            size: expected_manifest.size,
        }
    })?;
    let limit = expected_manifest
        .size
        .checked_add(1)
        .ok_or(PortableIntegrityError::ManifestLengthOverflow)?;
    Read::by_ref(&mut file)
        .take(limit)
        .read_to_end(&mut bytes)?;
    require_path_still_names_file(path, &file)?;
    PortableChunkManifestV1::decode_attested(&bytes, expected_manifest, expected_artifact)
}

fn chunk_range_for_size(
    catalog_size: u64,
    chunk_count: u64,
    index: u64,
) -> PortableIntegrityResult<PortableChunkRangeV1> {
    if index >= chunk_count {
        return Err(PortableIntegrityError::ChunkIndexOutOfRange { index, chunk_count });
    }
    let offset = index
        .checked_mul(u64::from(PORTABLE_CHUNK_SIZE_V1))
        .ok_or(PortableIntegrityError::ManifestLengthOverflow)?;
    let remaining = catalog_size
        .checked_sub(offset)
        .ok_or(PortableIntegrityError::ManifestLengthOverflow)?;
    let length = remaining.min(u64::from(PORTABLE_CHUNK_SIZE_V1));
    let length =
        u32::try_from(length).map_err(|_| PortableIntegrityError::ManifestLengthOverflow)?;
    Ok(PortableChunkRangeV1 { offset, length })
}

fn portable_chunk_digest_v1(index: u64, bytes: &[u8]) -> [u8; 32] {
    let actual_length = u32::try_from(bytes.len())
        .expect("portable catalog chunks are bounded by a u32 chunk size");
    let mut hasher = Sha256::new();
    hasher.update(CHUNK_HASH_DOMAIN_V1);
    hasher.update(index.to_be_bytes());
    hasher.update(actual_length.to_be_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

fn validate_canonical_sha256(
    value: &str,
    field: &'static str,
) -> PortableIntegrityResult<[u8; 32]> {
    validate_sha256(value, field).map_err(|_| PortableIntegrityError::InvalidSha256 { field })?;
    let mut bytes = [0_u8; 32];
    hex::decode_to_slice(value, &mut bytes)
        .expect("catalog SHA-256 validation accepted canonical hexadecimal bytes");
    Ok(bytes)
}

fn attestation_for_bytes(bytes: &[u8]) -> PortableManifestAttestationV1 {
    PortableManifestAttestationV1 {
        sha256: crate::hash::sha256(bytes),
        size: bytes.len() as u64,
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !bytes.is_empty() {
        match file.read_at(bytes, offset) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "catalog ended before its declared chunk boundary",
                ));
            }
            Ok(read) => {
                offset = offset.checked_add(read as u64).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "catalog offset overflow")
                })?;
                bytes = &mut bytes[read..];
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !bytes.is_empty() {
        match file.seek_read(bytes, offset) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "catalog ended before its declared chunk boundary",
                ));
            }
            Ok(read) => {
                offset = offset.checked_add(read as u64).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "catalog offset overflow")
                })?;
                bytes = &mut bytes[read..];
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn require_real_parent(path: &Path) -> PortableIntegrityResult<PathBuf> {
    if path.file_name().is_none() {
        return Err(PortableIntegrityError::InvalidPath(format!(
            "{} does not name a manifest file",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .ok_or_else(|| {
            PortableIntegrityError::InvalidPath(format!(
                "{} has no explicit parent directory",
                path.display()
            ))
        })?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(PortableIntegrityError::InvalidPath(format!(
            "{} must be a real directory",
            parent.display()
        )));
    }
    Ok(parent.to_path_buf())
}

fn create_private_temp_manifest(parent: &Path) -> PortableIntegrityResult<(PathBuf, File)> {
    for _ in 0..128 {
        let sequence = TEMP_MANIFEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".portable-chunk-manifest.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(PortableIntegrityError::InvalidPath(format!(
        "could not allocate an exclusive temporary manifest in {}",
        parent.display()
    )))
}

fn open_regular_nofollow(path: &Path) -> PortableIntegrityResult<File> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(PortableIntegrityError::NotRegularFile(path.to_path_buf()));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = options.open(path)?;
    let file_metadata = file.metadata()?;
    if !file_metadata.file_type().is_file() {
        return Err(PortableIntegrityError::NotRegularFile(path.to_path_buf()));
    }
    require_same_file(path, &path_metadata, &file_metadata)?;
    Ok(file)
}

fn require_path_still_names_file(path: &Path, file: &File) -> PortableIntegrityResult<()> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(PortableIntegrityError::PathChanged(path.to_path_buf()));
    }
    require_same_file(path, &path_metadata, &file.metadata()?)
}

#[cfg(unix)]
fn require_same_file(
    path: &Path,
    path_metadata: &fs::Metadata,
    file_metadata: &fs::Metadata,
) -> PortableIntegrityResult<()> {
    if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino() {
        return Err(PortableIntegrityError::PathChanged(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_same_file(
    _path: &Path,
    _path_metadata: &fs::Metadata,
    _file_metadata: &fs::Metadata,
) -> PortableIntegrityResult<()> {
    Ok(())
}

struct TemporaryManifest {
    path: PathBuf,
    armed: std::cell::Cell<bool>,
}

impl TemporaryManifest {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            armed: std::cell::Cell::new(true),
        }
    }

    fn disarm(&self) {
        self.armed.set(false);
    }
}

impl Drop for TemporaryManifest {
    fn drop(&mut self) {
        if self.armed.get() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests;
