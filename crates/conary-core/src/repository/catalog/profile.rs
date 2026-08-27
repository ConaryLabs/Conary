// crates/conary-core/src/repository/catalog/profile.rs

//! Deterministic profile-catalog composition from verified source catalogs.

use super::{
    CatalogBindingV1, CatalogCandidateWriter, CatalogContentV1, CatalogPackageOriginV1,
    CatalogPackageRecordV1, CatalogProfileCandidateScratchV1, CatalogProfileMemberScratchV1,
    CatalogReader, CatalogScopeV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V2,
    ProfileRevisionV2, ProfileSourceMemberV2, SourceSnapshotV1,
};
use crate::error::{Error, Result};
use crate::repository::supported_profiles::ProfileSourceRole;
use std::collections::BTreeMap;
use std::sync::Arc;

/// One verified source selected explicitly for a profile revision.
pub struct ProfileCatalogMemberInputV2<'a> {
    pub ordinal: u32,
    pub role: ProfileSourceRole,
    pub precedence: i32,
    pub required: bool,
    pub manifest: &'a SourceSnapshotV1,
    pub reader: &'a CatalogReader,
}

/// Complete logical profile candidate before its SQLite byte artifact is bound.
pub struct ProfileCatalogCandidateV2 {
    profile: String,
    projection_version: u32,
    members: Vec<ProfileSourceMemberV2>,
    content: CatalogContentV1,
}

impl ProfileCatalogCandidateV2 {
    pub fn compose(
        profile: impl Into<String>,
        projection_version: u32,
        inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
    ) -> Result<Self> {
        let profile = profile.into();
        let scope = CatalogScopeV1::Profile {
            profile: profile.clone(),
        };
        let mut packages = Vec::new();
        let mut package_indexes = BTreeMap::new();
        let (members, evidence) =
            visit_profile_packages(&profile, projection_version, inputs, |mut package| {
                package.canonicalize_for_scope(&scope)?;
                if let Some(index) = package_indexes.get(&package.package_key_sha256).copied() {
                    let existing: &CatalogPackageRecordV1 = &packages[index];
                    if existing.same_profile_record(&package)? {
                        return Ok(());
                    }
                    return Err(Error::ConflictError(format!(
                        "profile '{}' has contradictory package identity {} {}-{} {:?}",
                        profile,
                        package.name,
                        package.version,
                        package.package_release,
                        package.architecture
                    )));
                }
                package_indexes.insert(package.package_key_sha256.clone(), packages.len());
                packages.push(package);
                Ok(())
            })?;

        let content = CatalogContentV1::new(scope, evidence, packages)?;
        Ok(Self {
            profile,
            projection_version,
            members,
            content,
        })
    }

    #[must_use]
    pub fn content(&self) -> &CatalogContentV1 {
        &self.content
    }

    #[must_use]
    pub fn members(&self) -> &[ProfileSourceMemberV2] {
        &self.members
    }

    pub fn bind(self, binding: &CatalogBindingV1) -> Result<ProfileRevisionV2> {
        let expected_scope = CatalogScopeV1::Profile {
            profile: self.profile.clone(),
        };
        if binding.scope != expected_scope
            || binding.logical_digest_sha256 != self.content.logical_digest_sha256()?
            || binding.counts != self.content.counts()?
        {
            return Err(Error::ConflictError(
                "profile catalog artifact binding does not match its composed logical content"
                    .to_string(),
            ));
        }
        bind_profile_revision(self.profile, self.projection_version, self.members, binding)
    }
}

/// Compose one profile candidate with a bounded package iterator and write it
/// directly to private SQLite state.
pub fn write_profile_catalog_candidate(
    path: impl AsRef<std::path::Path>,
    profile: impl Into<String>,
    projection_version: u32,
    inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
) -> Result<ProfileRevisionV2> {
    write_profile_catalog_candidate_inner(path.as_ref(), profile, projection_version, inputs, None)
}

/// Compose a profile candidate with typed SQLite finalization admission.
pub fn write_profile_catalog_candidate_with_scratch_admission(
    path: impl AsRef<std::path::Path>,
    profile: impl Into<String>,
    projection_version: u32,
    inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
    scratch_admission: Arc<dyn super::CatalogScratchAdmission>,
) -> Result<ProfileRevisionV2> {
    write_profile_catalog_candidate_inner(
        path.as_ref(),
        profile,
        projection_version,
        inputs,
        Some(scratch_admission),
    )
}

/// Derive the exact ordered profile-member contract from verified source
/// manifests without visiting or copying package rows.
///
/// This is the identity-only half of profile composition. Callers can use it
/// to select an already durable immutable profile revision, but reuse remains
/// valid only when the complete returned contract and projection version
/// match and the selected bundle is independently reopened.
pub fn derive_profile_catalog_members(
    profile: &str,
    projection_version: u32,
    inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
) -> Result<Vec<ProfileSourceMemberV2>> {
    let (members, _) = visit_profile_members(profile, projection_version, inputs, |_, _| Ok(()))?;
    Ok(members)
}

fn write_profile_catalog_candidate_inner(
    path: &std::path::Path,
    profile: impl Into<String>,
    projection_version: u32,
    inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
    scratch_admission: Option<Arc<dyn super::CatalogScratchAdmission>>,
) -> Result<ProfileRevisionV2> {
    let profile = profile.into();
    let scope = CatalogScopeV1::Profile {
        profile: profile.clone(),
    };
    let mut writer = match scratch_admission {
        Some(admission) => CatalogCandidateWriter::create_with_profile_scratch_admission(
            path,
            scope,
            admission,
            profile_candidate_scratch(&inputs)?,
        )?,
        None => CatalogCandidateWriter::create(path, scope)?,
    };
    let (members, evidence) = visit_profile_members(
        &profile,
        projection_version,
        inputs,
        |input, source_snapshot_sha256| {
            writer.copy_profile_member(
                input.reader,
                input.ordinal,
                input.manifest.source_identity.clone(),
                input.manifest.repository_identity.clone(),
                source_snapshot_sha256.to_string(),
            )
        },
    )?;
    let binding = writer.finish(evidence)?;
    bind_profile_revision(profile, projection_version, members, &binding)
}

