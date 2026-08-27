// crates/conary-core/src/repository/parsers/debian/release.rs

//! Authenticated Debian Release identity and Packages-object path authority.

use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogMetadataObjectScratchV1, CatalogMetadataScratchV1};
use crate::repository::parsers::{AuthenticatedMetadataObjectRole, AuthenticatedSnapshotIdentity};
use crate::repository::registry::validate_source_identifier;

use super::super::common;
use super::validate_sha256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackagesIndexAuthority {
    release_path: String,
    source_path: String,
}

impl PackagesIndexAuthority {
    pub(super) fn new(distribution: &str, component: &str, architecture: &str) -> Result<Self> {
        validate_path_component(distribution, "Debian distribution")?;
        validate_path_component(component, "Debian component")?;
        validate_path_component(architecture, "Debian architecture")?;

        let release_path = format!("{component}/binary-{architecture}/Packages.gz");
        let source_path = format!("dists/{distribution}/{release_path}");
        common::validate_filename(&release_path).map_err(Error::ParseError)?;
        common::validate_filename(&source_path).map_err(Error::ParseError)?;
        Ok(Self {
            release_path,
            source_path,
        })
    }

    pub(super) fn release_path(&self) -> &str {
        &self.release_path
    }

    pub(super) fn source_path(&self) -> &str {
        &self.source_path
    }

    pub(super) fn scratch(
        &self,
        authenticated: &ReleaseSha256Entry,
    ) -> Result<CatalogMetadataScratchV1> {
        CatalogMetadataScratchV1::from_signed_objects(vec![CatalogMetadataObjectScratchV1 {
            role: AuthenticatedMetadataObjectRole::DebianPackages,
            source_path: self.source_path.clone(),
            size: authenticated.size,
        }])
    }
}

fn validate_path_component(value: &str, label: &str) -> Result<()> {
    validate_source_identifier(value, label)?;
    if matches!(value, "." | "..") {
        return Err(Error::ParseError(format!(
            "{label} must not be a relative path marker"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseSha256Entry {
    pub(super) sha256: String,
    pub(super) size: u64,
}

pub(super) fn authenticated_release_snapshot(bytes: &[u8]) -> AuthenticatedSnapshotIdentity {
    AuthenticatedSnapshotIdentity::for_bytes(bytes)
}

pub(super) fn parse_release_sha256_entry(
    release: &[u8],
    target: &str,
) -> Result<ReleaseSha256Entry> {
    let release = std::str::from_utf8(release)
        .map_err(|error| Error::ParseError(format!("Debian Release is not UTF-8: {error}")))?;
    let mut in_sha256 = false;
    let mut saw_sha256 = false;
    let mut matched = None;
    for line in release.lines() {
        if line == "SHA256:" {
            if saw_sha256 {
                return Err(Error::ParseError(
                    "Debian Release repeats the SHA256 field".to_string(),
                ));
            }
            saw_sha256 = true;
            in_sha256 = true;
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_sha256 = false;
            continue;
        }
        if !in_sha256 {
            continue;
        }
        let columns = line.split_ascii_whitespace().collect::<Vec<_>>();
        if columns.len() != 3 {
            return Err(Error::ParseError(format!(
                "Debian Release SHA256 entry must have checksum, size, and path: {line:?}"
            )));
        }
        validate_sha256(columns[0], "Debian Release")?;
        let size = columns[1].parse::<u64>().map_err(|error| {
            Error::ParseError(format!(
                "Debian Release SHA256 size '{}' is invalid: {error}",
                columns[1]
            ))
        })?;
        common::validate_filename(columns[2]).map_err(Error::ParseError)?;
        if columns[2] == target {
            if matched.is_some() {
                return Err(Error::ParseError(format!(
                    "Debian Release repeats SHA256 authority for {target}"
                )));
            }
            matched = Some(ReleaseSha256Entry {
                sha256: columns[0].to_string(),
                size,
            });
        }
    }
    if !saw_sha256 {
        return Err(Error::GpgVerificationFailed(
            "authenticated Debian Release has no SHA256 field".to_string(),
        ));
    }
    matched.ok_or_else(|| {
        Error::GpgVerificationFailed(format!(
            "authenticated Debian Release has no SHA256 identity for {target}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_authority_separates_release_and_repository_paths() {
        for distribution in [
            "resolute",
            "resolute-updates",
            "resolute-security",
            "resolute-backports",
        ] {
            let authority =
                PackagesIndexAuthority::new(distribution, "multiverse", "amd64").unwrap();
            assert_eq!(
                authority.release_path(),
                "multiverse/binary-amd64/Packages.gz"
            );
            assert_eq!(
                authority.source_path(),
                format!("dists/{distribution}/multiverse/binary-amd64/Packages.gz")
            );
        }
    }

    #[test]
    fn packages_authority_rejects_noncanonical_components() {
        for (distribution, component, architecture) in [
            ("../resolute", "main", "amd64"),
            ("resolute", "/main", "amd64"),
            ("resolute", "main", "amd64/../../arm64"),
            (".", "main", "amd64"),
            ("resolute", "..", "amd64"),
            ("", "main", "amd64"),
        ] {
            assert!(PackagesIndexAuthority::new(distribution, component, architecture).is_err());
        }
    }

    #[test]
    fn release_lookup_uses_release_relative_path() {
        let authority = PackagesIndexAuthority::new("resolute", "multiverse", "amd64").unwrap();
        let release = format!(
            "Origin: Ubuntu\nSHA256:\n {} 8192 {}\n",
            "a".repeat(64),
            authority.release_path()
        );
        let entry =
            parse_release_sha256_entry(release.as_bytes(), authority.release_path()).unwrap();
        assert_eq!(entry.sha256, "a".repeat(64));
        assert_eq!(entry.size, 8192);

        let error =
            parse_release_sha256_entry(release.as_bytes(), authority.source_path()).unwrap_err();
        assert!(error.to_string().contains("has no SHA256 identity"));
    }

    #[test]
    fn signed_packages_size_reserves_the_repository_relative_object() {
        let authority = PackagesIndexAuthority::new("resolute", "main", "amd64").unwrap();
        let entry = ReleaseSha256Entry {
            sha256: "a".repeat(64),
            size: 8192,
        };
        let requirement = authority.scratch(&entry).unwrap();

        assert_eq!(requirement.required_additional_bytes, 8192);
        assert_eq!(requirement.objects.len(), 1);
        assert_eq!(
            requirement.objects[0].role,
            AuthenticatedMetadataObjectRole::DebianPackages
        );
        assert_eq!(
            requirement.objects[0].source_path,
            "dists/resolute/main/binary-amd64/Packages.gz"
        );
    }

    #[test]
    fn snapshot_identity_owns_the_verified_cleartext_release_payload() {
        let release = b"Origin: Example\nSHA256:\n";
        assert_eq!(
            authenticated_release_snapshot(release).size(),
            Some(release.len() as u64)
        );
        assert_eq!(
            authenticated_release_snapshot(release),
            AuthenticatedSnapshotIdentity::for_bytes(release)
        );
        assert_ne!(
            authenticated_release_snapshot(release),
            AuthenticatedSnapshotIdentity::for_bytes(b"armored InRelease envelope")
        );
    }
}
