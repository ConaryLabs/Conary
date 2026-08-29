// conary-core/src/filesystem/fsverity.rs

//! fs-verity enablement for CAS objects
//!
//! Enables the Linux fs-verity feature on CAS objects. Once enabled,
//! the kernel computes and caches a Merkle tree hash over the file
//! contents. composefs uses these hashes for integrity verification
//! at read time.

use std::fmt;
use std::os::fd::{AsFd, AsRawFd};
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, warn};

/// Errors that can occur during fs-verity operations
#[derive(Debug, Error)]
pub enum FsVerityError {
    /// Failed to open the file for fs-verity enablement
    #[error("Failed to open {path} for fs-verity: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Filesystem does not support fs-verity
    #[error("Filesystem does not support fs-verity: {0}")]
    NotSupported(PathBuf),

    /// ioctl failed with an unexpected error
    #[error("Failed to enable fs-verity on {path}: {source}")]
    IoctlFailed {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// The hash algorithm of a validated fs-verity measurement.
///
/// Conary enables fs-verity with SHA-256 only. Keeping this enum closed makes
/// callers handle any kernel response using another algorithm as malformed
/// authority instead of silently accepting a digest with different semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsVerityHashAlgorithm {
    /// `FS_VERITY_HASH_ALG_SHA256`.
    Sha256,
}

/// A validated SHA-256 fs-verity measurement returned by the kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FsVerityMeasurement {
    /// The kernel-reported and validated hash algorithm.
    pub algorithm: FsVerityHashAlgorithm,
    /// The 32-byte fs-verity file digest.
    pub digest: [u8; SHA256_DIGEST_SIZE],
}

/// The ioctl operation that produced an fd-bound fs-verity error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsVerityOperation {
    /// `FS_IOC_ENABLE_VERITY`.
    Enable,
    /// `FS_IOC_MEASURE_VERITY`.
    Measure,
}

impl fmt::Display for FsVerityOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enable => formatter.write_str("enable"),
            Self::Measure => formatter.write_str("measure"),
        }
    }
}

/// Errors returned by fs-verity operations on an already-open file.
#[derive(Debug, Error)]
pub enum FsVerityFileError {
    /// `FS_IOC_MEASURE_VERITY` returned `ENODATA`.
    #[error("fs-verity is not enabled on the open file")]
    NotEnabled,

    /// The file's filesystem returned `ENOTTY` for the ioctl.
    #[error("the filesystem does not implement the fs-verity {operation} ioctl")]
    IoctlUnavailable {
        /// The ioctl that was unavailable.
        operation: FsVerityOperation,
    },

    /// The kernel or filesystem returned `EOPNOTSUPP` for the ioctl.
    #[error("fs-verity {operation} is not supported for the open file")]
    NotSupported {
        /// The unsupported ioctl operation.
        operation: FsVerityOperation,
    },

    /// A successful measure ioctl returned an algorithm or size other than
    /// the exact SHA-256 contract Conary requested.
    #[error(
        "kernel returned malformed fs-verity measurement (algorithm {algorithm}, digest size {digest_size}; expected SHA-256 algorithm 1 and 32 bytes)"
    )]
    MalformedMeasurement {
        /// Raw algorithm identifier returned by the kernel.
        algorithm: u16,
        /// Raw digest length returned by the kernel.
        digest_size: u16,
    },

    /// The kernel reported a digest larger than the largest Linux fs-verity
    /// digest buffer this implementation supplies.
    #[error("fs-verity measurement exceeds the {capacity}-byte digest buffer")]
    DigestTooLarge {
        /// Digest buffer capacity supplied to the kernel.
        capacity: usize,
    },

    /// The ioctl failed with an error that does not have a typed fs-verity
    /// meaning above.
    #[error("failed to {operation} fs-verity on the open file: {source}")]
    IoctlFailed {
        /// The ioctl that failed.
        operation: FsVerityOperation,
        /// The kernel error.
        source: std::io::Error,
    },
}

/// The result of enabling and then measuring one already-open file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FsVerityEnableAndMeasureResult {
    /// Whether this call enabled fs-verity (`false` means it was already on).
    pub newly_enabled: bool,
    /// The exact SHA-256 measurement read back from the same open file.
    pub measurement: FsVerityMeasurement,
}

