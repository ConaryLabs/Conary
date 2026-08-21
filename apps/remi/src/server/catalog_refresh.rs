// apps/remi/src/server/catalog_refresh.rs

//! Private construction and durable publication of one immutable profile catalog.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::db::models::Repository;
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, ProfileCatalogCandidateV1, ProfileCatalogMemberInputV1, ProfileRevisionV1,
    SourceCatalogCandidateV1, SourceSnapshotV1, publish_profile_catalog_bundle,
    publish_source_catalog_bundle, verify_source_catalog_bundle, write_catalog_candidate,
    write_profile_catalog_manifest, write_source_catalog_manifest,
};
use futures::StreamExt;

const PROFILE_CATALOG_PROJECTION_VERSION: u32 = 1;
const MAX_CONCURRENT_SOURCE_FETCHES: usize = 4;

/// One exact source member and its explicitly assigned profile ordinal.
pub struct ProfileSourcePlan {
    pub ordinal: u32,
    pub priority: i32,
    pub required: bool,
    pub repository: Repository,
}

/// One source bundle published before its containing profile can activate.
pub struct PublishedSourceCatalog {
    pub ordinal: u32,
    pub priority: i32,
    pub required: bool,
    pub manifest: SourceSnapshotV1,
    pub path: PathBuf,
}

/// Complete durable filesystem result, still inactive in operational SQLite.
pub struct PublishedProfileCatalog {
    pub profile: String,
    pub manifest: ProfileRevisionV1,
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
    for repository in &repositories {
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
                priority: repository.priority,
                required: true,
                repository,
            })
        })
        .collect()
}

/// Fetch every required native source, then build and durably publish exact
/// source bundles and their deterministic containing profile bundle.
pub async fn build_profile_catalog(
    run_id: &str,
    profile: &str,
    repositories: Vec<Repository>,
    keyring_dir: &Path,
    catalog_candidate_root: &Path,
    catalog_root: &Path,
) -> Result<PublishedProfileCatalog> {
    let plans = plan_profile_sources(profile, repositories)?;
    let mut fetches = futures::stream::iter(plans.into_iter().map(|plan| {
        let keyring_dir = keyring_dir.to_path_buf();
        async move {
            let candidate = conary_core::repository::fetch_native_source_catalog(
                &plan.repository,
                &keyring_dir,
            )
            .await
            .with_context(|| {
                format!(
                    "fetch authenticated catalog source '{}'",
                    plan.repository.name
                )
            })?;
            Ok::<_, anyhow::Error>((plan, candidate))
        }
    }))
    .buffer_unordered(MAX_CONCURRENT_SOURCE_FETCHES);
    let mut fetched = Vec::new();
    while let Some(result) = fetches.next().await {
        fetched.push(result?);
    }
    fetched.sort_by_key(|(plan, _)| plan.ordinal);

    let run_id = run_id.to_string();
    let profile = profile.to_string();
    let catalog_candidate_root = catalog_candidate_root.to_path_buf();
    let catalog_root = catalog_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        publish_profile_candidates(
            &run_id,
            &profile,
            fetched,
            &catalog_candidate_root,
            &catalog_root,
        )
    })
    .await
    .context("profile catalog publication task panicked")?
}

fn publish_profile_candidates(
    run_id: &str,
    profile: &str,
    fetched: Vec<(ProfileSourcePlan, SourceCatalogCandidateV1)>,
    catalog_candidate_root: &Path,
    catalog_root: &Path,
) -> Result<PublishedProfileCatalog> {
    let candidate_run_dir = create_candidate_run_dir(catalog_candidate_root, run_id)?;
    require_real_directory(catalog_root, "immutable catalog root")?;
    let mut published_sources = Vec::with_capacity(fetched.len());

    for (plan, candidate) in fetched {
        let candidate_directory = candidate_run_dir.join(format!("source-{:08}", plan.ordinal));
        create_private_directory(&candidate_directory, &candidate_run_dir)?;
        let binding = write_catalog_candidate(
            candidate_directory.join(CATALOG_FILE_NAME),
            candidate.content(),
        )
        .with_context(|| {
            format!(
                "write source catalog candidate for '{}'",
                plan.repository.name
            )
        })?;
        let manifest = candidate.bind(&binding).with_context(|| {
            format!(
                "bind source catalog candidate for '{}'",
                plan.repository.name
            )
        })?;
        write_source_catalog_manifest(&candidate_directory, &manifest)?;
        let path = publish_source_catalog_bundle(&candidate_directory, catalog_root, &manifest)?;
        published_sources.push(PublishedSourceCatalog {
            ordinal: plan.ordinal,
            priority: plan.priority,
            required: plan.required,
            manifest,
            path,
        });
    }

    let readers = published_sources
        .iter()
        .map(|source| verify_source_catalog_bundle(&source.path, &source.manifest))
        .collect::<conary_core::Result<Vec<_>>>()?;
    let inputs = published_sources
        .iter()
        .zip(&readers)
        .map(|(source, reader)| ProfileCatalogMemberInputV1 {
            ordinal: source.ordinal,
            priority: source.priority,
            required: source.required,
            manifest: &source.manifest,
            reader,
        })
        .collect();
    let candidate =
        ProfileCatalogCandidateV1::compose(profile, PROFILE_CATALOG_PROJECTION_VERSION, inputs)?;
    let profile_candidate_directory = candidate_run_dir.join("profile");
    create_private_directory(&profile_candidate_directory, &candidate_run_dir)?;
    let binding = write_catalog_candidate(
        profile_candidate_directory.join(CATALOG_FILE_NAME),
        candidate.content(),
    )?;
    let manifest = candidate.bind(&binding)?;
    write_profile_catalog_manifest(&profile_candidate_directory, &manifest)?;
    let path =
        publish_profile_catalog_bundle(&profile_candidate_directory, catalog_root, &manifest)?;

    Ok(PublishedProfileCatalog {
        profile: profile.to_string(),
        manifest,
        path,
        sources: published_sources,
        candidate_run_dir,
    })
}

fn create_candidate_run_dir(root: &Path, run_id: &str) -> Result<PathBuf> {
    let parsed = uuid::Uuid::parse_str(run_id).context("catalog run ID must be a UUID")?;
    if parsed.hyphenated().to_string() != run_id {
        bail!("catalog run ID must use canonical lowercase hyphenated UUID form");
    }
    require_real_directory(root, "catalog candidate root")?;
    let path = root.join(run_id);
    create_private_directory(&path, root)?;
    Ok(path)
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
