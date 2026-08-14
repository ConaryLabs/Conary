// conary-core/src/repository/parsers/snapshot.rs

//! Authenticated native repository metadata-root identity.

use crate::error::{Error, Result};

use super::PackageMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSnapshotIdentity {
    sha256: String,
}

impl AuthenticatedSnapshotIdentity {
    pub fn from_sha256(sha256: impl Into<String>) -> Result<Self> {
        let sha256 = sha256.into();
        validate_sha256(&sha256)?;
        Ok(Self { sha256 })
    }

    pub fn for_bytes(bytes: &[u8]) -> Self {
        Self {
            sha256: crate::hash::sha256(bytes),
        }
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedRepositoryMetadata {
    pub packages: Vec<PackageMetadata>,
    pub snapshot: AuthenticatedSnapshotIdentity,
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
}