/// `FS_IOC_ENABLE_VERITY` ioctl number
/// `_IOW('f', 0x85, struct fsverity_enable_arg)` = `0x40806685`
///
/// `libc::Ioctl` is the request type `ioctl(2)` actually takes on the target
/// C library: `c_ulong` on glibc, `c_int` on musl. Typing the constant with it
/// keeps the call site identical on both.
const FS_IOC_ENABLE_VERITY: libc::Ioctl = 0x4080_6685;

/// `FS_IOC_MEASURE_VERITY` ioctl number
/// `_IOWR('f', 0x86, struct fsverity_digest)` = `0xc0046686`
const FS_IOC_MEASURE_VERITY: libc::Ioctl = 0xc004_6686_u32 as libc::Ioctl;

/// `FS_VERITY_HASH_ALG_SHA256`
const FS_VERITY_HASH_ALG_SHA256: u32 = 1;

const SHA256_DIGEST_SIZE: usize = 32;
const MAX_LINUX_FSVERITY_DIGEST_SIZE: usize = 64;

/// Kernel struct for the fs-verity enable ioctl.
#[repr(C)]
struct FsverityEnableArg {
    version: u32,
    hash_algorithm: u32,
    block_size: u32,
    salt_size: u32,
    salt_ptr: u64,
    sig_size: u32,
    reserved1: u32,
    sig_ptr: u64,
    reserved2: [u64; 11],
}

/// Storage for Linux's variable-length `struct fsverity_digest`.
///
/// Linux currently defines SHA-256 and SHA-512, so a 64-byte tail lets the
/// kernel return either response. Conary then validates the response against
/// its strict SHA-256 contract instead of mistaking SHA-512 for a short buffer.
#[repr(C)]
struct FsverityDigestBuffer {
    digest_algorithm: u16,
    digest_size: u16,
    digest: [u8; MAX_LINUX_FSVERITY_DIGEST_SIZE],
}

fn fsverity_enable_arg() -> FsverityEnableArg {
    FsverityEnableArg {
        version: 1,
        hash_algorithm: FS_VERITY_HASH_ALG_SHA256,
        block_size: 4096,
        salt_size: 0,
        salt_ptr: 0,
        sig_size: 0,
        reserved1: 0,
        sig_ptr: 0,
        reserved2: [0; 11],
    }
}

fn classify_ioctl_error(operation: FsVerityOperation, source: std::io::Error) -> FsVerityFileError {
    match source.raw_os_error() {
        Some(libc::ENODATA) if operation == FsVerityOperation::Measure => {
            FsVerityFileError::NotEnabled
        }
        Some(libc::ENOTTY) => FsVerityFileError::IoctlUnavailable { operation },
        Some(libc::EOPNOTSUPP) => FsVerityFileError::NotSupported { operation },
        Some(libc::EOVERFLOW) if operation == FsVerityOperation::Measure => {
            FsVerityFileError::DigestTooLarge {
                capacity: MAX_LINUX_FSVERITY_DIGEST_SIZE,
            }
        }
        _ => FsVerityFileError::IoctlFailed { operation, source },
    }
}

fn decode_sha256_measurement(
    response: &FsverityDigestBuffer,
) -> Result<FsVerityMeasurement, FsVerityFileError> {
    let algorithm = response.digest_algorithm;
    let digest_size = response.digest_size;
    if u32::from(algorithm) != FS_VERITY_HASH_ALG_SHA256
        || usize::from(digest_size) != SHA256_DIGEST_SIZE
    {
        return Err(FsVerityFileError::MalformedMeasurement {
            algorithm,
            digest_size,
        });
    }

    let mut digest = [0u8; SHA256_DIGEST_SIZE];
    digest.copy_from_slice(&response.digest[..SHA256_DIGEST_SIZE]);
    Ok(FsVerityMeasurement {
        algorithm: FsVerityHashAlgorithm::Sha256,
        digest,
    })
}

