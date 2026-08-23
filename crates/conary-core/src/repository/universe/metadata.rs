// conary-core/src/repository/universe/metadata.rs

//! Exact TUF target binding for one signed Remi universe.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::trust::TargetDescription;

use super::{
    RemiUniverseCanonicalMapObjectV2, RemiUniverseCatalogObjectV2, RemiUniverseManifestV2,
};

/// A manifest whose target and complete object set are bound by one verified
/// dedicated universe `targets` role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRemiUniverseTargetSet {
    pub manifest: RemiUniverseManifestV2,
    pub manifest_sha256: String,
}

/// Parse the strict manifest target and prove that the dedicated TUF target set
/// contains exactly the manifest plus every object it names.
pub fn verify_remi_universe_manifest_target(
    manifest_bytes: &[u8],
    targets: &BTreeMap<String, TargetDescription>,
) -> Result<VerifiedRemiUniverseTargetSet> {
    let manifest = serde_json::from_slice::<RemiUniverseManifestV2>(manifest_bytes)
        .map_err(|error| Error::ParseError(format!("invalid Remi universe manifest: {error}")))?;
    manifest.validate()?;
    let manifest_sha256 = manifest.manifest_sha256()?;
    let manifest_path = manifest.target_path()?;
    let manifest_target = targets.get(&manifest_path).ok_or_else(|| {
        Error::TrustError(format!(
            "verified universe targets do not authorize manifest {manifest_path}"
        ))
    })?;
    verify_exact_target_description(
        &manifest_path,
        manifest_bytes,
        &manifest_sha256,
        manifest_target,
    )?;

    let expected_paths = std::iter::once(manifest_path)
        .chain(manifest.object_target_paths())
        .collect::<BTreeSet<_>>();
    let actual_paths = targets.keys().cloned().collect::<BTreeSet<_>>();
    if actual_paths != expected_paths {
        return Err(Error::TrustError(format!(
            "verified universe target set does not exactly match manifest authority: expected {expected_paths:?}, got {actual_paths:?}"
        )));
    }

    for profile in &manifest.profiles {
        verify_descriptor_target(&profile.catalog, targets)?;
    }
    verify_canonical_descriptor_target(&manifest.canonical_map, targets)?;

    Ok(VerifiedRemiUniverseTargetSet {
        manifest,
        manifest_sha256,
    })
}

/// Verify downloaded immutable object bytes against both the strict manifest
/// descriptor and the already-verified TUF target description.
pub fn verify_remi_universe_object_target(
    path: &str,
    bytes: &[u8],
    expected_sha256: &str,
    expected_size: u64,
    targets: &BTreeMap<String, TargetDescription>,
) -> Result<()> {
    let target = targets.get(path).ok_or_else(|| {
        Error::TrustError(format!(
            "verified universe targets do not authorize object {path}"
        ))
    })?;
    if target.length != expected_size {
        return Err(Error::TrustError(format!(
            "universe object {path} target length {} disagrees with manifest length {expected_size}",
            target.length
        )));
    }
    verify_exact_target_description(path, bytes, expected_sha256, target)
}

fn verify_descriptor_target(
    descriptor: &RemiUniverseCatalogObjectV2,
    targets: &BTreeMap<String, TargetDescription>,
) -> Result<()> {
    verify_target_descriptor(
        &descriptor.target_path(),
        &descriptor.sha256,
        descriptor.size,
        targets,
    )
}

fn verify_canonical_descriptor_target(
    descriptor: &RemiUniverseCanonicalMapObjectV2,
    targets: &BTreeMap<String, TargetDescription>,
) -> Result<()> {
    verify_target_descriptor(
        &descriptor.target_path(),
        &descriptor.sha256,
        descriptor.size,
        targets,
    )
}

fn verify_target_descriptor(
    path: &str,
    expected_sha256: &str,
    expected_size: u64,
    targets: &BTreeMap<String, TargetDescription>,
) -> Result<()> {
    let target = targets.get(path).ok_or_else(|| {
        Error::TrustError(format!(
            "verified universe targets do not authorize object {path}"
        ))
    })?;
    if target.length != expected_size {
        return Err(Error::TrustError(format!(
            "universe object {path} target length {} disagrees with manifest length {expected_size}",
            target.length
        )));
    }
    require_exact_sha256(path, expected_sha256, target)
}

fn verify_exact_target_description(
    path: &str,
    bytes: &[u8],
    expected_sha256: &str,
    target: &TargetDescription,
) -> Result<()> {
    let actual_size = u64::try_from(bytes.len())
        .map_err(|_| Error::InternalError("universe object size exceeds u64".to_string()))?;
    if target.length != actual_size {
        return Err(Error::ChecksumMismatch {
            expected: format!("{} bytes", target.length),
            actual: format!("{actual_size} bytes"),
        });
    }
    require_exact_sha256(path, expected_sha256, target)?;
    let actual_sha256 = crate::hash::sha256(bytes);
    if actual_sha256 != expected_sha256 {
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual: actual_sha256,
        });
    }
    Ok(())
}

