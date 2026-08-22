// crates/conary-core/src/repository/catalog/profile.rs

//! Deterministic profile-catalog composition from verified source catalogs.

use super::{
    CatalogBindingV1, CatalogCandidateWriter, CatalogContentV1, CatalogPackageOriginV1,
    CatalogPackageRecordV1, CatalogReader, CatalogScopeV1, CatalogSourceEvidenceV1,
    PROFILE_REVISION_SCHEMA_V1, ProfileRevisionV1, ProfileSourceMemberV1, SourceSnapshotV1,
};
use crate::error::{Error, Result};

/// One verified source selected explicitly for a profile revision.
pub struct ProfileCatalogMemberInputV1<'a> {
    pub ordinal: u32,
    pub priority: i32,
    pub required: bool,
    pub manifest: &'a SourceSnapshotV1,
    pub reader: &'a CatalogReader,
}

/// Complete logical profile candidate before its SQLite byte artifact is bound.
pub struct ProfileCatalogCandidateV1 {
    profile: String,
    projection_version: u32,
    members: Vec<ProfileSourceMemberV1>,
    content: CatalogContentV1,
}

impl ProfileCatalogCandidateV1 {
    pub fn compose(
        profile: impl Into<String>,
        projection_version: u32,
        inputs: Vec<ProfileCatalogMemberInputV1<'_>>,
    ) -> Result<Self> {
        let profile = profile.into();
        let mut packages = Vec::new();
        let (members, evidence) =
            visit_profile_packages(&profile, projection_version, inputs, |package| {
                packages.push(package);
                Ok(())
            })?;

        let content = CatalogContentV1::new(
            CatalogScopeV1::Profile {
                profile: profile.clone(),
            },
            evidence,
            packages,
        )?;
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
    pub fn members(&self) -> &[ProfileSourceMemberV1] {
        &self.members
    }

    pub fn bind(self, binding: &CatalogBindingV1) -> Result<ProfileRevisionV1> {
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
    inputs: Vec<ProfileCatalogMemberInputV1<'_>>,
) -> Result<ProfileRevisionV1> {
    let profile = profile.into();
    let mut writer = CatalogCandidateWriter::create(
        path,
        CatalogScopeV1::Profile {
            profile: profile.clone(),
        },
    )?;
    let (members, evidence) =
        visit_profile_packages(&profile, projection_version, inputs, |package| {
            writer.package(package)
        })?;
    let binding = writer.finish(evidence)?;
    bind_profile_revision(profile, projection_version, members, &binding)
}

fn visit_profile_packages(
    profile: &str,
    projection_version: u32,
    mut inputs: Vec<ProfileCatalogMemberInputV1<'_>>,
    mut visitor: impl FnMut(CatalogPackageRecordV1) -> Result<()>,
) -> Result<(Vec<ProfileSourceMemberV1>, Vec<CatalogSourceEvidenceV1>)> {
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
        members.push(ProfileSourceMemberV1 {
            ordinal: input.ordinal,
            source_identity: input.manifest.source_identity.clone(),
            repository_identity: input.manifest.repository_identity.clone(),
            stream: input.manifest.stream.clone(),
            priority: input.priority,
            required: input.required,
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        });
        evidence.push(CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: input.ordinal,
            source_identity: input.manifest.source_identity.clone(),
            repository_identity: input.manifest.repository_identity.clone(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        });
        input.reader.for_each_package(|mut package| {
            package.origin = CatalogPackageOriginV1::Profile {
                member_ordinal: input.ordinal,
                source_identity: input.manifest.source_identity.clone(),
                repository_identity: input.manifest.repository_identity.clone(),
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            };
            visitor(package)
        })?;
    }
    Ok((members, evidence))
}

fn bind_profile_revision(
    profile: String,
    projection_version: u32,
    members: Vec<ProfileSourceMemberV1>,
    binding: &CatalogBindingV1,
) -> Result<ProfileRevisionV1> {
    let manifest = ProfileRevisionV1 {
        schema_version: PROFILE_REVISION_SCHEMA_V1,
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
