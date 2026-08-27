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
    ProfileRevisionV2, SourceSnapshotV1, publish_profile_catalog_bundle,
    publish_source_catalog_bundle, write_profile_catalog_candidate_with_scratch_admission,
    write_profile_catalog_manifest, write_source_catalog_manifest,
};
use conary_core::repository::supported_profiles::ProfileSourceRole;
use futures::StreamExt;

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

/// Complete verified filesystem candidate before immutable publication begins.
pub struct StagedProfileCatalog {
    pub profile: String,
    pub manifest: ProfileRevisionV2,
    pub path: PathBuf,
    pub sources: Vec<StagedSourceCatalog>,
    pub candidate_run_dir: PathBuf,
}

/// Complete durable filesystem result, still inactive in operational SQLite.
pub struct PublishedProfileCatalog {
    pub profile: String,
    pub manifest: ProfileRevisionV2,
    pub path: PathBuf,
    pub sources: Vec<PublishedSourceCatalog>,
    pub candidate_run_dir: PathBuf,
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

/// Fetch every required native source, then bind and verify all source/profile
/// candidates without exposing any immutable publication path.
pub async fn stage_profile_catalog(
    run_id: &str,
    profile: &str,
    repositories: Vec<Repository>,
    keyring_dir: &Path,
    catalog_candidate_root: &Path,
    projection_cache_root: &Path,
    scratch_admission: Arc<dyn CatalogScratchAdmission>,
) -> Result<StagedProfileCatalog> {
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
                let manifest =
                    conary_core::repository::stream_native_source_catalog_with_scratch_admission(
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
                let reader = write_source_catalog_manifest(&candidate_directory, &manifest)?;
                Ok::<_, anyhow::Error>(VerifiedStagedSourceCatalog {
                    staged: StagedSourceCatalog {
                        ordinal: plan.ordinal,
                        role: plan.role,
                        precedence: plan.precedence,
                        required: plan.required,
                        manifest,
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

    let profile = profile.to_string();
    tokio::task::spawn_blocking(move || {
        stage_profile_candidate(&profile, fetched, candidate_run_dir, scratch_admission)
    })
    .await
    .context("profile catalog staging task panicked")?
}

fn stage_profile_candidate(
    profile: &str,
    staged_sources: Vec<VerifiedStagedSourceCatalog>,
    candidate_run_dir: PathBuf,
    scratch_admission: Arc<dyn CatalogScratchAdmission>,
) -> Result<StagedProfileCatalog> {
    let inputs = staged_sources
        .iter()
        .map(|source| ProfileCatalogMemberInputV2 {
            ordinal: source.staged.ordinal,
            role: source.staged.role,
            precedence: source.staged.precedence,
            required: source.staged.required,
            manifest: &source.staged.manifest,
            reader: &source.reader,
        })
        .collect();
    let profile_candidate_directory = candidate_run_dir.join("profile");
    create_private_directory(&profile_candidate_directory, &candidate_run_dir)?;
    let manifest = write_profile_catalog_candidate_with_scratch_admission(
        profile_candidate_directory.join(CATALOG_FILE_NAME),
        profile,
        PROFILE_CATALOG_PROJECTION_VERSION,
        inputs,
        scratch_admission,
    )?;
    manifest.validate_member_contract()?;
    write_profile_catalog_manifest(&profile_candidate_directory, &manifest)?;

    let staged_sources = staged_sources
        .into_iter()
        .map(|source| source.staged)
        .collect();

    Ok(StagedProfileCatalog {
        profile: profile.to_string(),
        manifest,
        path: profile_candidate_directory,
        sources: staged_sources,
        candidate_run_dir,
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
    let mut published_sources = Vec::with_capacity(staged.sources.len());
    for source in staged.sources {
        let path = publish_source_catalog_bundle(&source.path, catalog_root, &source.manifest)?;
        published_sources.push(PublishedSourceCatalog {
            ordinal: source.ordinal,
            role: source.role,
            precedence: source.precedence,
            required: source.required,
            manifest: source.manifest,
            path,
        });
    }
    let path = publish_profile_catalog_bundle(&staged.path, catalog_root, &staged.manifest)?;
    Ok(PublishedProfileCatalog {
        profile: staged.profile,
        manifest: staged.manifest,
        path,
        sources: published_sources,
        candidate_run_dir: staged.candidate_run_dir,
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
