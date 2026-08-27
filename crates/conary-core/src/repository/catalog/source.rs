// crates/conary-core/src/repository/catalog/source.rs

//! Native source-catalog candidate binding.

use super::{
    CatalogArtifactV1, CatalogBindingV1, CatalogContentV1, CatalogScopeV1,
    SOURCE_CATALOG_PROJECTION_VERSION_V2, SOURCE_SNAPSHOT_SCHEMA_V1, SourceMetadataObjectV1,
    SourceProvenanceV1, SourceSnapshotV1, SourceStreamV1,
};
use crate::error::{Error, Result};

/// Exact source authority captured before normalized content is bound to its
/// standalone SQLite artifact.
pub(in crate::repository) struct SourceCatalogAuthorityV1 {
    pub source_profile: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub stream: SourceStreamV1,
    pub stream_binding_sha256: String,
    pub provenance: SourceProvenanceV1,
    pub authenticated_root: CatalogArtifactV1,
    pub authenticated_objects: Vec<SourceMetadataObjectV1>,
}

/// Complete logical native-source candidate before its SQLite artifact is bound.
pub struct SourceCatalogCandidateV1 {
    source_profile: String,
    source_identity: String,
    repository_identity: String,
    stream: SourceStreamV1,
    stream_binding_sha256: String,
    provenance: SourceProvenanceV1,
    authenticated_root: CatalogArtifactV1,
    authenticated_objects: Vec<SourceMetadataObjectV1>,
    content: CatalogContentV1,
}

impl SourceCatalogCandidateV1 {
    pub(in crate::repository) fn new(
        authority: SourceCatalogAuthorityV1,
        content: CatalogContentV1,
    ) -> Result<Self> {
        let SourceCatalogAuthorityV1 {
            source_profile,
            source_identity,
            repository_identity,
            stream,
            stream_binding_sha256,
            provenance,
            authenticated_root,
            authenticated_objects,
        } = authority;
        let candidate = Self {
            source_profile,
            source_identity,
            repository_identity,
            stream,
            stream_binding_sha256,
            provenance,
            authenticated_root,
            authenticated_objects,
            content,
        };
        candidate.validate_logical_authority()?;
        Ok(candidate)
    }

    #[must_use]
    pub fn content(&self) -> &CatalogContentV1 {
        &self.content
    }

    #[must_use]
    pub fn source_profile(&self) -> &str {
        &self.source_profile
    }

    #[must_use]
    pub fn repository_identity(&self) -> &str {
        &self.repository_identity
    }

    pub fn bind(self, binding: &CatalogBindingV1) -> Result<SourceSnapshotV1> {
        if binding.logical_digest_sha256 != self.content.logical_digest_sha256()?
            || binding.counts != self.content.counts()?
        {
            return Err(Error::ConflictError(
                "source catalog artifact binding does not match its authenticated logical content"
                    .to_string(),
            ));
        }
        bind_source_snapshot(self.into_authority(), binding)
    }

    fn into_authority(self) -> SourceCatalogAuthorityV1 {
        SourceCatalogAuthorityV1 {
            source_profile: self.source_profile,
            source_identity: self.source_identity,
            repository_identity: self.repository_identity,
            stream: self.stream,
            stream_binding_sha256: self.stream_binding_sha256,
            provenance: self.provenance,
            authenticated_root: self.authenticated_root,
            authenticated_objects: self.authenticated_objects,
        }
    }

    fn validate_logical_authority(&self) -> Result<()> {
        let expected_scope = CatalogScopeV1::Source {
            source_profile: self.source_profile.clone(),
            source_identity: self.source_identity.clone(),
            repository_identity: self.repository_identity.clone(),
        };
        if self.content.scope != expected_scope {
            return Err(Error::ConfigError(
                "source catalog logical scope disagrees with its source authority".to_string(),
            ));
        }
        let evidence = self.content.source_evidence.iter().collect::<Vec<_>>();
        if evidence.len() != self.authenticated_objects.len()
            || self
                .authenticated_objects
                .iter()
                .zip(evidence)
                .any(|(object, evidence)| {
                    !matches!(
                        evidence,
                        super::CatalogSourceEvidenceV1::AuthenticatedObject {
                            role,
                            source_path,
                            sha256,
                            size,
                        } if role == &object.role
                            && source_path == &object.source_path
                            && sha256 == &object.sha256
                            && size == &object.size
                    )
                })
        {
            return Err(Error::ConfigError(
                "source catalog logical evidence disagrees with authenticated objects".to_string(),
            ));
        }
        self.content.validate()
    }
}

pub(in crate::repository) fn bind_source_snapshot(
    authority: SourceCatalogAuthorityV1,
    binding: &CatalogBindingV1,
) -> Result<SourceSnapshotV1> {
    let expected_scope = CatalogScopeV1::Source {
        source_profile: authority.source_profile.clone(),
        source_identity: authority.source_identity.clone(),
        repository_identity: authority.repository_identity.clone(),
    };
    if binding.scope != expected_scope
        || binding.counts.source_evidence != authority.authenticated_objects.len() as u64
    {
        return Err(Error::ConflictError(
            "source catalog artifact binding does not match its authenticated authority"
                .to_string(),
        ));
    }
    let manifest = SourceSnapshotV1 {
        schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
        source_profile: authority.source_profile,
        source_identity: authority.source_identity,
        repository_identity: authority.repository_identity,
        stream: authority.stream,
        stream_binding_sha256: authority.stream_binding_sha256,
        parser_projection_version: SOURCE_CATALOG_PROJECTION_VERSION_V2,
        provenance: authority.provenance,
        authenticated_root: authority.authenticated_root,
        authenticated_objects: authority.authenticated_objects,
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    };
    manifest.validate()?;
    Ok(manifest)
}
