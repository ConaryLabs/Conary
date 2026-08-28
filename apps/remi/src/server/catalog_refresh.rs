// apps/remi/src/server/catalog_refresh.rs

//! Private construction and durable publication of one immutable profile catalog.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use conary_core::db::models::Repository;
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, CatalogReader, CatalogScratchAdmission, ProfileCatalogMemberInputV2,
    ProfileRevisionV2, ProfileSourceMemberV2, SourceSnapshotV1, derive_profile_catalog_members,
    publish_profile_catalog_bundle_verified, publish_source_catalog_bundle_verified,
    write_profile_catalog_candidate_verified_with_scratch_admission,
    write_profile_catalog_manifest_verified, write_source_catalog_manifest_verified,
};
use conary_core::repository::supported_profiles::ProfileSourceRole;
use futures::StreamExt;

use super::catalog_authority::PinnedProfileCatalog;

pub(crate) const PROFILE_CATALOG_PROJECTION_VERSION: u32 = 2;
const MAX_CONCURRENT_SOURCE_FETCHES: usize = 4;

/// One exact source member and its explicitly assigned profile ordinal.
#[derive(Clone)]
pub struct ProfileSourcePlan {
    pub ordinal: u32,
    pub role: ProfileSourceRole,
    pub precedence: i32,
    pub required: bool,
    pub repository: Repository,
}

/// One source bundle published before its containing profile can activate.
pub struct PublishedSourceCatalog {
    pub ordinal: u32,
    pub role: ProfileSourceRole,
    pub precedence: i32,
    pub required: bool,
    pub manifest: SourceSnapshotV1,
    pub path: PathBuf,
}

/// One fully bound and verified source bundle still private to its fenced run.
pub struct StagedSourceCatalog {
    pub ordinal: u32,
    pub role: ProfileSourceRole,
    pub precedence: i32,
    pub required: bool,
    pub manifest: SourceSnapshotV1,
    pub path: PathBuf,
}

/// A private source candidate paired with the reader that proved its manifest
/// binding. The reader is consumed by profile composition before the durable
/// publication boundary independently verifies the complete bundle again.
struct VerifiedStagedSourceCatalog {
    staged: StagedSourceCatalog,
    reader: CatalogReader,
}

/// Verified private source candidates before profile composition or exact
/// immutable-profile reuse is selected.
pub struct StagedProfileSources {
    profile: String,
    members: Vec<ProfileSourceMemberV2>,
    sources: Vec<VerifiedStagedSourceCatalog>,
    candidate_run_dir: PathBuf,
    scratch_admission: Arc<dyn CatalogScratchAdmission>,
}

enum StagedProfileArtifact {
    Candidate {
        directory: PathBuf,
        verification: Box<CatalogReader>,
    },
    Reused(Box<PinnedProfileCatalog>),
}

/// Complete verified filesystem candidate before immutable publication begins.
pub struct StagedProfileCatalog {
    pub profile: String,
    pub manifest: ProfileRevisionV2,
    pub sources: Vec<StagedSourceCatalog>,
    pub candidate_run_dir: PathBuf,
    source_verifications: Vec<CatalogReader>,
    artifact: StagedProfileArtifact,
}

/// Complete durable filesystem result, still inactive in operational SQLite.
pub struct PublishedProfileCatalog {
    pub profile: String,
    pub manifest: ProfileRevisionV2,
    pub path: PathBuf,
    pub sources: Vec<PublishedSourceCatalog>,
    pub candidate_run_dir: PathBuf,
    _reused_profile_pin: Option<Box<PinnedProfileCatalog>>,
}