/// Enable fs-verity on an already-open, read-only file descriptor.
///
/// The descriptor remains borrowed for the complete ioctl, so callers can
/// bind identity before this call without reopening a pathname. Returns
/// `Ok(true)` when newly enabled and `Ok(false)` on `EEXIST`.
pub fn enable_fsverity_file<F: AsFd + ?Sized>(file: &F) -> Result<bool, FsVerityFileError> {
    let arg = fsverity_enable_arg();

    // SAFETY: `AsFd` guarantees a live borrowed descriptor for the duration
    // of this call, and `arg` is the exact initialized Linux UAPI structure.
    let result = unsafe {
        libc::ioctl(
            file.as_fd().as_raw_fd(),
            FS_IOC_ENABLE_VERITY,
            &raw const arg,
        )
    };
    if result == 0 {
        return Ok(true);
    }

    let source = std::io::Error::last_os_error();
    if source.raw_os_error() == Some(libc::EEXIST) {
        return Ok(false);
    }

    Err(classify_ioctl_error(FsVerityOperation::Enable, source))
}

/// Measure fs-verity on an already-open file descriptor.
///
/// Only an exact SHA-256 response is accepted. The descriptor is never
/// resolved back through a pathname, preventing rename or symlink swaps from
/// changing which inode supplies the measurement.
pub fn measure_fsverity_file<F: AsFd + ?Sized>(
    file: &F,
) -> Result<FsVerityMeasurement, FsVerityFileError> {
    let mut response = FsverityDigestBuffer {
        digest_algorithm: 0,
        digest_size: MAX_LINUX_FSVERITY_DIGEST_SIZE as u16,
        digest: [0; MAX_LINUX_FSVERITY_DIGEST_SIZE],
    };

    // SAFETY: `AsFd` guarantees a live borrowed descriptor, and `response`
    // provides the four-byte UAPI header followed by the advertised writable
    // digest buffer for the duration of the ioctl.
    let result = unsafe {
        libc::ioctl(
            file.as_fd().as_raw_fd(),
            FS_IOC_MEASURE_VERITY,
            &raw mut response,
        )
    };
    if result != 0 {
        return Err(classify_ioctl_error(
            FsVerityOperation::Measure,
            std::io::Error::last_os_error(),
        ));
    }

    decode_sha256_measurement(&response)
}

/// Enable and measure fs-verity through the same already-open descriptor.
///
/// This is the race-free primitive for callers that need to persist a digest:
/// no pathname lookup occurs between enablement and measurement.
pub fn enable_and_measure_fsverity_file<F: AsFd + ?Sized>(
    file: &F,
) -> Result<FsVerityEnableAndMeasureResult, FsVerityFileError> {
    let newly_enabled = enable_fsverity_file(file)?;
    let measurement = measure_fsverity_file(file)?;
    Ok(FsVerityEnableAndMeasureResult {
        newly_enabled,
        measurement,
    })
}

/// Enable fs-verity on a single file.
///
/// Returns `Ok(true)` if verity was newly enabled, `Ok(false)` if already
/// enabled, or an error if the operation fails for a reason other than
/// "already enabled".
pub fn enable_fsverity(path: &Path) -> Result<bool, FsVerityError> {
    // Open read-only (fs-verity requires the file not be open for writing)
    let file = std::fs::File::open(path).map_err(|e| FsVerityError::Open {
        path: path.to_path_buf(),
        source: e,
    })?;

    match enable_fsverity_file(&file) {
        Ok(newly_enabled) => Ok(newly_enabled),
        Err(FsVerityFileError::NotSupported { .. }) => {
            Err(FsVerityError::NotSupported(path.to_path_buf()))
        }
        // Preserve the legacy path API's exact behavior: only EOPNOTSUPP was
        // classified as NotSupported, while ENOTTY remained an ioctl failure.
        Err(FsVerityFileError::IoctlUnavailable { .. }) => Err(FsVerityError::IoctlFailed {
            path: path.to_path_buf(),
            source: std::io::Error::from_raw_os_error(libc::ENOTTY),
        }),
        Err(FsVerityFileError::IoctlFailed { source, .. }) => Err(FsVerityError::IoctlFailed {
            path: path.to_path_buf(),
            source,
        }),
        Err(other) => Err(FsVerityError::IoctlFailed {
            path: path.to_path_buf(),
            source: std::io::Error::other(other),
        }),
    }
}

