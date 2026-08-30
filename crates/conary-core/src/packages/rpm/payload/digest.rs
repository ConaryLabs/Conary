// conary-core/src/packages/rpm/payload/digest.rs

//! Typed RPM per-file digest computation state.

use super::header::RpmFileDigestAlgorithm;
use md5::Md5;
use sha1::{Digest as Digest10, Sha1};
use sha2::{Digest as Digest11, Sha224, Sha384, Sha512};
use sha3::{Sha3_256, Sha3_512};

/// Algorithm-tagged digest evidence computed from one bounded CPIO member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComputedFileDigest {
    pub(super) algorithm: RpmFileDigestAlgorithm,
    pub(super) hex: String,
}

/// Complete content evidence produced while writing one regular spool file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComputedRegularContent {
    pub(super) sha256: String,
    pub(super) declared: ComputedFileDigest,
    /// Exact bytes submitted to cryptographic hash states. SHA-256 authority
    /// reuses the canonical state; every other RPM algorithm adds one state.
    pub(super) bytes_hashed: u64,
}

/// The second state required only when RPM's declared digest is not SHA-256.
/// Keeping this enum closed over the pinned algorithm table prevents an
/// untyped name lookup or compatibility fallback during payload admission.
pub(super) enum DeclaredDigestHasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha2_224(Sha224),
    Sha2_384(Sha384),
    Sha2_512(Sha512),
    Sha3_256(Sha3_256),
    Sha3_512(Sha3_512),
}

impl DeclaredDigestHasher {
    pub(super) fn new(algorithm: RpmFileDigestAlgorithm) -> Option<Self> {
        match algorithm {
            RpmFileDigestAlgorithm::Md5 => Some(Self::Md5(Md5::new())),
            RpmFileDigestAlgorithm::Sha1 => Some(Self::Sha1(Sha1::new())),
            RpmFileDigestAlgorithm::Sha2_224 => Some(Self::Sha2_224(Sha224::new())),
            RpmFileDigestAlgorithm::Sha2_256 => None,
            RpmFileDigestAlgorithm::Sha2_384 => Some(Self::Sha2_384(Sha384::new())),
            RpmFileDigestAlgorithm::Sha2_512 => Some(Self::Sha2_512(Sha512::new())),
            RpmFileDigestAlgorithm::Sha3_256 => Some(Self::Sha3_256(Sha3_256::new())),
            RpmFileDigestAlgorithm::Sha3_512 => Some(Self::Sha3_512(Sha3_512::new())),
        }
    }

    pub(super) fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Md5(hasher) => Digest11::update(hasher, bytes),
            Self::Sha1(hasher) => Digest10::update(hasher, bytes),
            Self::Sha2_224(hasher) => Digest11::update(hasher, bytes),
            Self::Sha2_384(hasher) => Digest11::update(hasher, bytes),
            Self::Sha2_512(hasher) => Digest11::update(hasher, bytes),
            Self::Sha3_256(hasher) => Digest10::update(hasher, bytes),
            Self::Sha3_512(hasher) => Digest10::update(hasher, bytes),
        }
    }

    pub(super) fn finalize(self) -> String {
        match self {
            Self::Md5(hasher) => hex::encode(Digest11::finalize(hasher)),
            Self::Sha1(hasher) => hex::encode(Digest10::finalize(hasher)),
            Self::Sha2_224(hasher) => hex::encode(Digest11::finalize(hasher)),
            Self::Sha2_384(hasher) => hex::encode(Digest11::finalize(hasher)),
            Self::Sha2_512(hasher) => hex::encode(Digest11::finalize(hasher)),
            Self::Sha3_256(hasher) => hex::encode(Digest10::finalize(hasher)),
            Self::Sha3_512(hasher) => hex::encode(Digest10::finalize(hasher)),
        }
    }
}