/// Assign canonical member ordinals without using fetch completion or mutable
/// repository display names as profile authority.
pub fn plan_profile_sources(
    profile: &str,
    mut repositories: Vec<Repository>,
) -> Result<Vec<ProfileSourcePlan>> {
    if repositories.is_empty() {
        bail!("profile '{profile}' has no enabled source repositories");
    }
    let profile_contract = conary_core::repository::supported_profiles::profile_by_id(profile)
        .with_context(|| format!("profile '{profile}' has no typed support contract"))?;
    let expected_members = profile_contract
        .members()
        .iter()
        .map(|member| (member.repository_identity.as_str(), member))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut actual_members = BTreeSet::new();
    for repository in &repositories {
        if !repository.enabled {
            bail!(
                "profile '{}' repository '{}' is disabled",
                profile,
                repository.name
            );
        }
        if repository.source_profile.as_deref() != Some(profile) {
            bail!(
                "profile '{}' cannot plan repository '{}' from profile {:?}",
                profile,
                repository.name,
                repository.source_profile
            );
        }
        repository.validate_stream_binding().with_context(|| {
            format!(
                "repository '{}' lacks exact stream authority",
                repository.name
            )
        })?;
        let repository_identity = repository.repository_identity.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "profile '{}' repository '{}' has no exact repository identity",
                profile,
                repository.name
            )
        })?;
        let expected = expected_members.get(repository_identity).ok_or_else(|| {
            anyhow::anyhow!(
                "repository '{}' is not a declared member of profile '{}'",
                repository_identity,
                profile
            )
        })?;
        let role = repository.profile_member_role.ok_or_else(|| {
            anyhow::anyhow!(
                "profile '{}' repository '{}' has no typed member role",
                profile,
                repository.name
            )
        })?;
        if role != expected.role || repository.priority != expected.precedence {
            bail!(
                "profile '{}' repository '{}' has role '{}' and precedence {}, expected role \
                 '{}' and precedence {}",
                profile,
                repository_identity,
                role.as_str(),
                repository.priority,
                expected.role.as_str(),
                expected.precedence
            );
        }
        if !repository.profile_member_required {
            bail!(
                "profile '{}' repository '{}' must be required",
                profile,
                repository_identity
            );
        }
        if !actual_members.insert(repository_identity) {
            bail!(
                "profile '{}' repeats repository identity '{}'",
                profile,
                repository_identity
            );
        }
    }
    let expected_identities = expected_members.keys().copied().collect::<BTreeSet<_>>();
    if actual_members != expected_identities {
        let missing = expected_identities
            .difference(&actual_members)
            .copied()
            .collect::<Vec<_>>();
        let unexpected = actual_members
            .difference(&expected_identities)
            .copied()
            .collect::<Vec<_>>();
        bail!(
            "profile '{profile}' source membership is incomplete: missing {:?}, unexpected {:?}",
            missing,
            unexpected
        );
    }
    repositories.sort_by(|left, right| {
        right.priority.cmp(&left.priority).then_with(|| {
            left.repository_identity
                .as_deref()
                .cmp(&right.repository_identity.as_deref())
        })
    });
    let mut repository_identities = BTreeSet::new();
    repositories
        .into_iter()
        .enumerate()
        .map(|(ordinal, repository)| {
            let repository_identity =
                repository.repository_identity.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "repository '{}' has no exact repository identity",
                        repository.name
                    )
                })?;
            if !repository_identities.insert(repository_identity.to_string()) {
                bail!(
                    "profile '{}' repeats repository identity '{}'",
                    profile,
                    repository_identity
                );
            }
            Ok(ProfileSourcePlan {
                ordinal: u32::try_from(ordinal)
                    .context("profile source count exceeds member ordinal range")?,
                role: repository.profile_member_role.expect("validated"),
                precedence: repository.priority,
                required: repository.profile_member_required,
                repository,
            })
        })
        .collect()
}