fn require_exact_sha256(
    path: &str,
    expected_sha256: &str,
    target: &TargetDescription,
) -> Result<()> {
    if target.hashes.len() != 1
        || target.hashes.get("sha256").map(String::as_str) != Some(expected_sha256)
    {
        return Err(Error::TrustError(format!(
            "universe target {path} must carry exactly its manifest SHA-256"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::{
        CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2,
        ProfileRevisionV2, ProfileSourceMemberV2, SourceStreamKindV1, SourceStreamV1,
    };
    use crate::repository::universe::{REMI_UNIVERSE_SCHEMA_V2, RemiUniverseProfileV2};

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn target(bytes: &[u8]) -> TargetDescription {
        TargetDescription {
            length: bytes.len() as u64,
            hashes: BTreeMap::from([("sha256".to_string(), crate::hash::sha256(bytes))]),
        }
    }

    fn manifest() -> RemiUniverseManifestV2 {
        let mut declared = crate::repository::supported_profiles::profile_by_public_id("fedora-44")
            .unwrap()
            .members()
            .iter()
            .collect::<Vec<_>>();
        declared.sort_by(|left, right| right.precedence.cmp(&left.precedence));
        let members = declared
            .into_iter()
            .enumerate()
            .map(|(ordinal, member)| ProfileSourceMemberV2 {
                ordinal: ordinal as u32,
                source_identity: "fedora-project".to_string(),
                repository_identity: member.repository_identity.clone(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "44".to_string(),
                },
                role: member.role,
                precedence: member.precedence,
                required: true,
                source_snapshot_sha256: digest('1'),
            })
            .collect::<Vec<_>>();
        let source_evidence = members.len() as u64;
        let revision = ProfileRevisionV2 {
            schema_version: PROFILE_REVISION_SCHEMA_V2,
            profile: "fedora-44".to_string(),
            projection_version: 1,
            members,
            catalog: CatalogArtifactV1 {
                sha256: digest('2'),
                size: 17,
            },
            logical_digest_sha256: digest('3'),
            counts: CatalogCountsV1 {
                packages: 1,
                provides: 1,
                requirement_groups: 0,
                requirement_atoms: 0,
                source_evidence,
            },
        };
        let generated_at = "2026-08-22T10:00:00Z".parse().unwrap();
        RemiUniverseManifestV2 {
            schema_version: REMI_UNIVERSE_SCHEMA_V2,
            sequence: 1,
            metadata_root_sha256: digest('4'),
            generated_at,
            expires_at: generated_at + chrono::Duration::days(7),
            profiles: vec![RemiUniverseProfileV2 {
                ordinal: 0,
                profile_revision_sha256: revision.manifest_sha256().unwrap(),
                catalog: RemiUniverseCatalogObjectV2 {
                    schema_version: CATALOG_CONTENT_SCHEMA_V1,
                    sha256: revision.catalog.sha256.clone(),
                    size: revision.catalog.size,
                    logical_digest_sha256: revision.logical_digest_sha256.clone(),
                },
                revision,
            }],
            canonical_map: RemiUniverseCanonicalMapObjectV2 {
                schema_version: crate::canonical::CANONICAL_MAP_SCHEMA_VERSION,
                sha256: digest('5'),
                size: 19,
                revision: 0,
                entry_count: 0,
            },
        }
    }

    fn authorized_targets(
        manifest: &RemiUniverseManifestV2,
        bytes: &[u8],
    ) -> BTreeMap<String, TargetDescription> {
        BTreeMap::from([
            (manifest.target_path().unwrap(), target(bytes)),
            (
                manifest.profiles[0].catalog.target_path(),
                TargetDescription {
                    length: manifest.profiles[0].catalog.size,
                    hashes: BTreeMap::from([(
                        "sha256".to_string(),
                        manifest.profiles[0].catalog.sha256.clone(),
                    )]),
                },
            ),
            (
                manifest.canonical_map.target_path(),
                TargetDescription {
                    length: manifest.canonical_map.size,
                    hashes: BTreeMap::from([(
                        "sha256".to_string(),
                        manifest.canonical_map.sha256.clone(),
                    )]),
                },
            ),
        ])
    }

    #[test]
    fn verified_targets_bind_exact_manifest_and_object_set() {
        let manifest = manifest();
        let bytes = crate::json::canonical_json(&manifest).unwrap();
        let targets = authorized_targets(&manifest, &bytes);
        let verified = verify_remi_universe_manifest_target(&bytes, &targets).unwrap();
        assert_eq!(verified.manifest, manifest);
    }

    #[test]
    fn verified_targets_reject_extra_missing_and_mixed_objects() {
        let manifest = manifest();
        let bytes = crate::json::canonical_json(&manifest).unwrap();

        let mut extra = authorized_targets(&manifest, &bytes);
        extra.insert("objects/sha256/unrelated".to_string(), target(b"unrelated"));
        assert!(verify_remi_universe_manifest_target(&bytes, &extra).is_err());

        let mut missing = authorized_targets(&manifest, &bytes);
        missing.remove(&manifest.canonical_map.target_path());
        assert!(verify_remi_universe_manifest_target(&bytes, &missing).is_err());

        let mut mixed = authorized_targets(&manifest, &bytes);
        mixed
            .get_mut(&manifest.profiles[0].catalog.target_path())
            .unwrap()
            .length += 1;
        assert!(verify_remi_universe_manifest_target(&bytes, &mixed).is_err());
    }

    #[test]
    fn object_bytes_require_manifest_and_tuf_digest_agreement() {
        let bytes = b"immutable-catalog";
        let sha256 = crate::hash::sha256(bytes);
        let path = format!("objects/sha256/{sha256}");
        let targets = BTreeMap::from([(path.clone(), target(bytes))]);
        verify_remi_universe_object_target(&path, bytes, &sha256, bytes.len() as u64, &targets)
            .unwrap();

        let mut tampered = bytes.to_vec();
        tampered[0] ^= 1;
        assert!(
            verify_remi_universe_object_target(
                &path,
                &tampered,
                &sha256,
                bytes.len() as u64,
                &targets,
            )
            .is_err()
        );
    }
}