/// Enable fs-verity on all CAS objects in the given objects directory.
///
/// Uses `CasStore::iter_objects()` to walk the directory and enables verity
/// on each file.
///
/// Returns `(enabled_count, already_enabled_count, error_count)`.
pub fn enable_fsverity_on_cas(objects_dir: &Path) -> (u64, u64, u64) {
    let mut enabled = 0u64;
    let mut already = 0u64;
    let mut errors = 0u64;

    let cas = match super::CasStore::new(objects_dir) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to open CAS objects dir: {}", e);
            return (0, 0, 1);
        }
    };

    for result in cas.iter_objects() {
        let (_hash, path) = match result {
            Ok(v) => v,
            Err(e) => {
                debug!("fs-verity: error iterating CAS: {}", e);
                errors += 1;
                continue;
            }
        };

        match enable_fsverity(&path) {
            Ok(true) => enabled += 1,
            Ok(false) => already += 1,
            Err(e) => {
                debug!("fs-verity error on {}: {}", path.display(), e);
                errors += 1;
            }
        }
    }

    debug!(
        "fs-verity: {} enabled, {} already, {} errors",
        enabled, already, errors
    );

    (enabled, already, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_response(algorithm: u16, digest_size: u16) -> FsverityDigestBuffer {
        FsverityDigestBuffer {
            digest_algorithm: algorithm,
            digest_size,
            digest: [0x5a; MAX_LINUX_FSVERITY_DIGEST_SIZE],
        }
    }

    #[test]
    fn test_fsverity_enable_arg_size() {
        // Verify struct layout matches kernel expectations
        assert_eq!(
            std::mem::size_of::<FsverityEnableArg>(),
            128,
            "FsverityEnableArg must be 128 bytes to match kernel struct"
        );
    }

    #[test]
    fn test_fsverity_digest_buffer_layout() {
        assert_eq!(
            std::mem::offset_of!(FsverityDigestBuffer, digest),
            4,
            "digest bytes must immediately follow the Linux UAPI header"
        );
    }

    #[test]
    fn test_decode_exact_sha256_measurement() {
        let response = digest_response(FS_VERITY_HASH_ALG_SHA256 as u16, 32);
        let measurement = decode_sha256_measurement(&response).unwrap();
        assert_eq!(measurement.algorithm, FsVerityHashAlgorithm::Sha256);
        assert_eq!(measurement.digest, [0x5a; SHA256_DIGEST_SIZE]);
    }

    #[test]
    fn test_decode_rejects_non_sha256_algorithm() {
        let response = digest_response(2, 32);
        assert!(matches!(
            decode_sha256_measurement(&response),
            Err(FsVerityFileError::MalformedMeasurement {
                algorithm: 2,
                digest_size: 32,
            })
        ));
    }

    #[test]
    fn test_decode_rejects_wrong_sha256_digest_size() {
        let response = digest_response(FS_VERITY_HASH_ALG_SHA256 as u16, 31);
        assert!(matches!(
            decode_sha256_measurement(&response),
            Err(FsVerityFileError::MalformedMeasurement {
                algorithm: 1,
                digest_size: 31,
            })
        ));
    }

    #[test]
    fn test_measure_errno_classification_is_typed() {
        assert!(matches!(
            classify_ioctl_error(
                FsVerityOperation::Measure,
                std::io::Error::from_raw_os_error(libc::ENODATA),
            ),
            FsVerityFileError::NotEnabled
        ));
        assert!(matches!(
            classify_ioctl_error(
                FsVerityOperation::Measure,
                std::io::Error::from_raw_os_error(libc::ENOTTY),
            ),
            FsVerityFileError::IoctlUnavailable {
                operation: FsVerityOperation::Measure,
            }
        ));
        assert!(matches!(
            classify_ioctl_error(
                FsVerityOperation::Measure,
                std::io::Error::from_raw_os_error(libc::EOPNOTSUPP),
            ),
            FsVerityFileError::NotSupported {
                operation: FsVerityOperation::Measure,
            }
        ));
        assert!(matches!(
            classify_ioctl_error(
                FsVerityOperation::Measure,
                std::io::Error::from_raw_os_error(libc::EOVERFLOW),
            ),
            FsVerityFileError::DigestTooLarge {
                capacity: MAX_LINUX_FSVERITY_DIGEST_SIZE,
            }
        ));
    }

    #[test]
    fn test_enable_errno_classification_is_typed() {
        assert!(matches!(
            classify_ioctl_error(
                FsVerityOperation::Enable,
                std::io::Error::from_raw_os_error(libc::ENOTTY),
            ),
            FsVerityFileError::IoctlUnavailable {
                operation: FsVerityOperation::Enable,
            }
        ));
        assert!(matches!(
            classify_ioctl_error(
                FsVerityOperation::Enable,
                std::io::Error::from_raw_os_error(libc::EOPNOTSUPP),
            ),
            FsVerityFileError::NotSupported {
                operation: FsVerityOperation::Enable,
            }
        ));
        match classify_ioctl_error(
            FsVerityOperation::Enable,
            std::io::Error::from_raw_os_error(libc::ENODATA),
        ) {
            FsVerityFileError::IoctlFailed { operation, source } => {
                assert_eq!(operation, FsVerityOperation::Enable);
                assert_eq!(source.raw_os_error(), Some(libc::ENODATA));
            }
            error => panic!("unexpected enable errno classification: {error}"),
        }
    }

    #[test]
    fn test_fd_bound_measurement_on_real_filesystem() {
        use composefs::fsverity::{FsVerityHashValue, Sha256HashValue, compute_verity};

        const ORIGINAL_CONTENTS: &[u8] = b"fd-bound-fsverity-test";
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("object");
        let moved_path = tmp.path().join("held-object");
        std::fs::write(&path, ORIGINAL_CONTENTS).unwrap();
        let file = std::fs::File::open(&path).unwrap();

        // A fresh file cannot already have verity enabled. Filesystems without
        // fs-verity report one of the two documented support errors instead.
        assert!(matches!(
            measure_fsverity_file(&file),
            Err(FsVerityFileError::NotEnabled)
                | Err(FsVerityFileError::IoctlUnavailable {
                    operation: FsVerityOperation::Measure,
                })
                | Err(FsVerityFileError::NotSupported {
                    operation: FsVerityOperation::Measure,
                })
        ));

        // Replace the pathname after opening. A successful call below must
        // still enable and measure ORIGINAL_CONTENTS through `file`.
        std::fs::rename(&path, &moved_path).unwrap();
        std::fs::write(&path, b"replacement-path-contents").unwrap();

        match enable_and_measure_fsverity_file(&file) {
            Ok(result) => {
                assert!(result.newly_enabled);
                assert_eq!(result.measurement.algorithm, FsVerityHashAlgorithm::Sha256);
                assert_eq!(
                    hex::encode(result.measurement.digest),
                    compute_verity::<Sha256HashValue>(ORIGINAL_CONTENTS).to_hex()
                );
                assert_eq!(measure_fsverity_file(&file).unwrap(), result.measurement);

                let replacement = std::fs::File::open(&path).unwrap();
                assert!(matches!(
                    measure_fsverity_file(&replacement),
                    Err(FsVerityFileError::NotEnabled)
                ));

                let repeated = enable_and_measure_fsverity_file(&file).unwrap();
                assert!(!repeated.newly_enabled);
                assert_eq!(repeated.measurement, result.measurement);
            }
            Err(FsVerityFileError::IoctlUnavailable {
                operation: FsVerityOperation::Enable,
            })
            | Err(FsVerityFileError::NotSupported {
                operation: FsVerityOperation::Enable,
            }) => {}
            Err(error) => panic!("unexpected fs-verity enablement result: {error}"),
        }
    }

    #[test]
    fn test_enable_fsverity_nonexistent_file() {
        let result = enable_fsverity(Path::new("/nonexistent/file"));
        assert!(result.is_err());
    }

    #[test]
    fn test_enable_fsverity_on_cas_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (enabled, already, errors) = enable_fsverity_on_cas(tmp.path());
        assert_eq!(enabled, 0);
        assert_eq!(already, 0);
        assert_eq!(errors, 0);
    }
}