/// Fetch every required native source and derive the exact ordered profile
/// member contract without visiting package rows.
pub async fn stage_profile_sources(
    run_id: &str,
    profile: &str,
    repositories: Vec<Repository>,
    keyring_dir: &Path,
    catalog_candidate_root: &Path,
    projection_cache_root: &Path,
    scratch_admission: Arc<dyn CatalogScratchAdmission>,
) -> Result<StagedProfileSources> {
    let plans = plan_profile_sources(profile, repositories)?;
    let candidate_run_dir = create_candidate_run_dir(catalog_candidate_root, run_id)?;
    let planned_sources = plans
        .into_iter()
        .map(|plan| {
            let candidate_directory = candidate_run_dir.join(format!("source-{:08}", plan.ordinal));
            create_private_directory(&candidate_directory, &candidate_run_dir)?;
            Ok((plan, candidate_directory))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut fetches = futures::stream::iter(planned_sources.into_iter().map(
        |(plan, candidate_directory)| {
            let keyring_dir = keyring_dir.to_path_buf();
            let projection_cache_root = projection_cache_root.to_path_buf();
            let scratch_admission = Arc::clone(&scratch_admission);
            async move {
                let candidate = conary_core::repository::stream_native_source_catalog_verified_with_scratch_admission(
                        &plan.repository,
                        &keyring_dir,
                        &candidate_directory.join(CATALOG_FILE_NAME),
                        Some(&projection_cache_root),
                        scratch_admission,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "fetch authenticated catalog source '{}'",
                            plan.repository.name
                        )
                    })?;
                let reader = write_source_catalog_manifest_verified(
                    &candidate_directory,
                    &candidate.manifest,
                    &candidate.reader,
                )?;
                Ok::<_, anyhow::Error>(VerifiedStagedSourceCatalog {
                    staged: StagedSourceCatalog {
                        ordinal: plan.ordinal,
                        role: plan.role,
                        precedence: plan.precedence,
                        required: plan.required,
                        manifest: candidate.manifest,
                        path: candidate_directory,
                    },
                    reader,
                })
            }
        },
    ))
    .buffer_unordered(MAX_CONCURRENT_SOURCE_FETCHES);
    let mut fetched = Vec::new();
    while let Some(result) = fetches.next().await {
        fetched.push(result?);
    }
    drop(fetches);
    fetched.sort_by_key(|source| source.staged.ordinal);

    let members = derive_profile_catalog_members(
        profile,
        PROFILE_CATALOG_PROJECTION_VERSION,
        profile_member_inputs(&fetched),
    )?;
    Ok(StagedProfileSources {
        profile: profile.to_string(),
        members,
        sources: fetched,
        candidate_run_dir,
        scratch_admission,
    })
}

impl StagedProfileSources {
    /// The exact ordered identity contract used to select immutable reuse.
    #[must_use]
    pub fn members(&self) -> &[ProfileSourceMemberV2] {
        &self.members
    }
}

/// Either bind a fully reopened immutable profile with the exact same member
/// contract or compose a new private profile candidate.
pub async fn finish_staged_profile_catalog(
    staged: StagedProfileSources,
    reusable: Option<PinnedProfileCatalog>,
) -> Result<StagedProfileCatalog> {
    tokio::task::spawn_blocking(move || stage_profile_candidate(staged, reusable))
        .await
        .context("profile catalog staging task panicked")?
}

fn profile_member_inputs(
    sources: &[VerifiedStagedSourceCatalog],
) -> Vec<ProfileCatalogMemberInputV2<'_>> {
    sources
        .iter()
        .map(|source| ProfileCatalogMemberInputV2 {
            ordinal: source.staged.ordinal,
            role: source.staged.role,
            precedence: source.staged.precedence,
            required: source.staged.required,
            manifest: &source.staged.manifest,
            reader: &source.reader,
        })
        .collect()
}

/// Require every identity-bearing profile input to match before immutable
/// bytes can be reused. Catalog content is verified by reopening the selected
/// bundle, not by this bounded manifest comparison.
pub fn profile_revision_matches_contract(
    manifest: &ProfileRevisionV2,
    profile: &str,
    members: &[ProfileSourceMemberV2],
) -> bool {
    manifest.profile == profile
        && manifest.projection_version == PROFILE_CATALOG_PROJECTION_VERSION
        && manifest.members == members
        && manifest.validate_member_contract().is_ok()
}

fn stage_profile_candidate(
    staged: StagedProfileSources,
    reusable: Option<PinnedProfileCatalog>,
) -> Result<StagedProfileCatalog> {
    let StagedProfileSources {
        profile,
        members,
        sources,
        candidate_run_dir,
        scratch_admission,
    } = staged;
    let (manifest, artifact) = match reusable {
        Some(reusable) => {
            let manifest = reusable.manifest().clone();
            if !profile_revision_matches_contract(&manifest, &profile, &members) {
                bail!(
                    "reusable profile '{}' revision {} does not match its exact staged member contract",
                    profile,
                    reusable.profile_revision_sha256()
                );
            }
            (manifest, StagedProfileArtifact::Reused(Box::new(reusable)))
        }
        None => {
            let profile_candidate_directory = candidate_run_dir.join("profile");
            create_private_directory(&profile_candidate_directory, &candidate_run_dir)?;
            let candidate = write_profile_catalog_candidate_verified_with_scratch_admission(
                profile_candidate_directory.join(CATALOG_FILE_NAME),
                &profile,
                PROFILE_CATALOG_PROJECTION_VERSION,
                profile_member_inputs(&sources),
                scratch_admission,
            )?;
            let manifest = candidate.manifest;
            manifest.validate_member_contract()?;
            let verification = write_profile_catalog_manifest_verified(
                &profile_candidate_directory,
                &manifest,
                &candidate.reader,
            )?;
            (
                manifest,
                StagedProfileArtifact::Candidate {
                    directory: profile_candidate_directory,
                    verification: Box::new(verification),
                },
            )
        }
    };

    let mut staged_sources = Vec::with_capacity(sources.len());
    let mut source_verifications = Vec::with_capacity(sources.len());
    for source in sources {
        staged_sources.push(source.staged);
        source_verifications.push(source.reader);
    }

    Ok(StagedProfileCatalog {
        profile,
        manifest,
        sources: staged_sources,
        candidate_run_dir,
        source_verifications,
        artifact,
    })
}

