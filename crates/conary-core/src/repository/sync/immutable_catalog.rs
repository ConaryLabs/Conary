// conary-core/src/repository/sync/immutable_catalog.rs

//! Native repository projection into immutable source-catalog candidates.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
};
use crate::error::{Error, Result};
use crate::repository::catalog::source::SourceCatalogAuthorityV1;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCandidateWriter, CatalogContentV1, CatalogMetadataStreamAdmission,
    CatalogMetadataStreamScratchV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogScopeV1, CatalogScratchAdmission,
    CatalogSourceCandidateScratchV1, CatalogSourceEvidenceV1, SourceCatalogCandidateV1,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
    SourceSnapshotV1, SourceStreamKindV1, SourceStreamV1,
};
use crate::repository::parsers::{
    ArchPackageFragmentKind, ArchPackageRecord, AuthenticatedMetadataObject, ChecksumType,
    PackageMetadata, RepositorySnapshotSink, SnapshotPackageIdentity, SnapshotPackageJoin,
    SnapshotProvideUpdate,
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
    projection_cache_root: Option<&Path>,
) -> Result<SourceSnapshotV1> {
    stream_native_source_catalog_inner(
        repo,
        keyring_dir,
        candidate_path,
        projection_cache_root,
        None,
    )
    .await
}

/// Stream one source candidate with typed SQLite finalization admission.
pub async fn stream_native_source_catalog_with_scratch_admission(
    repo: &Repository,
    keyring_dir: &Path,
    candidate_path: &Path,
    projection_cache_root: Option<&Path>,
    scratch_admission: Arc<dyn CatalogScratchAdmission>,
) -> Result<SourceSnapshotV1> {
    stream_native_source_catalog_inner(
        repo,
        keyring_dir,
        candidate_path,
        projection_cache_root,
        Some(scratch_admission),
    )
    .await
}

async fn stream_native_source_catalog_inner(
    repo: &Repository,
    keyring_dir: &Path,
    candidate_path: &Path,
    projection_cache_root: Option<&Path>,
    scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
) -> Result<SourceSnapshotV1> {
    let parser = prepare_repository_native_parser(repo, keyring_dir).await?;
    let mut sink = NativeCatalogSnapshotSink::create(
        repo,
        candidate_path,
        projection_cache_root,
        scratch_admission,
    )?;
    let snapshot = parser.ingest_snapshot(&repo.url, &mut sink).await?;
    sink.finish(repo, snapshot)
}

struct NativeCatalogSnapshotSink {
    writer: Option<CatalogCandidateWriter>,
    scope: CatalogScopeV1,
    preflight_projection_bytes: u64,
    preflight_package_count: u64,
    repository_id: i64,
    source_profile: String,
    repository_url: String,
    content_url: Option<String>,
    origin: CatalogPackageOriginV1,
    authenticated_objects: BTreeMap<SourceMetadataObjectRoleV1, SourceMetadataObjectV1>,
    work_directory: tempfile::TempDir,
    candidate_path: std::path::PathBuf,
    projection_cache: Option<super::projection_cache::ProjectionCache>,
    cache_inputs: Option<(
        AuthenticatedSnapshotIdentity,
        Vec<AuthenticatedMetadataObject>,
    )>,
    cache_hit: bool,
    work_leases: Vec<Box<dyn Send>>,
    scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
}

