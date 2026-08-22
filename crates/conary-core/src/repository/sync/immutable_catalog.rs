// conary-core/src/repository/sync/immutable_catalog.rs

//! Native repository projection into immutable source-catalog candidates.

use std::collections::BTreeMap;
use std::path::Path;

use crate::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
};
use crate::error::{Error, Result};
use crate::repository::catalog::source::SourceCatalogAuthorityV1;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCandidateWriter, CatalogContentV1, CatalogPackageOriginV1,
    CatalogPackageRecordV1, CatalogScopeV1, CatalogSourceEvidenceV1, SourceCatalogCandidateV1,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
    SourceSnapshotV1, SourceStreamKindV1, SourceStreamV1,
};
use crate::repository::parsers::{
    AuthenticatedMetadataObject, PackageMetadata, RepositorySnapshotSink,
};

use super::types::{RepositorySyncSnapshot, SyncedPackageRow};
use super::{fetch_repository_native_snapshot, prepare_repository_native_parser};

/// Compatibility construction for local callers that still replace mutable
/// repository rows in one transaction.
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

/// Stream one authenticated native source directly into a private standalone
/// catalog candidate and return its exact immutable manifest.
pub async fn stream_native_source_catalog(
    repo: &Repository,
    keyring_dir: &Path,
    candidate_path: &Path,
) -> Result<SourceSnapshotV1> {
    let parser = prepare_repository_native_parser(repo, keyring_dir).await?;
    let mut sink = NativeCatalogSnapshotSink::create(repo, candidate_path)?;
    let snapshot = parser.ingest_snapshot(&repo.url, &mut sink).await?;
    sink.finish(repo, snapshot)
}

struct NativeCatalogSnapshotSink {
    writer: CatalogCandidateWriter,
    repository_id: i64,
    source_profile: String,
    repository_url: String,
    content_url: Option<String>,
    origin: CatalogPackageOriginV1,
    authenticated_objects: BTreeMap<SourceMetadataObjectRoleV1, SourceMetadataObjectV1>,
}

impl NativeCatalogSnapshotSink {
    fn create(repo: &Repository, candidate_path: &Path) -> Result<Self> {
        repo.validate_stream_binding()?;
        let source_profile = repo.source_profile.clone().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no exact source profile for immutable catalog publication",
                repo.name
            ))
        })?;
        let source_identity = repo.require_source_policy()?.source_identity.clone();
        let repository_identity = repo.repository_identity.clone().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no exact repository identity",
                repo.name
            ))
        })?;
        let scope = CatalogScopeV1::Source {
            source_profile: source_profile.clone(),
            source_identity: source_identity.clone(),
            repository_identity: repository_identity.clone(),
        };
        Ok(Self {
            writer: CatalogCandidateWriter::create(candidate_path, scope)?,
            repository_id: repo
                .id
                .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?,
            source_profile,
            repository_url: repo.url.clone(),
            content_url: repo.content_url.clone(),
            origin: CatalogPackageOriginV1::Source {
                source_identity,
                repository_identity,
            },
            authenticated_objects: BTreeMap::new(),
        })
    }

    fn finish(
        self,
        repo: &Repository,
        snapshot: AuthenticatedSnapshotIdentity,
    ) -> Result<SourceSnapshotV1> {
        let NativeCatalogSnapshotSink {
            writer,
            authenticated_objects,
            ..
        } = self;
        let authenticated_objects = authenticated_objects.into_values().collect::<Vec<_>>();
        let evidence = authenticated_objects
            .iter()
            .map(|object| CatalogSourceEvidenceV1::AuthenticatedObject {
                role: object.role.clone(),
                source_path: object.source_path.clone(),
                sha256: object.sha256.clone(),
                size: object.size,
            })
            .collect();
        let binding = writer.finish(evidence)?;
        let authority = source_catalog_authority(repo, snapshot, authenticated_objects)?;
        crate::repository::catalog::source::bind_source_snapshot(authority, &binding)
    }
}

impl RepositorySnapshotSink for NativeCatalogSnapshotSink {
    fn authenticated_object(&mut self, object: AuthenticatedMetadataObject) -> Result<()> {
        if self
            .authenticated_objects
            .insert(object.role.clone(), object)
            .is_some()
        {
            return Err(Error::ConflictError(
                "repository snapshot repeats an authenticated metadata object role".to_string(),
            ));
        }
        Ok(())
    }

    fn package(&mut self, package: PackageMetadata) -> Result<()> {
        let row = super::synced_package_row(
            self.repository_id,
            Some(&self.source_profile),
            &self.repository_url,
            self.content_url.as_deref(),
            package,
        );
        let record = CatalogPackageRecordV1::from_repository_projection(
            row.package,
            row.provides,
            row.requirement_groups,
            row.requirement_group_clauses,
            self.origin.clone(),
        )?;
        self.writer.package(record)
    }
}

fn source_catalog_candidate(
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

fn source_catalog_authority(
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

#[cfg(test)]
mod tests;
