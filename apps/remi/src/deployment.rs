// apps/remi/src/deployment.rs

//! Atomic, recoverable Remi configuration and schema transitions.

use crate::server::config::RemiConfig;
use crate::server::repository_manifest::RepositoryManifest;
use crate::server::runtime_lock::RuntimeRootLock;
use anyhow::{Context, Result, bail};
use conary_core::db::schema::{SCHEMA_EPOCH, SCHEMA_VERSION, SchemaCompatibility};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

mod database_transition;
mod refresh_diagnostics;
use database_transition::DatabaseTransition;
pub use refresh_diagnostics::{
    DeploymentProfileRefreshState, DeploymentRefreshFailureCategory, DeploymentRefreshFailureStage,
    DeploymentRefreshRunState,
};

const TRANSITION_SCHEMA: u32 = 3;
const DEFAULT_REPOSITORY_MANIFEST_TARGET: &str = "/etc/conary/remi-repositories.toml";
const DEFAULT_REPOSITORY_KEYS_DIR: &str = "/conary/repository-keys";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentState {
    pub schema_epoch: &'static str,
    pub schema_revision: i32,
    pub configured_profiles: usize,
    pub populated_profiles: usize,
    pub catalog_packages: u64,
    pub converted_packages: i64,
    pub candidate_profiles: usize,
    pub candidate_catalog_packages: u64,
    pub signing_profiles: Vec<String>,
    pub universe: Option<DeploymentUniverseState>,
    pub profiles: Vec<DeploymentProfileState>,
    pub candidates: Vec<DeploymentCandidateState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentProfileState {
    pub profile: String,
    pub configured_sources: usize,
    pub profile_revision_sha256: Option<String>,
    pub packages: u64,
    pub converted_packages: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentCandidateState {
    pub profile: String,
    pub configured_sources: usize,
    pub profile_revision_sha256: Option<String>,
    pub run_id: Option<String>,
    pub completed_at: Option<i64>,
    pub packages: u64,
    pub latest_refresh: Option<DeploymentProfileRefreshState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentUniverseState {
    pub manifest_sha256: String,
    pub sequence: u64,
    pub profiles: usize,
    pub canonical_map_revision: u64,
    pub canonical_map_entries: u64,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub matches_active_profiles: bool,
    pub fresh: bool,
}

impl DeploymentState {
    #[must_use]
    pub fn private_candidates_complete(&self) -> bool {
        self.configured_profiles > 0
            && self.candidate_profiles == self.configured_profiles
            && self.candidate_catalog_packages > 0
            && self.candidates.len() == self.configured_profiles
            && self.candidates.iter().all(|candidate| {
                candidate.profile_revision_sha256.is_some() && candidate.packages > 0
            })
    }

    #[must_use]
    pub fn repopulation_complete(&self) -> bool {
        self.configured_profiles > 0
            && self.populated_profiles == self.configured_profiles
            && self.converted_packages > 0
            && self
                .profiles
                .iter()
                .all(|profile| profile.converted_packages > 0)
            && self
                .universe
                .as_ref()
                .is_some_and(|universe| universe.matches_active_profiles && universe.fresh)
    }
}

#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub config_path: PathBuf,
    pub repository_manifest_source: PathBuf,
    pub repository_manifest_target: PathBuf,
    pub repository_keys_dir: PathBuf,
    pub deployment_id: String,
    pub max_concurrent: usize,
}

impl PrepareOptions {
    #[must_use]
    pub fn production(
        repository_manifest_source: PathBuf,
        deployment_id: String,
        max_concurrent: usize,
    ) -> Self {
        Self {
            config_path: PathBuf::from("/etc/conary/remi.toml"),
            repository_manifest_source,
            repository_manifest_target: PathBuf::from(DEFAULT_REPOSITORY_MANIFEST_TARGET),
            repository_keys_dir: PathBuf::from(DEFAULT_REPOSITORY_KEYS_DIR),
            deployment_id,
            max_concurrent,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionManifest {
    schema_version: u32,
    deployment_id: String,
    runtime_root: PathBuf,
    expected_schema_epoch: String,
    expected_schema_revision: i32,
    config: FileTransition,
    repository_manifest: FileTransition,
    database: DatabaseTransition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTransition {
    target: PathBuf,
    backup: PathBuf,
    existed: bool,
}

pub fn prepare(options: &PrepareOptions) -> Result<PathBuf> {
    validate_deployment_id(&options.deployment_id)?;
    if !(1..=128).contains(&options.max_concurrent) {
        bail!("max_concurrent must be in the range 1..=128");
    }
    require_plain_file(&options.config_path, "Remi config")?;
    require_plain_file(
        &options.repository_manifest_source,
        "repository manifest source",
    )?;
    let repository_manifest = RepositoryManifest::load(&options.repository_manifest_source)?;
    if repository_manifest.repositories.is_empty() {
        bail!("deployment repository manifest must declare at least one source");
    }
    let new_config = build_current_config(options, &repository_manifest)?;
    let parsed_config: RemiConfig =
        toml::from_str(&new_config).context("updated Remi config does not match current schema")?;
    parsed_config.validate()?;

    let runtime_lock = RuntimeRootLock::acquire(parsed_config.storage_root())?;
    let runtime_root = runtime_lock.root().to_path_buf();
    let _runtime_lock = runtime_lock;
    require_plain_directory(&runtime_root, "Remi storage root")?;

    crate::server::signing_authority::ensure_repository_authority(
        &repository_manifest,
        &options.repository_keys_dir,
    )?;
    crate::server::signing_authority::ensure_universe_authority(&options.repository_keys_dir)?;

    let db_path = runtime_root.join("metadata/conary.db");
    let compatibility = conary_core::db::schema::inspect(&db_path)
        .with_context(|| format!("failed to inspect database {}", db_path.display()))?;

    let backup_root = runtime_root.join("deployment-backups");
    fs::create_dir_all(&backup_root)
        .with_context(|| format!("failed to create {}", backup_root.display()))?;
    require_plain_directory(&backup_root, "deployment backup root")?;

    let transition_dir = create_transition_dir(&backup_root, &options.deployment_id)?;
    let manifest_path = transition_dir.join("transition.json");
    let config_transition = plan_file_transition(
        &options.config_path,
        &transition_dir.join("remi.toml.previous"),
    )?;
    let repository_transition = plan_file_transition(
        &options.repository_manifest_target,
        &transition_dir.join("remi-repositories.toml.previous"),
    )?;
    let database = database_transition::plan(&db_path, compatibility, &transition_dir)?;
    let transition = TransitionManifest {
        schema_version: TRANSITION_SCHEMA,
        deployment_id: options.deployment_id.clone(),
        runtime_root,
        expected_schema_epoch: SCHEMA_EPOCH.to_string(),
        expected_schema_revision: SCHEMA_VERSION,
        config: config_transition,
        repository_manifest: repository_transition,
        database,
    };

    write_json_atomic(&manifest_path, &transition)?;
    let apply_result = apply_transition(
        &transition,
        &new_config,
        &options.repository_manifest_source,
    );
    if let Err(error) = apply_result {
        let rollback_error = rollback_transition(&transition, &manifest_path).err();
        if let Some(rollback_error) = rollback_error {
            return Err(error.context(format!(
                "automatic rollback also failed: {rollback_error:#}"
            )));
        }
        return Err(error);
    }

    Ok(manifest_path)
}

/// Initialize or verify the durable endpoint-wide universe signing authority
/// and return its public, self-signed metadata root path.
pub fn initialize_universe_authority(repository_keys_dir: &Path) -> Result<PathBuf> {
    crate::server::signing_authority::ensure_universe_authority(repository_keys_dir)?;
    Ok(crate::server::signing_authority::universe_root_metadata_path(repository_keys_dir))
}

pub fn rollback(manifest_path: &Path) -> Result<()> {
    require_plain_file(manifest_path, "transition manifest")?;
    let content = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let transition: TransitionManifest = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    validate_transition_manifest(&transition)?;
    let _runtime_lock = RuntimeRootLock::acquire(&transition.runtime_root)?;

    rollback_transition(&transition, manifest_path)
}

fn rollback_transition(transition: &TransitionManifest, manifest_path: &Path) -> Result<()> {
    database_transition::rollback(&transition.database, manifest_path)?;
    restore_file(&transition.repository_manifest)?;
    restore_file(&transition.config)?;
    Ok(())
}

/// Inspect deployment state without claiming runtime ownership.
///
/// This is a read-only evidence surface. Its result never proves that prepare,
/// rollback, or another runtime mutation is safe to start.
pub fn inspect_state(config_path: &Path) -> Result<DeploymentState> {
    require_plain_file(config_path, "Remi config")?;
    let config = RemiConfig::load(config_path)?;
    config.validate()?;
    let repository_manifest_path = config
        .repository_manifest
        .as_deref()
        .context("Remi config does not declare repository_manifest")?;
    require_plain_file(repository_manifest_path, "repository manifest")?;
    let repository_manifest = RepositoryManifest::load(repository_manifest_path)?;
    let repository_keys_dir = config
        .release_publish
        .repository_keys_dir
        .as_deref()
        .context("Remi config does not declare release_publish.repository_keys_dir")?;
    let signing_profiles = crate::server::signing_authority::inspect_repository_authority(
        &repository_manifest,
        repository_keys_dir,
    )?;
    crate::server::signing_authority::inspect_universe_authority(repository_keys_dir)?;
    let db_path = config.storage_root().join("metadata/conary.db");
    match conary_core::db::schema::inspect(&db_path)? {
        SchemaCompatibility::Current => {}
        other => bail!("Remi database is not current: {other:?}"),
    }

    let conn = conary_core::db::open_fast(&db_path)?;
    repository_manifest.verify_reconciled(&conn)?;
    let configured_counts = repository_manifest
        .repositories
        .iter()
        .filter(|definition| definition.enabled)
        .filter(|definition| {
            conary_core::repository::supported_profiles::profile_by_public_id(&definition.profile)
                .is_some()
        })
        .fold(
            BTreeMap::<String, usize>::new(),
            |mut profiles, definition| {
                *profiles.entry(definition.profile.clone()).or_default() += 1;
                profiles
            },
        );
    let authority = crate::server::catalog_authority::CatalogAuthority::for_inspection(
        &db_path,
        config.storage_root().join("catalogs"),
    );
    let configured = conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .filter_map(|profile| {
            configured_counts
                .get(profile.id())
                .map(|count| (profile.id().to_string(), *count))
        })
        .collect::<Vec<_>>();
    if configured.len() != configured_counts.len() {
        bail!("configured public profile authority disagrees with the support contract");
    }
    let profiles = inspect_deployment_profiles(&conn, &authority, &configured)?;
    let candidates = inspect_deployment_candidates(&conn, &authority, &configured)?;
    let populated_profiles = profiles
        .iter()
        .filter(|profile| profile.packages > 0)
        .count();
    let catalog_packages = profiles.iter().try_fold(0_u64, |total, profile| {
        total
            .checked_add(profile.packages)
            .context("deployment catalog package count overflow")
    })?;
    let converted_packages = profiles.iter().try_fold(0_i64, |total, profile| {
        total
            .checked_add(profile.converted_packages)
            .context("deployment converted package count overflow")
    })?;
    let candidate_profiles = candidates
        .iter()
        .filter(|candidate| candidate.profile_revision_sha256.is_some())
        .count();
    let candidate_catalog_packages = candidates.iter().try_fold(0_u64, |total, candidate| {
        total
            .checked_add(candidate.packages)
            .context("deployment candidate catalog package count overflow")
    })?;
    let universe = inspect_active_universe(&conn, &profiles)?;

    Ok(DeploymentState {
        schema_epoch: SCHEMA_EPOCH,
        schema_revision: SCHEMA_VERSION,
        configured_profiles: profiles.len(),
        populated_profiles,
        catalog_packages,
        converted_packages,
        candidate_profiles,
        candidate_catalog_packages,
        signing_profiles,
        universe,
        profiles,
        candidates,
    })
}

fn inspect_deployment_profiles(
    conn: &rusqlite::Connection,
    authority: &crate::server::catalog_authority::CatalogAuthority,
    configured: &[(String, usize)],
) -> Result<Vec<DeploymentProfileState>> {
    let mut profiles = Vec::with_capacity(configured.len());
    for (profile, configured_sources) in configured {
        let pointer = conary_core::db::models::RemiActiveProfileRevision::find(conn, profile)?;
        let Some(pointer) = pointer else {
            profiles.push(DeploymentProfileState {
                profile: profile.clone(),
                configured_sources: *configured_sources,
                profile_revision_sha256: None,
                packages: 0,
                converted_packages: 0,
            });
            continue;
        };
        let inspection = authority
            .inspect_active_profile(profile)
            .with_context(|| format!("inspect active immutable profile '{profile}'"))?;
        if inspection.pointer != pointer {
            bail!("active profile '{profile}' changed during deployment inspection");
        }
        if inspection.manifest.members.len() != *configured_sources {
            bail!(
                "active profile '{profile}' contains {} sources; configured authority contains {}",
                inspection.manifest.members.len(),
                configured_sources
            );
        }
        let converted_packages = conn.query_row(
            "SELECT COUNT(*)
             FROM converted_packages
             WHERE source_profile = ?1 AND profile_revision_sha256 = ?2",
            rusqlite::params![profile, &pointer.profile_revision_sha256],
            |row| row.get(0),
        )?;
        profiles.push(DeploymentProfileState {
            profile: profile.clone(),
            configured_sources: *configured_sources,
            profile_revision_sha256: Some(pointer.profile_revision_sha256),
            packages: inspection.manifest.counts.packages,
            converted_packages,
        });
    }
    Ok(profiles)
}

fn inspect_deployment_candidates(
    conn: &rusqlite::Connection,
    authority: &crate::server::catalog_authority::CatalogAuthority,
    configured: &[(String, usize)],
) -> Result<Vec<DeploymentCandidateState>> {
    let mut candidates = Vec::with_capacity(configured.len());
    for (profile, configured_sources) in configured {
        let latest_refresh = refresh_diagnostics::latest_profile_refresh(conn, profile)?;
        let candidate = conary_core::repository::current_profile_sync_candidate(conn, profile)?;
        let Some(candidate) = candidate else {
            candidates.push(DeploymentCandidateState {
                profile: profile.clone(),
                configured_sources: *configured_sources,
                profile_revision_sha256: None,
                run_id: None,
                completed_at: None,
                packages: 0,
                latest_refresh,
            });
            continue;
        };
        let selection = crate::server::catalog_authority::ProfileRevisionSelection {
            source_profile: candidate.source_profile.clone(),
            profile_revision_sha256: candidate.profile_revision_sha256.clone(),
        };
        let inspection = authority
            .verify_selected_profile(&selection)
            .with_context(|| format!("inspect private immutable profile '{profile}'"))?;
        inspection
            .manifest
            .validate_member_contract()
            .with_context(|| format!("validate private profile '{profile}' member contract"))?;
        if inspection.manifest.members.len() != *configured_sources {
            bail!(
                "private profile '{profile}' does not match its exact configured source authority"
            );
        }
        conary_core::db::models::verify_private_profile_candidate_authority(
            conn,
            profile,
            &candidate.profile_revision_sha256,
            &candidate.run_id,
        )
        .with_context(|| format!("verify private profile '{profile}' repository authority"))?;
        if conary_core::repository::current_profile_sync_candidate(conn, profile)?.as_ref()
            != Some(&candidate)
        {
            bail!("private profile '{profile}' changed during deployment inspection");
        }
        candidates.push(DeploymentCandidateState {
            profile: profile.clone(),
            configured_sources: *configured_sources,
            profile_revision_sha256: Some(candidate.profile_revision_sha256),
            run_id: Some(candidate.run_id),
            completed_at: Some(candidate.completed_at),
            packages: inspection.manifest.counts.packages,
            latest_refresh,
        });
    }
    Ok(candidates)
}

fn inspect_active_universe(
    conn: &rusqlite::Connection,
    profiles: &[DeploymentProfileState],
) -> Result<Option<DeploymentUniverseState>> {
    use rusqlite::OptionalExtension;

    let active = conn
        .query_row(
            "SELECT active.manifest_sha256, active.sequence, revision.manifest_json
             FROM remi_active_universe_revision active
             JOIN remi_universe_revisions revision
               ON revision.manifest_sha256 = active.manifest_sha256
             WHERE active.singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((manifest_sha256, sequence, manifest_json)) = active else {
        return Ok(None);
    };
    let manifest =
        serde_json::from_str::<conary_core::repository::universe::RemiUniverseManifestV2>(
            &manifest_json,
        )
        .context("parse active Remi universe manifest")?;
    manifest.validate().map_err(anyhow::Error::from)?;
    let canonical = conary_core::json::canonical_json(&manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize active Remi universe manifest")?;
    let sequence = u64::try_from(sequence).context("active universe sequence is negative")?;
    if canonical != manifest_json.as_bytes()
        || manifest.manifest_sha256()? != manifest_sha256
        || manifest.sequence != sequence
    {
        bail!("active Remi universe pointer disagrees with its manifest authority");
    }
    let matches_active_profiles = manifest.profiles.len() == profiles.len()
        && manifest
            .profiles
            .iter()
            .zip(profiles)
            .all(|(member, active)| {
                member.revision.profile == active.profile
                    && active.profile_revision_sha256.as_deref()
                        == Some(member.profile_revision_sha256.as_str())
            });
    let fresh = manifest.expires_at > chrono::Utc::now();
    Ok(Some(DeploymentUniverseState {
        manifest_sha256,
        sequence,
        profiles: manifest.profiles.len(),
        canonical_map_revision: manifest.canonical_map.revision,
        canonical_map_entries: manifest.canonical_map.entry_count,
        expires_at: manifest.expires_at,
        matches_active_profiles,
        fresh,
    }))
}

fn build_current_config(
    options: &PrepareOptions,
    repository_manifest: &RepositoryManifest,
) -> Result<String> {
    let content = fs::read_to_string(&options.config_path)
        .with_context(|| format!("failed to read {}", options.config_path.display()))?;
    let mut value: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse {}", options.config_path.display()))?;
    let root = value
        .as_table_mut()
        .context("Remi config root must be a TOML table")?;

    root.remove("upstream");
    root.insert(
        "repository_manifest".to_string(),
        toml::Value::String(
            options
                .repository_manifest_target
                .to_str()
                .context("repository manifest target must be UTF-8")?
                .to_string(),
        ),
    );
    let conversion = root
        .entry("conversion")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .context("conversion must be a TOML table")?;
    conversion.insert(
        "max_concurrent".to_string(),
        toml::Value::Integer(options.max_concurrent as i64),
    );
    if let Some(storage) = root.get_mut("storage").and_then(toml::Value::as_table_mut) {
        storage.remove("eviction_threshold");
        storage.remove("eviction_min_age");
    }
    if let Some(r2) = root.get_mut("r2").and_then(toml::Value::as_table_mut) {
        r2.remove("account_id");
        r2.remove("write_through");
        r2.remove("r2_redirect");
    }
    let release_publish = root
        .entry("release_publish")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .context("release_publish must be a TOML table")?;
    release_publish.insert(
        "repository_keys_dir".to_string(),
        toml::Value::String(
            options
                .repository_keys_dir
                .to_str()
                .context("repository signing authority path must be UTF-8")?
                .to_string(),
        ),
    );
    let prewarm = root
        .entry("prewarm")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .context("prewarm must be a TOML table")?;
    let profiles = repository_manifest
        .repositories
        .iter()
        .filter(|repository| {
            conary_core::repository::supported_profiles::profile_by_public_id(&repository.profile)
                .is_some()
        })
        .map(|repository| {
            conary_core::repository::supported_profiles::profile_by_public_id(&repository.profile)
                .map(|profile| profile.remi_route_slug())
                .with_context(|| {
                    format!(
                        "validated repository profile '{}' disappeared",
                        repository.profile
                    )
                })
        })
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    prewarm.insert("enabled".to_string(), toml::Value::Boolean(true));
    prewarm.insert(
        "metadata_sync_interval".to_string(),
        toml::Value::String("6h".to_string()),
    );
    prewarm.insert("convert_top_n".to_string(), toml::Value::Integer(1000));
    prewarm.insert(
        "distros".to_string(),
        toml::Value::Array(
            profiles
                .into_iter()
                .map(|profile| toml::Value::String(profile.to_string()))
                .collect(),
        ),
    );

    toml::to_string_pretty(&value).context("failed to serialize current Remi config")
}

fn create_transition_dir(root: &Path, deployment_id: &str) -> Result<PathBuf> {
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    for suffix in 0..100_u8 {
        let name = if suffix == 0 {
            format!("{timestamp}-{deployment_id}-{SCHEMA_EPOCH}")
        } else {
            format!("{timestamp}-{deployment_id}-{SCHEMA_EPOCH}-{suffix}")
        };
        let path = root.join(name);
        match fs::create_dir(&path) {
            Ok(()) => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o750))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to create {}", path.display()));
            }
        }
    }
    bail!("could not allocate a unique deployment backup directory")
}

fn plan_file_transition(target: &Path, backup: &Path) -> Result<FileTransition> {
    let existed = target.exists();
    if existed {
        require_plain_file(target, "deployment-managed file")?;
    } else if let Some(parent) = target.parent() {
        require_plain_directory(parent, "deployment-managed file parent")?;
    }
    Ok(FileTransition {
        target: target.to_path_buf(),
        backup: backup.to_path_buf(),
        existed,
    })
}

fn apply_transition(
    transition: &TransitionManifest,
    new_config: &str,
    repository_manifest_source: &Path,
) -> Result<()> {
    backup_file(&transition.config)?;
    backup_file(&transition.repository_manifest)?;
    install_bytes_atomic(&transition.config.target, new_config.as_bytes(), 0o644)?;
    install_file_atomic(
        repository_manifest_source,
        &transition.repository_manifest.target,
        0o644,
    )?;

    database_transition::apply(&transition.database)
}

fn backup_file(transition: &FileTransition) -> Result<()> {
    if transition.existed {
        fs::copy(&transition.target, &transition.backup).with_context(|| {
            format!(
                "failed to back up {} to {}",
                transition.target.display(),
                transition.backup.display()
            )
        })?;
        File::open(&transition.backup)?.sync_all()?;
    }
    Ok(())
}

fn restore_file(transition: &FileTransition) -> Result<()> {
    if transition.existed {
        require_plain_file(&transition.backup, "deployment file backup")?;
        install_file_atomic(&transition.backup, &transition.target, 0o644)
    } else {
        remove_plain_file_if_present(&transition.target)
    }
}

fn install_file_atomic(source: &Path, target: &Path, mode: u32) -> Result<()> {
    require_plain_file(source, "deployment source")?;
    let bytes = fs::read(source).with_context(|| format!("failed to read {}", source.display()))?;
    install_bytes_atomic(target, &bytes, mode)
}

fn install_bytes_atomic(target: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = target.parent().context("deployment target has no parent")?;
    require_plain_directory(parent, "deployment target parent")?;
    if target.exists() {
        require_plain_file(target, "deployment target")?;
    }
    let temp = parent.join(format!(
        ".{}.next-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .context("deployment target name must be UTF-8")?,
        std::process::id()
    ));
    remove_plain_file_if_present(&temp)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(mode))?;
    fs::rename(&temp, target)?;
    sync_parent(parent)
}

fn write_json_atomic(path: &Path, value: &TransitionManifest) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    install_bytes_atomic(path, &bytes, 0o600)
}

fn remove_plain_file_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                bail!("refusing to remove non-regular file {}", path.display());
            }
            fs::remove_file(path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn require_plain_file(path: &Path, description: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("{description} is missing: {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("{description} is not a plain file: {}", path.display());
    }
    Ok(())
}

fn require_plain_directory(path: &Path, description: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("{description} is missing: {}", path.display()))?;
    if !metadata.file_type().is_dir() {
        bail!("{description} is not a plain directory: {}", path.display());
    }
    Ok(())
}

fn validate_deployment_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 96
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character))
    {
        bail!(
            "deployment ID must use 1 to 96 ASCII alphanumeric, '.', '_', '+', or '-' characters"
        );
    }
    Ok(())
}

fn validate_transition_manifest(transition: &TransitionManifest) -> Result<()> {
    if transition.schema_version != TRANSITION_SCHEMA {
        bail!(
            "unsupported transition manifest schema {}",
            transition.schema_version
        );
    }
    if transition.expected_schema_epoch != SCHEMA_EPOCH
        || transition.expected_schema_revision != SCHEMA_VERSION
    {
        bail!(
            "transition targets schema {} revision {}, but this binary owns {} revision {}",
            transition.expected_schema_epoch,
            transition.expected_schema_revision,
            SCHEMA_EPOCH,
            SCHEMA_VERSION
        );
    }
    if !transition.runtime_root.is_absolute() {
        bail!("transition runtime root must be absolute");
    }
    let canonical_root = fs::canonicalize(&transition.runtime_root).with_context(|| {
        format!(
            "failed to resolve transition runtime root {}",
            transition.runtime_root.display()
        )
    })?;
    if canonical_root != transition.runtime_root {
        bail!(
            "transition runtime root is not canonical: {}",
            transition.runtime_root.display()
        );
    }
    validate_deployment_id(&transition.deployment_id)
}

fn sync_parent(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