impl NativeCatalogSnapshotSink {
    fn create(
        repo: &Repository,
        candidate_path: &Path,
        projection_cache_root: Option<&Path>,
        scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
    ) -> Result<Self> {
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
        let candidate_parent = candidate_path.parent().ok_or_else(|| {
            Error::InvalidPath(format!(
                "source catalog candidate {} has no parent",
                candidate_path.display()
            ))
        })?;
        let work_directory = tempfile::Builder::new()
            .prefix("native-objects-")
            .tempdir_in(candidate_parent)?;
        let projection_cache = projection_cache_root
            .map(|root| match scratch_admission.as_ref() {
                Some(admission) => {
                    super::projection_cache::ProjectionCache::open_with_scratch_admission(
                        root,
                        repo.stream_binding_sha256.as_deref().expect("validated"),
                        Arc::clone(admission),
                    )
                }
                None => super::projection_cache::ProjectionCache::open(
                    root,
                    repo.stream_binding_sha256.as_deref().expect("validated"),
                ),
            })
            .transpose()?;
        let writer = scratch_admission
            .is_none()
            .then(|| CatalogCandidateWriter::create(candidate_path, scope.clone()))
            .transpose()?;
        let preflight_projection_bytes = u64::try_from(
            crate::json::canonical_json(&scope)
                .map_err(|error| {
                    Error::ParseError(format!("serialize native source scope: {error}"))
                })?
                .len(),
        )
        .map_err(|_| Error::IoError("native source scope byte count exceeds u64".to_string()))?;
        Ok(Self {
            writer,
            scope,
            preflight_projection_bytes,
            preflight_package_count: 0,
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
            work_directory,
            candidate_path: candidate_path.to_path_buf(),
            projection_cache,
            cache_inputs: None,
            cache_hit: false,
            work_leases: Vec::new(),
            scratch_admission,
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
            candidate_path,
            projection_cache,
            cache_inputs,
            cache_hit,
            work_directory,
            work_leases,
            ..
        } = self;
        drop(work_directory);
        drop(work_leases);
        let writer = writer.ok_or_else(|| {
            Error::InternalError(
                "native source parser completed without beginning its admitted candidate"
                    .to_string(),
            )
        })?;
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
        if !cache_hit
            && let (Some(cache), Some((cache_snapshot, cache_objects))) =
                (projection_cache, cache_inputs)
        {
            cache.publish(&cache_snapshot, &cache_objects, &binding, &candidate_path)?;
        }
        let authority = source_catalog_authority(repo, snapshot, authenticated_objects)?;
        crate::repository::catalog::source::bind_source_snapshot(authority, &binding)
    }

    fn project_package(&self, package: PackageMetadata) -> Result<CatalogPackageRecordV1> {
        let row = super::synced_package_row(
            self.repository_id,
            Some(&self.source_profile),
            &self.repository_url,
            self.content_url.as_deref(),
            package,
        );
        let mut record = CatalogPackageRecordV1::from_repository_projection(
            row.package,
            row.provides,
            row.requirement_groups,
            row.requirement_group_clauses,
            self.origin.clone(),
        )?;
        record.canonicalize_for_scope(&self.scope)?;
        Ok(record)
    }

    fn observe_projection<T: serde::Serialize>(&mut self, value: &T) -> Result<()> {
        let bytes = crate::json::canonical_json(value).map_err(|error| {
            Error::ParseError(format!("serialize native source preflight fact: {error}"))
        })?;
        self.preflight_projection_bytes = self
            .preflight_projection_bytes
            .checked_add(u64::try_from(bytes.len()).map_err(|_| {
                Error::IoError("native source preflight byte count exceeds u64".to_string())
            })?)
            .ok_or_else(|| {
                Error::IoError("native source preflight byte count overflow".to_string())
            })?;
        Ok(())
    }

    fn writer_mut(&mut self) -> Result<&mut CatalogCandidateWriter> {
        self.writer.as_mut().ok_or_else(|| {
            Error::InternalError(
                "native source candidate mutation preceded scratch admission".to_string(),
            )
        })
    }
}

impl RepositorySnapshotSink for NativeCatalogSnapshotSink {
    fn work_directory(&self) -> &Path {
        self.work_directory.path()
    }

    fn reserve_authenticated_metadata(
        &mut self,
        requirement: crate::repository::catalog::CatalogMetadataScratchV1,
    ) -> Result<()> {
        requirement.validate()?;
        if let Some(admission) = &self.scratch_admission {
            let lease = admission.reserve_metadata(self.work_directory.path(), requirement)?;
            self.work_leases.push(lease);
        }
        Ok(())
    }

    fn streamed_authenticated_metadata(
        &mut self,
        requirement: CatalogMetadataStreamScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        requirement.validate()?;
        match &self.scratch_admission {
            Some(admission) => admission.stream_metadata(self.work_directory.path(), requirement),
            None => crate::repository::parsers::validation_only_metadata_stream(requirement),
        }
    }

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

    fn reuse_cached_projection(
        &mut self,
        snapshot: &AuthenticatedSnapshotIdentity,
        objects: &[AuthenticatedMetadataObject],
    ) -> Result<bool> {
        self.cache_inputs = Some((snapshot.clone(), objects.to_vec()));
        let Some(cache) = &self.projection_cache else {
            return Ok(false);
        };
        let Some(reader) = cache.lookup(snapshot, objects)? else {
            return Ok(false);
        };
        if let Some(admission) = &self.scratch_admission {
            let requirement = CatalogSourceCandidateScratchV1::from_cached_catalog(
                reader.binding().artifact.size,
                reader.binding().counts.packages,
            )?;
            let admission = Arc::clone(admission);
            self.writer = Some(
                CatalogCandidateWriter::create_with_source_scratch_admission(
                    &self.candidate_path,
                    self.scope.clone(),
                    admission,
                    requirement,
                )?,
            );
        }
        self.writer_mut()?.copy_source_catalog(&reader)?;
        for object in objects.iter().cloned() {
            self.authenticated_object(object)?;
        }
        self.cache_hit = true;
        Ok(true)
    }

    fn requires_source_candidate_preflight(&self) -> bool {
        self.scratch_admission.is_some()
    }

    fn preflight_package(&mut self, package: PackageMetadata) -> Result<()> {
        if !self.requires_source_candidate_preflight() {
            return Ok(());
        }
        let record = self.project_package(package)?;
        self.observe_projection(&record)?;
        self.preflight_package_count = self
            .preflight_package_count
            .checked_add(1)
            .ok_or_else(|| Error::IoError("native source package count overflow".to_string()))?;
        Ok(())
    }