/// Publish one completely staged profile only after its exact candidate
/// identities have been durably journaled by the fenced run.
pub async fn publish_staged_profile_catalog(
    staged: StagedProfileCatalog,
    catalog_root: &Path,
) -> Result<PublishedProfileCatalog> {
    let catalog_root = catalog_root.to_path_buf();
    tokio::task::spawn_blocking(move || publish_staged_profile(staged, &catalog_root))
        .await
        .context("profile catalog publication task panicked")?
}

fn publish_staged_profile(
    staged: StagedProfileCatalog,
    catalog_root: &Path,
) -> Result<PublishedProfileCatalog> {
    require_real_directory(catalog_root, "immutable catalog root")?;
    if staged.sources.len() != staged.source_verifications.len() {
        bail!("staged source verification count changed before publication");
    }
    let mut published_sources = Vec::with_capacity(staged.sources.len());
    for (source, verification) in staged.sources.into_iter().zip(staged.source_verifications) {
        let path = publish_source_catalog_bundle_verified(
            &source.path,
            catalog_root,
            &source.manifest,
            &verification,
        )?;
        published_sources.push(PublishedSourceCatalog {
            ordinal: source.ordinal,
            role: source.role,
            precedence: source.precedence,
            required: source.required,
            manifest: source.manifest,
            path,
        });
    }
    let (path, reused_profile_pin) = match staged.artifact {
        StagedProfileArtifact::Candidate {
            directory,
            verification,
        } => (
            publish_profile_catalog_bundle_verified(
                &directory,
                catalog_root,
                &staged.manifest,
                &verification,
            )?,
            None,
        ),
        StagedProfileArtifact::Reused(reused) => {
            if reused.manifest() != &staged.manifest {
                bail!("reused profile manifest changed before publication");
            }
            let catalog_path = reused.catalog_path();
            if catalog_path.file_name().and_then(|name| name.to_str()) != Some(CATALOG_FILE_NAME) {
                bail!(
                    "reused profile catalog path {} has an unexpected file name",
                    catalog_path.display()
                );
            }
            let bundle = catalog_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("reused profile catalog has no bundle parent"))?
                .to_path_buf();
            (bundle, Some(reused))
        }
    };
    Ok(PublishedProfileCatalog {
        profile: staged.profile,
        manifest: staged.manifest,
        path,
        sources: published_sources,
        candidate_run_dir: staged.candidate_run_dir,
        _reused_profile_pin: reused_profile_pin,
    })
}

fn create_candidate_run_dir(root: &Path, run_id: &str) -> Result<PathBuf> {
    validate_run_id(run_id)?;
    require_real_directory(root, "catalog candidate root")?;
    let path = root.join(run_id);
    create_private_directory(&path, root)?;
    Ok(path)
}

/// Remove only one exact private candidate run after publication or failure.
/// Immutable source/profile destinations are never beneath this path.
pub fn cleanup_candidate_run(root: &Path, run_id: &str) -> Result<()> {
    validate_run_id(run_id)?;
    require_real_directory(root, "catalog candidate root")?;
    let path = root.join(run_id);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!(
            "catalog candidate run {} must be a real directory",
            path.display()
        );
    }
    fs::remove_dir_all(&path)
        .with_context(|| format!("remove exact catalog candidate run {}", path.display()))?;
    File::open(root)?.sync_all()?;
    Ok(())
}

fn validate_run_id(run_id: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(run_id).context("catalog run ID must be a UUID")?;
    if parsed.hyphenated().to_string() != run_id {
        bail!("catalog run ID must use canonical lowercase hyphenated UUID form");
    }
    Ok(())
}

fn create_private_directory(path: &Path, parent: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    require_real_directory(path, "catalog candidate directory")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!("{label} {} must be a real directory", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
