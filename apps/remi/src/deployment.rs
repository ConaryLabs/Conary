// apps/remi/src/deployment.rs

//! Atomic, recoverable Remi configuration and schema transitions.

use crate::server::config::RemiConfig;
use crate::server::repository_manifest::RepositoryManifest;
use anyhow::{Context, Result, bail};
use conary_core::db::schema::{SCHEMA_EPOCH, SCHEMA_VERSION, SchemaCompatibility};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TRANSITION_SCHEMA: u32 = 1;
const DEFAULT_REPOSITORY_MANIFEST_TARGET: &str = "/etc/conary/remi-repositories.toml";
const DEFAULT_REPOSITORY_KEYS_DIR: &str = "/conary/repository-keys";

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentState {
    pub schema_epoch: &'static str,
    pub schema_revision: i32,
    pub configured_sources: usize,
    pub populated_sources: usize,
    pub repository_packages: i64,
    pub converted_packages: i64,
    pub signing_profiles: Vec<String>,
    pub sources: Vec<DeploymentSourceState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentSourceState {
    pub name: String,
    pub profile: String,
    pub package_format: &'static str,
    pub packages: i64,
    pub converted_packages: i64,
}

impl DeploymentState {
    #[must_use]
    pub fn repopulation_complete(&self) -> bool {
        self.configured_sources > 0
            && self.populated_sources == self.configured_sources
            && self.converted_packages > 0
            && self
                .sources
                .iter()
                .all(|source| source.converted_packages > 0)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum DatabaseTransition {
    KeepCurrent {
        files: Vec<DatabaseFileTransition>,
    },
    Initialize {
        target: PathBuf,
    },
    Rebuild {
        observed: String,
        files: Vec<DatabaseFileTransition>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseFileTransition {
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
    crate::server::signing_authority::ensure_repository_authority(
        &repository_manifest,
        &options.repository_keys_dir,
    )?;

    let new_config = build_current_config(options, &repository_manifest)?;
    let parsed_config: RemiConfig =
        toml::from_str(&new_config).context("updated Remi config does not match current schema")?;
    parsed_config.validate()?;
    let db_path = parsed_config.storage_root().join("metadata/conary.db");
    let compatibility = conary_core::db::schema::inspect(&db_path)
        .with_context(|| format!("failed to inspect database {}", db_path.display()))?;

    let backup_root = parsed_config.storage_root().join("deployment-backups");
    require_plain_directory(parsed_config.storage_root(), "Remi storage root")?;
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
    let database = plan_database_transition(&db_path, compatibility, &transition_dir)?;
    let transition = TransitionManifest {
        schema_version: TRANSITION_SCHEMA,
        deployment_id: options.deployment_id.clone(),
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
        let rollback_error = rollback(&manifest_path).err();
        if let Some(rollback_error) = rollback_error {
            return Err(error.context(format!(
                "automatic rollback also failed: {rollback_error:#}"
            )));
        }
        return Err(error);
    }

    Ok(manifest_path)
}

pub fn rollback(manifest_path: &Path) -> Result<()> {
    require_plain_file(manifest_path, "transition manifest")?;
    let content = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let transition: TransitionManifest = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    validate_transition_manifest(&transition)?;

    rollback_database(&transition.database, manifest_path)?;
    restore_file(&transition.repository_manifest)?;
    restore_file(&transition.config)?;
    Ok(())
}

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
    let db_path = config.storage_root().join("metadata/conary.db");
    match conary_core::db::schema::inspect(&db_path)? {
        SchemaCompatibility::Current => {}
        other => bail!("Remi database is not current: {other:?}"),
    }

    let conn = conary_core::db::open_fast(&db_path)?;
    repository_manifest.verify_reconciled(&conn)?;
    let mut sources = Vec::with_capacity(repository_manifest.repositories.len());
    let mut populated_sources = 0;
    for definition in &repository_manifest.repositories {
        let packages: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM repository_packages rp
             JOIN repositories r ON r.id = rp.repository_id
             WHERE r.name = ?1",
            [&definition.name],
            |row| row.get(0),
        )?;
        if packages > 0 {
            populated_sources += 1;
        }
        let converted_packages: i64 = conn.query_row(
            "SELECT COUNT(*) FROM converted_packages WHERE source_profile = ?1",
            [&definition.profile],
            |row| row.get(0),
        )?;
        sources.push(DeploymentSourceState {
            name: definition.name.clone(),
            profile: definition.profile.clone(),
            package_format: definition.parser.format().as_str(),
            packages,
            converted_packages,
        });
    }

    Ok(DeploymentState {
        schema_epoch: SCHEMA_EPOCH,
        schema_revision: SCHEMA_VERSION,
        configured_sources: sources.len(),
        populated_sources,
        repository_packages: count_rows(&conn, "repository_packages")?,
        converted_packages: count_rows(&conn, "converted_packages")?,
        signing_profiles,
        sources,
    })
}

fn count_rows(conn: &rusqlite::Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get(0))
        .with_context(|| format!("failed to count {table}"))
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

fn plan_database_transition(
    db_path: &Path,
    compatibility: SchemaCompatibility,
    transition_dir: &Path,
) -> Result<DatabaseTransition> {
    match compatibility {
        SchemaCompatibility::Current => Ok(DatabaseTransition::KeepCurrent {
            files: plan_database_files(db_path, transition_dir)?,
        }),
        SchemaCompatibility::Fresh => {
            if let Some(parent) = db_path.parent() {
                require_plain_directory(parent, "database parent")?;
            }
            Ok(DatabaseTransition::Initialize {
                target: db_path.to_path_buf(),
            })
        }
        SchemaCompatibility::RebuildRequired { observed } => {
            let files = plan_database_files(db_path, transition_dir)?;
            Ok(DatabaseTransition::Rebuild { observed, files })
        }
    }
}

fn plan_database_files(
    db_path: &Path,
    transition_dir: &Path,
) -> Result<Vec<DatabaseFileTransition>> {
    sqlite_paths(db_path)
        .into_iter()
        .map(|target| {
            let existed = target.exists();
            if existed {
                require_plain_file(&target, "SQLite database file")?;
            }
            let file_name = target
                .file_name()
                .context("SQLite database path has no file name")?
                .to_os_string();
            Ok(DatabaseFileTransition {
                target,
                backup: transition_dir.join(file_name),
                existed,
            })
        })
        .collect()
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

    match &transition.database {
        DatabaseTransition::KeepCurrent { files } => {
            for file in files.iter().filter(|file| file.existed) {
                fs::copy(&file.target, &file.backup).with_context(|| {
                    format!(
                        "failed to snapshot {} to {}",
                        file.target.display(),
                        file.backup.display()
                    )
                })?;
                File::open(&file.backup)?.sync_all()?;
            }
            sync_parent(
                files
                    .first()
                    .and_then(|file| file.backup.parent())
                    .context("database transition has no backup parent")?,
            )?;
        }
        DatabaseTransition::Rebuild { files, .. } => {
            for file in files.iter().filter(|file| file.existed) {
                fs::rename(&file.target, &file.backup).with_context(|| {
                    format!(
                        "failed to move {} to {}",
                        file.target.display(),
                        file.backup.display()
                    )
                })?;
            }
            sync_parent(
                files
                    .first()
                    .and_then(|file| file.target.parent())
                    .context("database transition has no parent")?,
            )?;
        }
        DatabaseTransition::Initialize { .. } => {}
    }
    Ok(())
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

fn rollback_database(database: &DatabaseTransition, manifest_path: &Path) -> Result<()> {
    match database {
        DatabaseTransition::Initialize { target } => {
            preserve_failed_database(&sqlite_paths(target), manifest_path)?;
            Ok(())
        }
        DatabaseTransition::KeepCurrent { files } | DatabaseTransition::Rebuild { files, .. } => {
            for file in files.iter().filter(|file| !file.existed) {
                preserve_failed_database(std::slice::from_ref(&file.target), manifest_path)?;
            }
            for file in files.iter().filter(|file| file.existed) {
                if file.backup.exists() {
                    preserve_failed_database(std::slice::from_ref(&file.target), manifest_path)?;
                    require_plain_file(&file.backup, "SQLite transition backup")?;
                    fs::rename(&file.backup, &file.target).with_context(|| {
                        format!(
                            "failed to restore {} from {}",
                            file.target.display(),
                            file.backup.display()
                        )
                    })?;
                } else if !file.target.exists() {
                    bail!(
                        "database transition lost both live file and backup for {}",
                        file.target.display()
                    );
                }
            }
            if let Some(parent) = files.first().and_then(|file| file.target.parent()) {
                sync_parent(parent)?;
            }
            Ok(())
        }
    }
}

fn preserve_failed_database(paths: &[PathBuf], manifest_path: &Path) -> Result<()> {
    let failed_dir = manifest_path
        .parent()
        .context("transition manifest has no parent")?
        .join("failed-current");
    let mut created = false;
    for path in paths {
        if !path.exists() {
            continue;
        }
        require_plain_file(path, "failed current database file")?;
        if !created {
            fs::create_dir_all(&failed_dir)?;
            fs::set_permissions(&failed_dir, fs::Permissions::from_mode(0o750))?;
            created = true;
        }
        let file_name = path.file_name().context("database path has no file name")?;
        fs::rename(path, failed_dir.join(file_name))?;
    }
    Ok(())
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
    validate_deployment_id(&transition.deployment_id)
}

fn sqlite_paths(db_path: &Path) -> [PathBuf; 3] {
    [
        db_path.to_path_buf(),
        appended_path(db_path, "-wal"),
        appended_path(db_path, "-shm"),
    ]
}

fn appended_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn sync_parent(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::fs::symlink;

    fn write(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    fn base_config(root: &Path) -> String {
        format!(
            r#"
[server]
bind = "127.0.0.1:8080"
admin_bind = "127.0.0.1:8081"

[storage]
root = "{}"

[upstream.old]
base_url = "https://legacy.example.test"

[conversion]
max_concurrent = 4
"#,
            root.display()
        )
    }

    fn repository_manifest() -> &'static str {
        r#"
schema_version = 3

[[repositories]]
name = "opaque-source"
url = "https://packages.example.test/repository"
profile = "fedora-44"
source_identity = "example-publisher"
repository_identity = "opaque-source-rpm-x86_64"
stream_kind = "release"
stream_identity = "44"
update_mode = "follow"
enabled = true
priority = 100
metadata_expire_seconds = 21600

[repositories.parser]
package_format = "rpm"
architecture = "x86_64"

[repositories.trust]
ecosystem = "rpm"

[repositories.trust.metadata]
kind = "metalink"
url = "https://mirrors.example.test/metalink"

[[repositories.trust.package_keys]]
url = "https://keys.example.test/fedora.gpg"
fingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
"#
    }

    fn options(root: &Path) -> PrepareOptions {
        PrepareOptions {
            config_path: root.join("etc/remi.toml"),
            repository_manifest_source: root.join("staged-repositories.toml"),
            repository_manifest_target: root.join("etc/remi-repositories.toml"),
            repository_keys_dir: root.join("repository-keys"),
            deployment_id: "remi-0.8.0".to_string(),
            max_concurrent: 32,
        }
    }

    fn arrange() -> (tempfile::TempDir, PrepareOptions) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        fs::create_dir(root.join("etc")).unwrap();
        fs::create_dir(root.join("metadata")).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("repository-keys"))
            .unwrap();
        write(&root.join("etc/remi.toml"), &base_config(&root));
        write(
            &root.join("staged-repositories.toml"),
            repository_manifest(),
        );
        let options = options(&root);
        (temp, options)
    }

    #[test]
    fn prepare_hard_switches_config_and_initializes_fresh_database() {
        let (temp, options) = arrange();
        let manifest = prepare(&options).unwrap();

        let config = fs::read_to_string(&options.config_path).unwrap();
        assert!(!config.contains("[upstream"));
        assert!(config.contains("max_concurrent = 32"));
        assert!(config.contains("repository_manifest"));
        assert!(config.contains("repository_keys_dir"));
        assert!(config.contains("convert_top_n = 1000"));
        assert!(config.contains("distros = [\"fedora\"]"));
        assert_eq!(
            fs::read_to_string(&options.repository_manifest_target).unwrap(),
            repository_manifest()
        );

        fs::write(
            temp.path().join("metadata/conary.db"),
            b"new-current-database",
        )
        .unwrap();
        rollback(&manifest).unwrap();
        assert!(
            fs::read_to_string(&options.config_path)
                .unwrap()
                .contains("[upstream.old]")
        );
        assert!(!options.repository_manifest_target.exists());
        assert!(!temp.path().join("metadata/conary.db").exists());
    }

    #[test]
    fn inspect_state_proves_exact_reconciled_source_authority() {
        let (_temp, options) = arrange();
        prepare(&options).unwrap();

        let config = RemiConfig::load(&options.config_path).unwrap();
        let db_path = config.storage_root().join("metadata/conary.db");
        let mut conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        RepositoryManifest::load(&options.repository_manifest_target)
            .unwrap()
            .reconcile(&mut conn)
            .unwrap();
        drop(conn);

        let state = inspect_state(&options.config_path).unwrap();
        assert_eq!(state.schema_epoch, SCHEMA_EPOCH);
        assert_eq!(state.configured_sources, 1);
        assert_eq!(state.populated_sources, 0);
        assert_eq!(state.signing_profiles, vec!["fedora-44"]);
        assert_eq!(state.sources[0].name, "opaque-source");
        assert_eq!(state.sources[0].profile, "fedora-44");
        assert_eq!(state.sources[0].package_format, "rpm");
        assert!(!state.repopulation_complete());
        let json = serde_json::to_value(&state).unwrap();
        assert!(json.get("evidence_clusters").is_none());
        assert!(json.get("evidence_samples").is_none());
        assert!(json["sources"][0].get("evidence_samples").is_none());
    }

    #[test]
    fn retired_database_is_moved_and_restored_on_rollback() {
        let (temp, options) = arrange();
        let db_path = temp.path().join("metadata/conary.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO schema_version (version) VALUES (79);",
        )
        .unwrap();
        drop(conn);
        let original = fs::read(&db_path).unwrap();

        let manifest = prepare(&options).unwrap();
        assert!(!db_path.exists());
        conary_core::db::init(&db_path).unwrap();
        rollback(&manifest).unwrap();
        assert_eq!(fs::read(&db_path).unwrap(), original);
        assert!(
            manifest
                .parent()
                .unwrap()
                .join("failed-current/conary.db")
                .exists()
        );
    }

    #[test]
    fn current_database_is_snapshotted_and_restored_on_rollback() {
        let (temp, options) = arrange();
        let db_path = temp.path().join("metadata/conary.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE deployment_marker (value TEXT NOT NULL);
             INSERT INTO deployment_marker (value) VALUES ('before');",
        )
        .unwrap();
        drop(conn);

        let manifest = prepare(&options).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE deployment_marker SET value = 'after'", [])
            .unwrap();
        drop(conn);

        rollback(&manifest).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let marker: String = conn
            .query_row("SELECT value FROM deployment_marker", [], |row| row.get(0))
            .unwrap();
        assert_eq!(marker, "before");
        assert!(
            manifest
                .parent()
                .unwrap()
                .join("failed-current/conary.db")
                .exists()
        );
    }

    #[test]
    fn prepare_rejects_symlinked_authority_inputs() {
        let (temp, options) = arrange();
        let real = temp.path().join("real-manifest.toml");
        fs::rename(&options.repository_manifest_source, &real).unwrap();
        symlink(&real, &options.repository_manifest_source).unwrap();

        assert!(
            prepare(&options)
                .unwrap_err()
                .to_string()
                .contains("plain file")
        );
        assert!(
            fs::read_to_string(&options.config_path)
                .unwrap()
                .contains("[upstream.old]")
        );
    }
}