    fn preflight_package_provides(
        &mut self,
        provides: Vec<crate::repository::dependency_model::RepositoryProvide>,
    ) -> Result<()> {
        if !self.requires_source_candidate_preflight() {
            return Ok(());
        }
        for provide in provides {
            let normalized = super::native::normalized_repository_provide(
                &provide,
                crate::repository::versioning::VersionScheme::Rpm,
            );
            self.observe_projection(&CatalogProvideRecordV1::from(normalized))?;
        }
        Ok(())
    }

    fn preflight_requirement_groups(
        &mut self,
        groups: Vec<crate::repository::dependency_model::RepositoryRequirementGroup>,
    ) -> Result<()> {
        if !self.requires_source_candidate_preflight() || groups.is_empty() {
            return Ok(());
        }
        let package = PackageMetadata {
            name: "conary-preflight-requirements".to_string(),
            version: "0".to_string(),
            architecture: None,
            debian_multi_arch: None,
            description: None,
            checksum: "0".repeat(64),
            checksum_type: ChecksumType::Sha256,
            size: 0,
            download_url: "preflight.pkg.tar.zst".to_string(),
            extra_metadata: serde_json::Value::Null,
            dependency_flavor:
                crate::repository::dependency_source::RepositoryDependencyFlavor::Arch,
            version_scheme: crate::repository::versioning::VersionScheme::Arch,
            requirements: groups,
            provides: Vec::new(),
        };
        let record = self.project_package(package)?;
        self.observe_projection(&record)
    }

    fn preflight_arch_package_fragment(
        &mut self,
        directory: &str,
        kind: ArchPackageFragmentKind,
        content: &str,
    ) -> Result<()> {
        if !self.requires_source_candidate_preflight() {
            return Ok(());
        }
        let kind = match kind {
            ArchPackageFragmentKind::Desc => "desc",
            ArchPackageFragmentKind::Depends => "depends",
        };
        self.observe_projection(&(directory, kind, content))
    }

    fn begin_source_candidate(&mut self) -> Result<()> {
        if !self.requires_source_candidate_preflight() {
            return Ok(());
        }
        if self.writer.is_some() {
            return Err(Error::ConflictError(
                "native source candidate was begun more than once".to_string(),
            ));
        }
        let requirement = CatalogSourceCandidateScratchV1::from_projection_facts(
            self.preflight_projection_bytes,
            self.preflight_package_count,
        )?;
        let admission = Arc::clone(self.scratch_admission.as_ref().expect("checked"));
        self.writer = Some(
            CatalogCandidateWriter::create_with_source_scratch_admission(
                &self.candidate_path,
                self.scope.clone(),
                admission,
                requirement,
            )?,
        );
        Ok(())
    }

    fn package(&mut self, package: PackageMetadata) -> Result<()> {
        let record = self.project_package(package)?;
        self.writer_mut()?.package(record)
    }

    fn stage_arch_package_fragment(
        &mut self,
        directory: String,
        kind: ArchPackageFragmentKind,
        content: String,
    ) -> Result<()> {
        self.writer_mut()?
            .stage_arch_package_fragment(directory, kind, content)
    }

    fn take_arch_package_record(&mut self) -> Result<Option<ArchPackageRecord>> {
        self.writer_mut()?.take_arch_package_record()
    }

    fn extend_package_provides(
        &mut self,
        join: SnapshotPackageJoin,
        identity: &SnapshotPackageIdentity,
        provides: Vec<crate::repository::dependency_model::RepositoryProvide>,
    ) -> Result<SnapshotProvideUpdate> {
        if self.requires_source_candidate_preflight() && self.writer.is_none() {
            let added = provides.len();
            self.preflight_package_provides(provides)?;
            return Ok(SnapshotProvideUpdate {
                matched_packages: 1,
                added,
                already_known: 0,
            });
        }
        let checksum = match identity.checksum_type {
            ChecksumType::Sha1 => format!("sha1:{}", identity.checksum.trim_start_matches("sha1:")),
            ChecksumType::Sha256 => {
                format!("sha256:{}", identity.checksum.trim_start_matches("sha256:"))
            }
            ChecksumType::Sha512 | ChecksumType::Md5 => identity.checksum.clone(),
        };
        let provides = provides
            .iter()
            .map(|provide| {
                super::native::normalized_repository_provide(
                    provide,
                    crate::repository::versioning::VersionScheme::Rpm,
                )
                .into()
            })
            .collect();
        let result = self.writer_mut()?.extend_package_provides(
            join.as_str(),
            &checksum,
            &identity.name,
            &identity.version,
            identity.architecture.as_deref(),
            provides,
        )?;
        Ok(SnapshotProvideUpdate {
            matched_packages: result.matched_packages,
            added: result.added,
            already_known: result.already_known,
        })
    }

    fn finish_package_join(&mut self, join: SnapshotPackageJoin) -> Result<()> {
        if self.requires_source_candidate_preflight() && self.writer.is_none() {
            return Ok(());
        }
        self.writer_mut()?.finish_package_join(join.as_str())
    }

    fn validate_rpm_primary_file_requirements(&mut self, repo_url: &str) -> Result<()> {
        if self.requires_source_candidate_preflight() && self.writer.is_none() {
            return Ok(());
        }
        self.writer_mut()?
            .validate_rpm_primary_file_requirements(repo_url)
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