fn profile_candidate_scratch(
    inputs: &[ProfileCatalogMemberInputV2<'_>],
) -> Result<CatalogProfileCandidateScratchV1> {
    for input in inputs {
        input.manifest.validate()?;
        require_reader_matches_source(input.reader, input.manifest)?;
    }
    CatalogProfileCandidateScratchV1::from_members(
        inputs
            .iter()
            .map(|input| CatalogProfileMemberScratchV1 {
                ordinal: input.ordinal,
                catalog_bytes: input.manifest.catalog.size,
                package_count: input.manifest.counts.packages,
            })
            .collect(),
    )
}

fn visit_profile_packages(
    profile: &str,
    projection_version: u32,
    inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
    mut visitor: impl FnMut(CatalogPackageRecordV1) -> Result<()>,
) -> Result<(Vec<ProfileSourceMemberV2>, Vec<CatalogSourceEvidenceV1>)> {
    visit_profile_members(
        profile,
        projection_version,
        inputs,
        |input, source_snapshot_sha256| {
            input.reader.for_each_package(|mut package| {
                package.origin = CatalogPackageOriginV1::Profile {
                    member_ordinal: input.ordinal,
                    source_identity: input.manifest.source_identity.clone(),
                    repository_identity: input.manifest.repository_identity.clone(),
                    source_snapshot_sha256: source_snapshot_sha256.to_string(),
                };
                visitor(package)
            })
        },
    )
}

fn visit_profile_members(
    profile: &str,
    projection_version: u32,
    mut inputs: Vec<ProfileCatalogMemberInputV2<'_>>,
    mut visitor: impl FnMut(&ProfileCatalogMemberInputV2<'_>, &str) -> Result<()>,
) -> Result<(Vec<ProfileSourceMemberV2>, Vec<CatalogSourceEvidenceV1>)> {
    if projection_version == 0 {
        return Err(Error::ConfigError(
            "profile catalog projection version must be positive".to_string(),
        ));
    }
    if inputs.is_empty() {
        return Err(Error::ConfigError(
            "profile catalog requires at least one source member".to_string(),
        ));
    }
    inputs.sort_by_key(|input| input.ordinal);
    let mut members = Vec::with_capacity(inputs.len());
    let mut evidence = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.into_iter().enumerate() {
        let expected_ordinal = u32::try_from(index).map_err(|_| {
            Error::ConfigError("profile catalog contains too many members".to_string())
        })?;
        if input.ordinal != expected_ordinal {
            return Err(Error::ConfigError(format!(
                "profile catalog member ordinal {} is noncanonical; expected {expected_ordinal}",
                input.ordinal
            )));
        }
        input.manifest.validate()?;
        if input.manifest.source_profile != profile {
            return Err(Error::ConfigError(format!(
                "profile catalog '{}' cannot include source snapshot for profile '{}'",
                profile, input.manifest.source_profile
            )));
        }
        let source_snapshot_sha256 = input.manifest.manifest_sha256()?;
        require_reader_matches_source(input.reader, input.manifest)?;
        members.push(ProfileSourceMemberV2 {
            ordinal: input.ordinal,
            role: input.role,
            source_identity: input.manifest.source_identity.clone(),
            repository_identity: input.manifest.repository_identity.clone(),
            stream: input.manifest.stream.clone(),
            precedence: input.precedence,
            required: input.required,
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        });
        evidence.push(CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: input.ordinal,
            source_identity: input.manifest.source_identity.clone(),
            repository_identity: input.manifest.repository_identity.clone(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        });
        visitor(&input, &source_snapshot_sha256)?;
    }
    Ok((members, evidence))
}

fn bind_profile_revision(
    profile: String,
    projection_version: u32,
    members: Vec<ProfileSourceMemberV2>,
    binding: &CatalogBindingV1,
) -> Result<ProfileRevisionV2> {
    let manifest = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile,
        projection_version,
        members,
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    };
    manifest.validate()?;
    Ok(manifest)
}

fn require_reader_matches_source(
    reader: &CatalogReader,
    manifest: &SourceSnapshotV1,
) -> Result<()> {
    let expected_scope = CatalogScopeV1::Source {
        source_profile: manifest.source_profile.clone(),
        source_identity: manifest.source_identity.clone(),
        repository_identity: manifest.repository_identity.clone(),
    };
    let binding = reader.binding();
    if binding.scope != expected_scope
        || binding.artifact != manifest.catalog
        || binding.logical_digest_sha256 != manifest.logical_digest_sha256
        || binding.counts != manifest.counts
    {
        return Err(Error::ConflictError(format!(
            "source catalog reader for '{}' does not match its exact snapshot manifest",
            manifest.repository_identity
        )));
    }
    let expected_evidence = manifest
        .authenticated_objects
        .iter()
        .map(|object| CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role.clone(),
            source_path: object.source_path.clone(),
            sha256: object.sha256.clone(),
            size: object.size,
        })
        .collect::<Vec<_>>();
    if reader.source_evidence()? != expected_evidence {
        return Err(Error::ConflictError(format!(
            "source catalog reader for '{}' has mixed authenticated evidence",
            manifest.repository_identity
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
