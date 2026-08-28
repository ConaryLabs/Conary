// conary-core/src/repository/sync/immutable_catalog/authority.rs

//! Exact source-manifest authority derived from one enrolled native repository.

use crate::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
};
use crate::error::{Error, Result};
use crate::repository::catalog::source::SourceCatalogAuthorityV1;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogScopeV1, CatalogSourceEvidenceV1, SOURCE_CATALOG_PROJECTION_VERSION_V2,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceCatalogCandidateV1, SourceEcosystemV1, SourceMetadataObjectV1,
    SourceProvenanceV1, SourceSnapshotV1, SourceStreamKindV1, SourceStreamV1,
};

use super::super::types::SyncedPackageRow;

pub(super) fn source_snapshot_matches_authority(
    manifest: &SourceSnapshotV1,
    authority: &SourceCatalogAuthorityV1,
) -> bool {
    manifest.schema_version == SOURCE_SNAPSHOT_SCHEMA_V1
        && manifest.source_profile == authority.source_profile
        && manifest.source_identity == authority.source_identity
        && manifest.repository_identity == authority.repository_identity
        && manifest.stream == authority.stream
        && manifest.stream_binding_sha256 == authority.stream_binding_sha256
        && manifest.parser_projection_version == SOURCE_CATALOG_PROJECTION_VERSION_V2
        && manifest.provenance == authority.provenance
        && manifest.authenticated_root == authority.authenticated_root
        && manifest.authenticated_objects == authority.authenticated_objects
}

pub(super) fn source_catalog_candidate(
    repo: &Repository,
    packages: Vec<SyncedPackageRow>,
    snapshot: AuthenticatedSnapshotIdentity,
    authenticated_objects: Vec<SourceMetadataObjectV1>,
) -> Result<SourceCatalogCandidateV1> {
    let authority = source_catalog_authority(repo, snapshot, authenticated_objects.clone())?;
    let source_profile = authority.source_profile.clone();
    let source_identity = authority.source_identity.clone();
    let repository_identity = authority.repository_identity.clone();
    let origin = CatalogPackageOriginV1::Source {
        source_identity: source_identity.clone(),
        repository_identity: repository_identity.clone(),
    };
    let packages = packages
        .into_iter()
        .map(|row| {
            CatalogPackageRecordV1::from_repository_projection(
                row.package,
                row.provides,
                row.requirement_groups,
                row.requirement_group_clauses,
                origin.clone(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let source_evidence = authenticated_objects
        .iter()
        .map(|object| CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role.clone(),
            source_path: object.source_path.clone(),
            sha256: object.sha256.clone(),
            size: object.size,
        })
        .collect();
    let content = CatalogContentV1::new(
        CatalogScopeV1::Source {
            source_profile,
            source_identity,
            repository_identity,
        },
        source_evidence,
        packages,
    )?;
    SourceCatalogCandidateV1::new(authority, content)
}

pub(super) fn source_catalog_authority(
    repo: &Repository,
    snapshot: AuthenticatedSnapshotIdentity,
    mut authenticated_objects: Vec<SourceMetadataObjectV1>,
) -> Result<SourceCatalogAuthorityV1> {
    repo.validate_stream_binding()?;
    let source_profile = repo.source_profile.clone().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository '{}' has no exact source profile for immutable catalog publication",
            repo.name
        ))
    })?;
    repo.validate_authenticated_snapshot(&snapshot)?;
    let policy = repo.require_source_policy()?;
    let repository_identity = repo.repository_identity.clone().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository '{}' has no exact repository identity",
            repo.name
        ))
    })?;
    let stream_binding_sha256 = repo.stream_binding_sha256.clone().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository '{}' has no native stream binding",
            repo.name
        ))
    })?;
    let parser_config = repo.require_parser_config()?.clone();
    let trust_policy = repo.require_trust_policy()?.clone();
    let parser_config_sha256 = canonical_authority_sha256(repo, "parser", &parser_config)?;
    let trust_policy_sha256 = canonical_authority_sha256(repo, "trust", &trust_policy)?;
    let authenticated_root_size = snapshot.size().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository '{}' authenticated root has no exact byte length",
            repo.name
        ))
    })?;
    authenticated_objects.sort_by(|left, right| {
        left.role
            .cmp(&right.role)
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    let ecosystem = match policy.ecosystem {
        NativeSourceEcosystem::Rpm => SourceEcosystemV1::Rpm,
        NativeSourceEcosystem::Deb => SourceEcosystemV1::Deb,
        NativeSourceEcosystem::Alpm => SourceEcosystemV1::Alpm,
        NativeSourceEcosystem::Eopkg => SourceEcosystemV1::Eopkg,
    };
    let stream = match &policy.stream {
        NativeSourceStream::Release { identity } => SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: identity.clone(),
        },
        NativeSourceStream::Channel { identity } => SourceStreamV1 {
            kind: SourceStreamKindV1::Channel,
            identity: identity.clone(),
        },
        NativeSourceStream::Rolling { identity } => SourceStreamV1 {
            kind: SourceStreamKindV1::Rolling,
            identity: identity.clone(),
        },
    };
    Ok(SourceCatalogAuthorityV1 {
        source_profile,
        source_identity: policy.source_identity.clone(),
        repository_identity,
        stream,
        stream_binding_sha256,
        provenance: SourceProvenanceV1 {
            ecosystem,
            metadata_url: repo.url.clone(),
            content_url: repo.content_url.clone(),
            parser_config,
            parser_config_sha256,
            trust_policy,
            trust_policy_sha256,
        },
        authenticated_root: CatalogArtifactV1 {
            sha256: snapshot.sha256().to_string(),
            size: authenticated_root_size,
        },
        authenticated_objects,
    })
}

fn canonical_authority_sha256(
    repo: &Repository,
    label: &str,
    value: &impl serde::Serialize,
) -> Result<String> {
    let bytes = crate::json::canonical_json(value).map_err(|error| {
        Error::ParseError(format!(
            "serialize repository '{}' {label} authority: {error}",
            repo.name
        ))
    })?;
    Ok(crate::hash::sha256(&bytes))
}
