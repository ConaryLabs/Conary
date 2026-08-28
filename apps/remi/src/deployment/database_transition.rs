// apps/remi/src/deployment/database_transition.rs

//! Persisted database ownership across recoverable Remi deployments.

use super::{require_plain_directory, require_plain_file, sync_parent};
use anyhow::{Context, Result, bail};
use conary_core::db::schema::{SchemaCompatibility, inspect};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum DatabaseTransition {
    /// The schema revision is the persisted-data compatibility boundary.
    ///
    /// A same-revision binary rollback therefore retains the exact live
    /// database instead of cloning every byte before each deployment.
    KeepCurrent {
        target: PathBuf,
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
pub(super) struct DatabaseFileTransition {
    target: PathBuf,
    backup: PathBuf,
    existed: bool,
}

pub(super) fn plan(
    db_path: &Path,
    compatibility: SchemaCompatibility,
    transition_dir: &Path,
) -> Result<DatabaseTransition> {
    match compatibility {
        SchemaCompatibility::Current => Ok(DatabaseTransition::KeepCurrent {
            target: db_path.to_path_buf(),
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

pub(super) fn apply(transition: &DatabaseTransition) -> Result<()> {
    match transition {
        DatabaseTransition::KeepCurrent { target } => {
            require_plain_file(target, "current SQLite database")?;
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

pub(super) fn rollback(database: &DatabaseTransition, manifest_path: &Path) -> Result<()> {
    match database {
        DatabaseTransition::KeepCurrent { target } => match inspect(target)? {
            SchemaCompatibility::Current => Ok(()),
            compatibility => {
                bail!("same-schema rollback found an incompatible live database: {compatibility:?}")
            }
        },
        DatabaseTransition::Initialize { target } => {
            preserve_failed_database(&sqlite_paths(target), manifest_path)?;
            Ok(())
        }
        DatabaseTransition::Rebuild { files, .. } => {
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
    if created {
        sync_parent(&failed_dir)?;
    }
    Ok(())
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
