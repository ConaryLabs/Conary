// crates/conary-core/src/repository/sync/immutable_catalog.rs

//! Native repository projection into immutable source-catalog candidates.

use std::path::Path;

use crate::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
};
use crate::error::{Error, Result};
use crate::repository::catalog::source::SourceCatalogAuthorityV1;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogScopeV1, CatalogSourceEvidenceV1, SourceCatalogCandidateV1, SourceEcosystemV1,
    SourceMetadataObjectV1, SourceProvenanceV1, SourceStreamKindV1, SourceStreamV1,
};

use super::fetch_repository_native_snapshot;
use super::types::{RepositorySyncSnapshot, SyncedPackageRow};

/// Fetch and normalize one authenticated native repository without writing any
/// package, capability, or requirement rows into operational SQLite.
pub async fn fetch_native_source_catalog(
    repo: &Repository,
    keyring_dir: &Path,
) -> Result<SourceCatalogCandidateV1> {
    let fetched = fetch_repository_native_snapshot(repo, keyring_dir).await?;
    let RepositorySyncSnapshot::NativeRows {
        packages,
        snapshot,
        authenticated_objects,
    } = fetched
    else {
        unreachable!("native repository fetch returned a non-native snapshot")
    };
    source_catalog_candidate(repo, packages, snapshot, authenticated_objects)
}

fn source_catalog_candidate(
    repo: &Repository,
    packages: Vec<SyncedPackageRow>,
    snapshot: AuthenticatedSnapshotIdentity,
    authenticated_objects: Vec<SourceMetadataObjectV1>,
) -> Result<SourceCatalogCandidateV1> {
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

    let origin = CatalogPackageOriginV1::Source {
        source_identity: policy.source_identity.clone(),
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
            source_profile: source_profile.clone(),
            source_identity: policy.source_identity.clone(),
            repository_identity: repository_identity.clone(),
        },
        source_evidence,
        packages,
    )?;
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
    SourceCatalogCandidateV1::new(
        SourceCatalogAuthorityV1 {
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
        },
        content,
    )
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

#[cfg(test)]
mod tests;
