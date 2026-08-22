// conary-core/src/repository/parsers/snapshot.rs

//! Authenticated native repository metadata-root identity.

use crate::error::{Error, Result};
use crate::repository::catalog::SourceMetadataObjectV1;

use super::PackageMetadata;

/// One exact authenticated child metadata object read by a native parser.
///
/// This is an alias of the catalog contract's object type so parser output and
/// the immutable source-snapshot manifest cannot silently grow two competing
/// representations of the same authenticated bytes.
pub type AuthenticatedMetadataObject = SourceMetadataObjectV1;
pub use crate::repository::catalog::SourceMetadataObjectRoleV1 as AuthenticatedMetadataObjectRole;

pub(crate) fn authenticated_metadata_object(
    role: AuthenticatedMetadataObjectRole,
    source_path: &str,
    bytes: &[u8],
) -> AuthenticatedMetadataObject {
    AuthenticatedMetadataObject {
        role,
        source_path: source_path.to_string(),
        sha256: crate::hash::sha256(bytes),
        size: bytes.len() as u64,
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedSnapshotIdentity {
    sha256: String,
    /// Exact byte length when this identity was created from the verified
    /// payload. Identities reconstructed from the legacy SHA-only database
    /// field have no size and must not be presented as byte-complete proof.
    size: Option<u64>,
}

// The digest is the snapshot identity. Exact byte length is supplementary
// construction evidence: legacy operational rows cannot recover it, while an
// immutable SourceSnapshotV1 must carry it explicitly before publication.
impl PartialEq for AuthenticatedSnapshotIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.sha256 == other.sha256
    }
}

impl Eq for AuthenticatedSnapshotIdentity {}

impl AuthenticatedSnapshotIdentity {
    pub fn from_sha256(sha256: impl Into<String>) -> Result<Self> {
        let sha256 = sha256.into();
        validate_sha256(&sha256)?;
        Ok(Self { sha256, size: None })
    }

    pub fn for_bytes(bytes: &[u8]) -> Self {
        Self {
            sha256: crate::hash::sha256(bytes),
            size: Some(bytes.len() as u64),
        }
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Return the exact authenticated byte length when it is known.
    ///
    /// `None` is reserved for identities reconstructed from the pre-catalog
    /// SHA-only persistence format. Callers building a source snapshot must
    /// reject that case rather than inventing a length.
    pub fn size(&self) -> Option<u64> {
        self.size
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedRepositoryMetadata {
    pub packages: Vec<PackageMetadata>,
    pub snapshot: AuthenticatedSnapshotIdentity,
    /// Exact child metadata objects covered by `snapshot` and consumed to
    /// produce `packages`. The vector is strict and canonical: at most one
    /// object per role, ordered by role then source path.
    pub authenticated_objects: Vec<AuthenticatedMetadataObject>,
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(
            "authenticated repository snapshot SHA-256 must be exactly 64 lowercase hexadecimal characters"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_identity_hashes_the_exact_authenticated_bytes() {
        let first = AuthenticatedSnapshotIdentity::for_bytes(b"authenticated metadata\n");
        let second = AuthenticatedSnapshotIdentity::for_bytes(b"authenticated metadata");

        assert_eq!(
            first.sha256(),
            "1605d58c69c0ca7c19a03491349abe8eb234a3de94c3e97e6af79d91191dbd9b"
        );
        assert_ne!(first, second);
    }

    #[test]
    fn persisted_snapshot_identity_rejects_noncanonical_sha256() {
        for invalid in ["a", &"A".repeat(64), &"g".repeat(64)] {
            assert!(AuthenticatedSnapshotIdentity::from_sha256(invalid).is_err());
        }
    }

    #[test]
    fn byte_snapshot_identity_preserves_exact_size_without_inventing_legacy_size() {
        let identity = AuthenticatedSnapshotIdentity::for_bytes(b"root bytes");
        assert_eq!(identity.size(), Some(10));

        let legacy = AuthenticatedSnapshotIdentity::from_sha256(identity.sha256()).unwrap();
        assert_eq!(legacy.size(), None);
        assert_eq!(identity, legacy);
    }

    #[test]
    fn child_object_evidence_hashes_the_exact_served_bytes() {
        let bytes = b"served Packages.gz bytes";
        let object = authenticated_metadata_object(
            AuthenticatedMetadataObjectRole::DebianPackages,
            "main/binary-amd64/Packages.gz",
            bytes,
        );

        assert_eq!(object.role, AuthenticatedMetadataObjectRole::DebianPackages);
        assert_eq!(object.source_path, "main/binary-amd64/Packages.gz");
        assert_eq!(object.sha256, crate::hash::sha256(bytes));
        assert_eq!(object.size, bytes.len() as u64);
    }
}
