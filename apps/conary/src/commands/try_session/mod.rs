// apps/conary/src/commands/try_session/mod.rs
//! Try-session policy helpers.

use anyhow::{Context, Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::db::backup::{CheckpointReason, create_checkpoint};
use conary_core::db::models::{CreateTrySession, TrySession, TrySessionMode};
use conary_core::packages::traits::PackageFormat;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod executor;
mod install;
mod namespace;
mod session;
mod util;
mod validation;

use executor::run_try_command_for_session;
#[cfg(test)]
use install::build_try_transaction_config;
use install::{build_try_install_plan, install_try_package};
use namespace::{
    apply_declarative_try_hooks, expose_try_namespace_root, hook_account_entry_exists,
    promotable_try_hook_root, root_relative_path, teardown_try_namespace_mounts,
};
use util::remove_dir_if_exists;
use validation::{TryExecutionRoot, validate_try_package_policy};

#[derive(Debug, Clone, Copy)]
pub(crate) struct TryStartRequest<'a> {
    pub db_path: &'a str,
    pub package_path: &'a Path,
    pub activate: bool,
    pub allow_irreversible: bool,
    pub command: Option<&'a [&'a str]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TryStartOutcome {
    pub session_id: String,
    pub work_dir: PathBuf,
    pub install_root: PathBuf,
    pub copied_package_path: PathBuf,
    pub copied_db_path: PathBuf,
    pub namespace_root: PathBuf,
    pub try_generation_id: i64,
}

pub(crate) async fn cmd_try_package(
    db_path: &str,
    package_path: &Path,
    activate: bool,
    allow_irreversible: bool,
    run: &[String],
) -> Result<()> {
    let command = run.iter().map(String::as_str).collect::<Vec<_>>();
    let outcome = begin_try_session(TryStartRequest {
        db_path,
        package_path,
        activate,
        allow_irreversible,
        command: if command.is_empty() {
            None
        } else {
            Some(command.as_slice())
        },
    })?;

    println!("Try session {} is active", outcome.session_id);
    println!("Package copy: {}", outcome.copied_package_path.display());
    println!("Namespace root: {}", outcome.namespace_root.display());
    println!("Generation: {}", outcome.try_generation_id);
    if activate {
        println!(
            "Run `conary try keep` to keep it or `conary try rollback` to restore the previous generation."
        );
    } else {
        println!("Run `conary try keep` to promote it or `conary try rollback` to discard it.");
    }
    Ok(())
}

pub(crate) async fn cmd_try_status(db_path: &str) -> Result<()> {
    let live_conn = conary_core::db::open(db_path)?;
    match TrySession::find_active_or_orphaned(&live_conn)? {
        Some(session) => {
            println!("Try session: {}", session.id);
            println!("Status: {}", session.status.as_str());
            println!("Mode: {}", session.mode.as_str());
            if let Some(name) = &session.package_name {
                println!("Package: {name}");
            }
            if let Some(version) = &session.package_version {
                println!("Version: {version}");
            }
            if let Some(generation) = session.try_generation_id {
                println!("Generation: {generation}");
            }
            if let Some(pid) = session.launcher_pid {
                println!("Launcher PID: {pid}");
            }
        }
        None => {
            println!("No active try session");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_try_rollback(db_path: &str) -> Result<()> {
    rollback_active_try_session(db_path)?;
    println!("Try session rolled back");
    Ok(())
}

pub(crate) async fn cmd_try_keep(db_path: &str) -> Result<()> {
    keep_active_try_session(db_path)?;
    println!("Try session kept");
    Ok(())
}

pub(crate) fn begin_try_session(request: TryStartRequest<'_>) -> Result<TryStartOutcome> {
    let live_conn = conary_core::db::open(request.db_path)
        .with_context(|| format!("failed to open Conary DB {}", request.db_path))?;
    if let Some(active) = TrySession::find_active_or_orphaned(&live_conn)? {
        bail!(
            "active or orphaned try session already exists: {}",
            active.id
        );
    }

    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(request.db_path));
    let session_id = format!("try-{}", uuid::Uuid::new_v4());
    let work_dir = runtime_root.root().join("try").join(&session_id);
    let install_root = work_dir.join("root");
    let copied_package_path = work_dir.join("package.ccs");
    let copied_db_path = work_dir.join("conary.db");
    std::fs::create_dir_all(&install_root).with_context(|| {
        format!(
            "failed to create try install root {}",
            install_root.display()
        )
    })?;
    std::fs::copy(request.package_path, &copied_package_path).with_context(|| {
        format!(
            "failed to copy try package {} to {}",
            request.package_path.display(),
            copied_package_path.display()
        )
    })?;

    let copied_package_path_string = copied_package_path.to_string_lossy().into_owned();
    let package = <CcsPackage as PackageFormat>::parse(&copied_package_path_string)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| {
            format!(
                "failed to parse try package {}",
                copied_package_path.display()
            )
        })?;
    let mode = if request.activate {
        TrySessionMode::Activated
    } else {
        TrySessionMode::Namespace
    };
    let execution_root = if request.activate {
        TryExecutionRoot::Generation
    } else {
        TryExecutionRoot::Namespace
    };
    validate_try_package_policy(
        &package,
        execution_root,
        request.allow_irreversible,
        request.activate,
    )?;

    let previous_generation_id = if request.activate {
        conary_core::generation::mount::current_generation(runtime_root.root())?
    } else {
        None
    };
    let session = TrySession::create_active(
        &live_conn,
        CreateTrySession {
            id: &session_id,
            package_path: &copied_package_path_string,
            package_name: Some(package.name()),
            package_version: Some(package.version()),
            previous_generation_id,
            mode,
            work_dir: &work_dir.to_string_lossy(),
        },
    )?;
    vacuum_db_into(&live_conn, &copied_db_path)?;

    let mut copied_conn = conary_core::db::open(&copied_db_path)?;
    let install_plan =
        build_try_install_plan(&runtime_root, &work_dir, copied_db_path.clone(), mode);
    install_try_package(&mut copied_conn, &package, &install_plan)?;

    let summary = format!("Try {}-{}", package.name(), package.version());
    let built = crate::commands::composefs_ops::build_inactive_generation_for_runtime(
        &copied_conn,
        &runtime_root,
        &summary,
        None,
    )?;
    let hook_upperdir = promotable_try_hook_root(&runtime_root, built.generation_number)?;
    let namespace_root = expose_try_namespace_root(
        &runtime_root,
        &work_dir,
        &copied_conn,
        built.generation_number,
        &hook_upperdir,
    )?;
    apply_declarative_try_hooks(package.manifest(), &namespace_root)?;

    session.set_try_generation(&live_conn, built.generation_number)?;
    let copied_session = TrySession::find_by_id(&copied_conn, &session_id)?
        .ok_or_else(|| anyhow::anyhow!("copied try session {session_id} missing"))?;
    copied_session.set_try_generation(&copied_conn, built.generation_number)?;

    if request.activate {
        eprintln!(
            "WARNING: activated try publishes generation {} as the host-global current generation; use `conary try rollback` if validation fails.",
            built.generation_number
        );
        crate::commands::composefs_ops::publish_generation_link(
            request.db_path,
            built.generation_number,
        )?;
        if request.command.is_none() {
            record_activated_try_boot(&live_conn, &session.id, &current_boot_id())?;
        }
    }

    if let Some(command) = request.command {
        run_try_command_for_session(
            command,
            &namespace_root,
            request.activate,
            &live_conn,
            &copied_conn,
            &session,
            &copied_session,
        )?;
    }

    Ok(TryStartOutcome {
        session_id,
        work_dir,
        install_root,
        copied_package_path,
        copied_db_path,
        namespace_root,
        try_generation_id: built.generation_number,
    })
}

pub(crate) fn rollback_active_try_session(db_path: &str) -> Result<()> {
    let live_conn = conary_core::db::open(db_path)?;
    let session = TrySession::find_active_or_orphaned(&live_conn)?
        .ok_or_else(|| anyhow::anyhow!("no active or orphaned try session found"))?;
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let work_dir = PathBuf::from(&session.work_dir);

    if session.mode == TrySessionMode::Activated {
        let package = <CcsPackage as PackageFormat>::parse(&session.package_path)
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| {
                format!(
                    "failed to read copied try package {} for activated rollback",
                    session.package_path
                )
            })?;
        validate_try_package_policy(&package, TryExecutionRoot::Generation, false, true)?;
        if let Some(previous) = session.previous_generation_id {
            crate::commands::composefs_ops::publish_generation_link(db_path, previous)?;
        }
    } else {
        teardown_try_namespace_mounts(&work_dir)?;
        if let Some(try_generation_id) = session.try_generation_id {
            let current = conary_core::generation::mount::current_generation(runtime_root.root())?;
            if current != Some(try_generation_id) {
                remove_dir_if_exists(runtime_root.generation_path(try_generation_id))?;
                remove_dir_if_exists(
                    runtime_root
                        .etc_state_dir()
                        .join(try_generation_id.to_string()),
                )?;
            }
        }
    }

    remove_dir_if_exists(work_dir)?;
    session.mark_rolled_back(&live_conn)?;
    drop(live_conn);
    Ok(())
}

pub(crate) fn keep_active_try_session(db_path: &str) -> Result<()> {
    keep_active_try_session_inner(db_path, || {})
}

#[cfg(test)]
fn keep_active_try_session_with_probe<F>(db_path: &str, probe: F) -> Result<()>
where
    F: FnOnce(),
{
    keep_active_try_session_inner(db_path, probe)
}

fn keep_active_try_session_inner<F>(db_path: &str, probe: F) -> Result<()>
where
    F: FnOnce(),
{
    let live_conn = conary_core::db::open(db_path)?;
    let session = TrySession::find_active_or_orphaned(&live_conn)?
        .ok_or_else(|| anyhow::anyhow!("no active or orphaned try session found"))?;
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));

    if session.mode == TrySessionMode::Activated {
        let mut lock_config = build_try_transaction_config(&runtime_root, PathBuf::from(db_path));
        lock_config.lock_timeout_secs = TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS;
        let mut lock_engine = TransactionEngine::new(lock_config)?;
        lock_engine.begin()?;

        let result = (|| -> Result<()> {
            let try_generation_id = session
                .try_generation_id
                .ok_or_else(|| anyhow::anyhow!("activated try session has no try generation"))?;
            let current = conary_core::generation::mount::current_generation(runtime_root.root())?;
            if current != Some(try_generation_id) {
                bail!(
                    "activated try generation {try_generation_id} is no longer current; run `conary try rollback`"
                );
            }
            session.mark_kept(&live_conn)?;
            probe();
            Ok(())
        })();

        lock_engine.release_lock();
        return result;
    }

    let try_generation_id = session
        .try_generation_id
        .ok_or_else(|| anyhow::anyhow!("namespace try session has no try generation"))?;
    let work_dir = PathBuf::from(&session.work_dir);
    let copied_db_path = work_dir.join("conary.db");
    let mut lock_config = build_try_transaction_config(&runtime_root, PathBuf::from(db_path));
    lock_config.lock_timeout_secs = TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS;
    let mut lock_engine = TransactionEngine::new(lock_config)?;
    lock_engine.begin()?;

    let result = (|| -> Result<()> {
        verify_namespace_try_hook_effects(&session, &runtime_root, try_generation_id)?;
        checkpoint_session_db(&copied_db_path)?;
        let backup = create_checkpoint(db_path, CheckpointReason::PreMutation)?;
        drop(live_conn);

        let previous_current_generation =
            conary_core::generation::mount::current_generation(runtime_root.root())?;
        let promotion_result = (|| -> Result<()> {
            replace_live_db_with_session_copy(Path::new(db_path), &copied_db_path)?;
            maybe_force_try_keep_post_backup_failure("after-db-promote")?;
            let promoted_conn = conary_core::db::open(db_path)?;
            crate::commands::composefs_ops::publish_generation_link(db_path, try_generation_id)?;
            maybe_force_try_keep_post_backup_failure("after-current-link")?;
            crate::commands::composefs_ops::mark_generation_state_active(
                &promoted_conn,
                try_generation_id,
            )?;
            let promoted_session = TrySession::find_by_id(&promoted_conn, &session.id)?
                .ok_or_else(|| anyhow::anyhow!("promoted try session {} missing", session.id))?;
            promoted_session.mark_kept(&promoted_conn)?;
            probe();
            Ok(())
        })();

        if let Err(error) = promotion_result {
            match restore_live_db_from_checkpoint(Path::new(db_path), &backup.backup_path) {
                Ok(()) => {
                    if let Err(link_error) = restore_previous_current_generation_link(
                        db_path,
                        &runtime_root,
                        previous_current_generation,
                    ) {
                        return Err(error.context(format!(
                            "try keep promotion failed after backup; restored live DB checkpoint but failed to restore current generation link: {link_error}"
                        )));
                    }
                    return Err(error.context(
                        "try keep promotion failed after backup; restored live DB checkpoint",
                    ));
                }
                Err(restore_error) => {
                    return Err(error.context(format!(
                        "try keep promotion failed after backup; failed to restore live DB checkpoint {}: {restore_error}",
                        backup.backup_path.display()
                    )));
                }
            }
        }

        Ok(())
    })();

    lock_engine.release_lock();
    result
}

fn verify_namespace_try_hook_effects(
    session: &TrySession,
    runtime_root: &ConaryRuntimeRoot,
    try_generation_id: i64,
) -> Result<()> {
    let package = <CcsPackage as PackageFormat>::parse(&session.package_path)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| {
            format!(
                "failed to read copied try package {} for keep-time hook verification",
                session.package_path
            )
        })?;
    let manifest = package.manifest();
    if !manifest.hooks.has_declarative_hooks() {
        return Ok(());
    }

    let generation_root = runtime_root.generation_path(try_generation_id);
    let etc_state_root = runtime_root
        .etc_state_dir()
        .join(try_generation_id.to_string());

    for directory in &manifest.hooks.directories {
        let relative = root_relative_path(&directory.path)?;
        let in_generation = generation_root.join(&relative);
        let in_etc_state = etc_state_root.join(&relative);
        if !in_generation.exists() && !in_etc_state.exists() {
            bail!(
                "try hook effects for {} are not present in the promotable generation root or live etc-state upperdir; run `conary try rollback`",
                directory.path
            );
        }
    }
    for group in &manifest.hooks.groups {
        if !hook_account_entry_exists(&generation_root, &etc_state_root, "etc/group", &group.name) {
            bail!(
                "try hook effects for group {} are not present in the promotable generation root or live etc-state upperdir; run `conary try rollback`",
                group.name
            );
        }
    }
    for user in &manifest.hooks.users {
        if !hook_account_entry_exists(&generation_root, &etc_state_root, "etc/passwd", &user.name) {
            bail!(
                "try hook effects for user {} are not present in the promotable generation root or live etc-state upperdir; run `conary try rollback`",
                user.name
            );
        }
    }

    Ok(())
}

fn maybe_force_try_keep_post_backup_failure(point: &str) -> Result<()> {
    #[cfg(test)]
    if let Ok(requested) = std::env::var("CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP")
        && (requested == point || requested == "1")
    {
        bail!("forced try keep failure after backup at {point}");
    }

    #[cfg(not(test))]
    {
        let _ = point;
    }

    Ok(())
}

fn restore_previous_current_generation_link(
    db_path: &str,
    runtime_root: &ConaryRuntimeRoot,
    previous_generation: Option<i64>,
) -> Result<()> {
    match previous_generation {
        Some(generation) => {
            crate::commands::composefs_ops::publish_generation_link(db_path, generation)
        }
        None => {
            let current_link = runtime_root.current_link();
            match std::fs::remove_file(&current_link) {
                Ok(()) => conary_core::filesystem::durable::sync_parent_directory(&current_link)
                    .map_err(|error| anyhow::anyhow!(error))
                    .with_context(|| {
                        format!(
                            "failed to sync parent directory after removing {}",
                            current_link.display()
                        )
                    }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error)
                    .with_context(|| format!("failed to remove {}", current_link.display())),
            }
        }
    }
}

fn restore_live_db_from_checkpoint(live_db_path: &Path, backup_path: &Path) -> Result<()> {
    let parent = live_db_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("live DB path {} has no parent", live_db_path.display()))?;
    std::fs::create_dir_all(parent)?;
    let quarantine_stamp = chrono::Utc::now()
        .format("try-restore-%Y%m%dT%H%M%SZ")
        .to_string();

    for candidate in sqlite_database_paths(live_db_path) {
        if candidate.exists() {
            let quarantined = quarantine_path(&candidate, &quarantine_stamp)?;
            std::fs::rename(&candidate, &quarantined).with_context(|| {
                format!(
                    "failed to quarantine failed promoted DB path {} to {}",
                    candidate.display(),
                    quarantined.display()
                )
            })?;
        }
    }
    remove_sqlite_sidecars(live_db_path)?;

    let restore_tmp = live_db_path.with_extension("try-restore.tmp");
    if restore_tmp.exists() {
        std::fs::remove_file(&restore_tmp)?;
    }
    std::fs::copy(backup_path, &restore_tmp).with_context(|| {
        format!(
            "failed to copy DB checkpoint {} to {}",
            backup_path.display(),
            restore_tmp.display()
        )
    })?;
    std::fs::File::open(&restore_tmp)?.sync_all()?;
    verify_sqlite_file(&restore_tmp)?;
    std::fs::rename(&restore_tmp, live_db_path).with_context(|| {
        format!(
            "failed to restore DB checkpoint {} to {}",
            backup_path.display(),
            live_db_path.display()
        )
    })?;
    conary_core::filesystem::durable::sync_parent_directory(live_db_path)?;
    let verified_conn = conary_core::db::open(live_db_path)?;
    drop(verified_conn);
    Ok(())
}

fn verify_sqlite_file(path: &Path) -> Result<()> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        bail!(
            "SQLite integrity check failed for {}: {integrity}",
            path.display()
        );
    }
    Ok(())
}

fn checkpoint_session_db(copied_db_path: &Path) -> Result<()> {
    {
        let conn = rusqlite::Connection::open(copied_db_path)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    remove_sqlite_sidecars(copied_db_path)?;
    let conn = conary_core::db::open(copied_db_path)?;
    drop(conn);
    Ok(())
}

fn replace_live_db_with_session_copy(live_db_path: &Path, copied_db_path: &Path) -> Result<()> {
    let parent = live_db_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("live DB path {} has no parent", live_db_path.display()))?;
    std::fs::create_dir_all(parent)?;
    let quarantine_stamp = chrono::Utc::now()
        .format("try-promote-%Y%m%dT%H%M%SZ")
        .to_string();

    for candidate in sqlite_database_paths(live_db_path) {
        if candidate.exists() {
            let quarantined = quarantine_path(&candidate, &quarantine_stamp)?;
            std::fs::rename(&candidate, &quarantined).with_context(|| {
                format!(
                    "failed to quarantine live DB path {} to {}",
                    candidate.display(),
                    quarantined.display()
                )
            })?;
            sync_try_db_parent_directory(&quarantined)?;
        }
    }
    remove_sqlite_sidecars(live_db_path)?;
    std::fs::rename(copied_db_path, live_db_path).with_context(|| {
        format!(
            "failed to promote try DB {} to {}",
            copied_db_path.display(),
            live_db_path.display()
        )
    })?;
    sync_try_db_parent_directory(live_db_path)?;
    Ok(())
}

fn sync_try_db_parent_directory(path: &Path) -> Result<()> {
    conary_core::filesystem::durable::sync_parent_directory(path)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("failed to sync parent directory for {}", path.display()))?;

    #[cfg(test)]
    if let Some(log_path) = std::env::var_os("CONARY_TEST_TRY_SYNC_PARENT_LOG") {
        use std::io::Write as _;

        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| {
                format!(
                    "failed to open try parent sync log {}",
                    Path::new(&log_path).display()
                )
            })?;
        writeln!(log, "{}", path.display())?;
    }

    Ok(())
}

fn vacuum_db_into(conn: &rusqlite::Connection, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    let destination_string = destination.to_string_lossy().into_owned();
    conn.execute("VACUUM main INTO ?1", [destination_string.as_str()])?;
    Ok(())
}

fn record_activated_try_boot(
    conn: &rusqlite::Connection,
    session_id: &str,
    boot_id: &str,
) -> Result<()> {
    let session = TrySession::find_by_id(conn, session_id)?
        .ok_or_else(|| anyhow::anyhow!("try session {session_id} not found"))?;
    Ok(session.record_boot_without_launcher(conn, boot_id)?)
}

pub(crate) fn current_boot_id() -> String {
    if let Ok(value) = std::env::var("CONARY_TEST_BOOT_ID") {
        return value;
    }
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown-boot".to_string())
}

fn remove_sqlite_sidecars(db_path: &Path) -> Result<()> {
    for path in [
        sqlite_sidecar_path(db_path, "-wal"),
        sqlite_sidecar_path(db_path, "-shm"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to remove SQLite sidecar {}", path.display())
                });
            }
        }
    }
    Ok(())
}

fn sqlite_database_paths(db_path: &Path) -> [PathBuf; 3] {
    [
        db_path.to_path_buf(),
        sqlite_sidecar_path(db_path, "-wal"),
        sqlite_sidecar_path(db_path, "-shm"),
    ]
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = OsString::from(db_path.as_os_str());
    path.push(suffix);
    PathBuf::from(path)
}

fn quarantine_path(path: &Path, stamp: &str) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path {} has no file name", path.display()))?
        .to_string_lossy();
    Ok(path.with_file_name(format!("{file_name}.{stamp}.old")))
}

#[cfg(test)]
pub(super) mod test_support {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::{
        AlternativeHook, CcsManifest, DirectoryHook, GroupHook, SysctlHook, SystemdHook,
        TmpfilesHook, UserHook,
    };
    use conary_core::ccs::{BuildResult, ComponentData, FileEntry, FileType};
    use conary_core::db::models::TrySession;
    use conary_core::runtime_root::ConaryRuntimeRoot;

    use super::{TryStartOutcome, TryStartRequest, begin_try_session};

    pub(super) struct TryRuntimeFixture {
        pub(super) _temp: tempfile::TempDir,
        pub(super) root: PathBuf,
        pub(super) db_path: PathBuf,
        pub(super) db_path_string: String,
    }

    impl TryRuntimeFixture {
        pub(super) fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            let db_path = root.join("conary.db");
            let db_path_string = db_path.to_string_lossy().into_owned();
            conary_core::db::init(&db_path).unwrap();
            stage_test_boot_assets(&root);
            Self {
                _temp: temp,
                root,
                db_path,
                db_path_string,
            }
        }

        pub(super) fn runtime_root(&self) -> ConaryRuntimeRoot {
            ConaryRuntimeRoot::from_db_path(self.db_path.clone())
        }

        pub(super) fn write_package(&self, name: &str, manifest: CcsManifest) -> PathBuf {
            write_try_package(self.root.join(format!("{name}.ccs")), manifest)
        }

        pub(super) fn open(&self) -> rusqlite::Connection {
            conary_core::db::open(&self.db_path).unwrap()
        }
    }

    fn stage_test_boot_assets(root: &Path) {
        let kernel_version =
            conary_core::generation::builder::detect_kernel_version_from_troves(&[])
                .unwrap_or_else(|| "test-kernel".to_string());
        let boot_root = root.join("boot");
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(
            boot_root.join(format!("vmlinuz-{kernel_version}")),
            b"test-kernel",
        )
        .unwrap();
        std::fs::write(
            boot_root.join(format!("initramfs-{kernel_version}.img")),
            b"test-initramfs",
        )
        .unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
    }

    fn write_try_package(package_path: PathBuf, manifest: CcsManifest) -> PathBuf {
        let tool_content = format!("#!/bin/sh\necho {}\n", manifest.package.name).into_bytes();
        let tool_hash = conary_core::hash::sha256(&tool_content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = conary_core::hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: format!("/usr/bin/{}", manifest.package.name),
                hash: tool_hash.clone(),
                size: tool_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let total_size = (tool_content.len() + init_content.len()) as u64;
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: total_size,
                },
            )]),
            files,
            blobs: HashMap::from([(tool_hash, tool_content), (init_hash, init_content)]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();
        package_path
    }

    pub(super) fn begin_namespace_try(
        fixture: &TryRuntimeFixture,
        package_path: &Path,
    ) -> anyhow::Result<TryStartOutcome> {
        begin_try_session(TryStartRequest {
            db_path: &fixture.db_path_string,
            package_path,
            activate: false,
            allow_irreversible: false,
            command: None,
        })
    }

    pub(super) fn begin_activated_try(
        fixture: &TryRuntimeFixture,
        package_path: &Path,
    ) -> anyhow::Result<TryStartOutcome> {
        begin_try_session(TryStartRequest {
            db_path: &fixture.db_path_string,
            package_path,
            activate: true,
            allow_irreversible: false,
            command: None,
        })
    }

    pub(super) fn stored_session(fixture: &TryRuntimeFixture, id: &str) -> TrySession {
        TrySession::find_by_id(&fixture.open(), id)
            .unwrap()
            .expect("stored try session")
    }

    pub(super) fn create_current_generation_link(root: &Path, generation: i64) {
        std::fs::create_dir_all(root.join(format!("generations/{generation}"))).unwrap();
        conary_core::generation::mount::update_current_symlink(root, generation).unwrap();
    }

    pub(super) fn has_cas_object(root: &Path) -> bool {
        let objects_dir = root.join("objects");
        if !objects_dir.exists() {
            return false;
        }
        walkdir::WalkDir::new(objects_dir)
            .into_iter()
            .filter_map(Result::ok)
            .any(|entry| {
                entry.file_type().is_file()
                    && entry.file_name() != "conary.lock"
                    && entry.metadata().map(|m| m.len() > 0).unwrap_or(false)
            })
    }

    pub(super) fn write_try_mountinfo(path: &Path, mounted_paths: &[&Path]) -> anyhow::Result<()> {
        let mut contents = String::new();
        for (index, mounted_path) in mounted_paths.iter().enumerate() {
            contents.push_str(&format!(
                "{} 1 0:{} / {} rw,relatime - overlay overlay rw\n",
                100 + index,
                100 + index,
                escape_mountinfo_path(mounted_path)
            ));
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    fn escape_mountinfo_path(path: &Path) -> String {
        path.to_string_lossy()
            .replace('\\', "\\134")
            .replace(' ', "\\040")
            .replace('\t', "\\011")
            .replace('\n', "\\012")
    }

    pub(super) struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        pub(super) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    pub(super) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(super) fn manifest_with_declarative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("declarative", "1.0.0");
        manifest.hooks.directories.push(DirectoryHook {
            path: "/var/lib/declarative".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: None,
        });
        manifest
    }

    pub(super) fn manifest_with_user_group_hooks() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("user-group-hooks", "1.0.0");
        manifest.hooks.groups.push(GroupHook {
            name: "trygroup".to_string(),
            system: true,
            reversible: None,
        });
        manifest.hooks.users.push(UserHook {
            name: "tryuser".to_string(),
            system: true,
            home: Some("/nonexistent".to_string()),
            shell: Some("/usr/sbin/nologin".to_string()),
            group: Some("trygroup".to_string()),
            reversible: None,
        });
        manifest
    }

    pub(super) fn manifest_with_systemd_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("systemd-hook", "1.0.0");
        manifest.hooks.systemd.push(SystemdHook {
            unit: "try-systemd.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
        manifest
    }

    pub(super) fn manifest_with_tmpfiles_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("tmpfiles-hook", "1.0.0");
        manifest.hooks.tmpfiles.push(TmpfilesHook {
            entry_type: "d".to_string(),
            path: "/var/lib/try-tmpfiles".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            reversible: Some(true),
        });
        manifest
    }

    pub(super) fn manifest_with_sysctl_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("sysctl-hook", "1.0.0");
        manifest.hooks.sysctl.push(SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "0".to_string(),
            only_if_lower: false,
            reversible: Some(true),
        });
        manifest
    }

    pub(super) fn manifest_with_alternative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("alternative-hook", "1.0.0");
        manifest.hooks.alternatives.push(AlternativeHook {
            name: "try-editor".to_string(),
            path: "/usr/bin/try-editor".to_string(),
            priority: 50,
            reversible: Some(true),
        });
        manifest
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use conary_core::ccs::manifest::CcsManifest;
    use conary_core::db::models::{TrySession, TrySessionMode};

    use super::test_support::*;
    use super::*;

    #[test]
    fn activated_no_command_session_records_boot_without_launcher_pid() -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 1);
        let package = fixture.write_package(
            "try-activated-no-command",
            CcsManifest::new_minimal("try-activated-no-command", "1.0.0"),
        );

        let outcome = begin_activated_try(&fixture, &package)?;

        let stored = stored_session(&fixture, &outcome.session_id);
        assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-a"));
        assert_eq!(stored.launcher_pid, None);
        Ok(())
    }

    #[test]
    fn namespace_try_start_rejects_unsupported_declarative_hook_classes_before_session()
    -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        for (manifest, expected) in [
            (manifest_with_systemd_hook(), "hooks.systemd"),
            (manifest_with_tmpfiles_hook(), "hooks.tmpfiles"),
            (manifest_with_sysctl_hook(), "hooks.sysctl"),
            (manifest_with_alternative_hook(), "hooks.alternatives"),
        ] {
            let fixture = TryRuntimeFixture::new();
            let package = fixture.write_package("try-unsupported-hook", manifest);

            let err = begin_namespace_try(&fixture, &package)
                .expect_err("unsupported declarative hook class should fail before session opens");
            let message = err.to_string();
            assert!(message.contains(expected), "{message}");
            assert!(message.contains("M2"), "{message}");
            assert!(
                TrySession::find_active_or_orphaned(&fixture.open())?.is_none(),
                "try start must fail before creating an open session"
            );
        }
        Ok(())
    }

    #[test]
    fn namespace_try_start_creates_active_session_and_copied_artifact() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let original_package =
            fixture.write_package("try-demo", CcsManifest::new_minimal("try-demo", "1.0.0"));

        let outcome = begin_namespace_try(&fixture, &original_package)?;

        let session = stored_session(&fixture, &outcome.session_id);
        assert_eq!(
            session.status,
            conary_core::db::models::TrySessionStatus::Active
        );
        assert_eq!(session.mode, TrySessionMode::Namespace);
        assert_eq!(session.package_name.as_deref(), Some("try-demo"));
        assert_eq!(session.package_version.as_deref(), Some("1.0.0"));
        assert_eq!(session.try_generation_id, Some(outcome.try_generation_id));
        assert_ne!(Path::new(&session.package_path), original_package.as_path());
        assert_eq!(
            Path::new(&session.package_path),
            outcome.copied_package_path
        );
        assert!(outcome.copied_package_path.exists());
        assert!(outcome.copied_db_path.exists());
        assert!(outcome.install_root.exists());
        assert!(outcome.work_dir.starts_with(fixture.root.join("try")));

        let copied = conary_core::db::open(&outcome.copied_db_path)?;
        let copied_session = TrySession::find_by_id(&copied, &outcome.session_id)?.unwrap();
        assert_eq!(
            copied_session.try_generation_id,
            Some(outcome.try_generation_id)
        );
        Ok(())
    }

    #[test]
    fn namespace_try_start_with_active_session_errors_with_active_id() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let first_package =
            fixture.write_package("try-first", CcsManifest::new_minimal("try-first", "1.0.0"));
        let second_package = fixture.write_package(
            "try-second",
            CcsManifest::new_minimal("try-second", "1.0.0"),
        );
        let first = begin_namespace_try(&fixture, &first_package)?;

        let err = begin_namespace_try(&fixture, &second_package)
            .expect_err("second open try session should fail");
        let message = err.to_string();
        assert!(message.contains(&first.session_id), "{message}");
        assert!(
            message.contains("active or orphaned try session"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn try_generation_build_leaves_current_link_and_writes_live_runtime_artifacts()
    -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 77);
        let before_current = std::fs::read_link(fixture.root.join("current"))?;
        let package = fixture.write_package(
            "try-artifacts",
            CcsManifest::new_minimal("try-artifacts", "1.0.0"),
        );

        let outcome = begin_namespace_try(&fixture, &package)?;

        assert_eq!(
            std::fs::read_link(fixture.root.join("current"))?,
            before_current
        );
        assert!(
            fixture
                .root
                .join(format!("generations/{}", outcome.try_generation_id))
                .join(conary_core::generation::metadata::GENERATION_METADATA_FILE)
                .exists(),
            "try generation must be built under live runtime generations/"
        );
        assert!(
            has_cas_object(&fixture.root),
            "try transaction must write CAS objects under live runtime objects/"
        );
        assert!(
            !outcome.work_dir.join("objects").exists()
                && !outcome.work_dir.join("generations").exists(),
            "throwaway work dir must not become the runtime artifact root"
        );
        Ok(())
    }

    #[test]
    fn try_transaction_config_override_keeps_live_runtime_paths_for_copied_db() {
        let fixture = TryRuntimeFixture::new();
        let work_dir = fixture.root.join("try/session-a");
        let copied_db = work_dir.join("conary.db");

        let config = build_try_transaction_config(&fixture.runtime_root(), copied_db.clone());

        assert_eq!(config.db_path, copied_db);
        assert_eq!(config.root, fixture.root);
        assert_eq!(config.objects_dir, fixture.root.join("objects"));
        assert_eq!(config.generations_dir, fixture.root.join("generations"));
        assert_eq!(config.etc_state_dir, fixture.root.join("etc-state"));
        assert_eq!(config.mount_point, fixture.root.join("mnt"));
    }

    #[test]
    fn namespace_try_install_plan_uses_scratch_root_no_scripts_and_config_override() {
        let fixture = TryRuntimeFixture::new();
        let work_dir = fixture.root.join("try/session-a");
        let copied_db = work_dir.join("conary.db");

        let plan = build_try_install_plan(
            &fixture.runtime_root(),
            &work_dir,
            copied_db.clone(),
            TrySessionMode::Namespace,
        );

        assert_eq!(plan.install_root, work_dir.join("root"));
        assert_ne!(plan.install_root, PathBuf::from("/"));
        assert!(
            plan.no_scripts,
            "namespace try installs must suppress install-time hooks"
        );
        assert_eq!(plan.transaction_config.db_path, copied_db);
        assert_eq!(
            plan.transaction_config.objects_dir,
            fixture.root.join("objects")
        );
        assert_eq!(
            plan.transaction_config.generations_dir,
            fixture.root.join("generations")
        );
    }

    #[test]
    fn activated_try_publishes_generation_records_previous_and_marks_mode() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 7);
        let package = fixture.write_package(
            "try-activated",
            CcsManifest::new_minimal("try-activated", "1.0.0"),
        );

        let outcome = begin_activated_try(&fixture, &package)?;

        let session = stored_session(&fixture, &outcome.session_id);
        assert_eq!(session.mode, TrySessionMode::Activated);
        assert_eq!(session.previous_generation_id, Some(7));
        assert_eq!(session.try_generation_id, Some(outcome.try_generation_id));
        assert_eq!(
            conary_core::generation::mount::current_generation(&fixture.root)?,
            Some(outcome.try_generation_id)
        );
        Ok(())
    }

    #[test]
    fn activated_rollback_uses_copied_package_after_original_is_deleted() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 5);
        let package = fixture.write_package(
            "try-rollback-activated",
            CcsManifest::new_minimal("try-rollback-activated", "1.0.0"),
        );
        let outcome = begin_activated_try(&fixture, &package)?;
        std::fs::remove_file(&package)?;

        rollback_active_try_session(&fixture.db_path_string)?;

        let session = stored_session(&fixture, &outcome.session_id);
        assert_eq!(
            session.status,
            conary_core::db::models::TrySessionStatus::RolledBack
        );
        assert_eq!(
            conary_core::generation::mount::current_generation(&fixture.root)?,
            Some(5)
        );
        assert!(
            !outcome.work_dir.exists(),
            "rollback must remove try work dir"
        );
        Ok(())
    }

    #[test]
    fn namespace_rollback_marks_rolled_back_and_removes_work_dir() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 2);
        let package = fixture.write_package(
            "try-rollback",
            CcsManifest::new_minimal("try-rollback", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;

        rollback_active_try_session(&fixture.db_path_string)?;

        let session = stored_session(&fixture, &outcome.session_id);
        assert_eq!(
            session.status,
            conary_core::db::models::TrySessionStatus::RolledBack
        );
        assert!(
            !outcome.work_dir.exists(),
            "rollback must remove try work dir"
        );
        assert!(
            !fixture
                .root
                .join(format!("generations/{}", outcome.try_generation_id))
                .exists(),
            "unkept inactive try generation should be removed"
        );
        assert_eq!(
            conary_core::generation::mount::current_generation(&fixture.root)?,
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn namespace_rollback_leaves_session_retryable_when_work_dir_removal_fails()
    -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 2);
        let package = fixture.write_package(
            "try-rollback-workdir-fail",
            CcsManifest::new_minimal("try-rollback-workdir-fail", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;
        let _fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_REMOVE_DIR_FAIL", &outcome.work_dir);

        let err = rollback_active_try_session(&fixture.db_path_string).expect_err(
            "rollback should fail before marking rolled_back when work dir cleanup fails",
        );
        let message = format!("{err:#}");
        assert!(
            message.contains("forced try directory removal failure"),
            "{message}"
        );
        assert!(
            message.contains(&outcome.work_dir.display().to_string()),
            "{message}"
        );
        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::Active
        );
        assert!(
            outcome.work_dir.exists(),
            "failed cleanup must leave work dir for retry"
        );
        Ok(())
    }

    #[test]
    fn namespace_keep_publishes_try_generation_and_marks_kept() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let package =
            fixture.write_package("try-keep", CcsManifest::new_minimal("try-keep", "1.0.0"));
        let outcome = begin_namespace_try(&fixture, &package)?;

        keep_active_try_session(&fixture.db_path_string)?;

        let session = stored_session(&fixture, &outcome.session_id);
        assert_eq!(
            session.status,
            conary_core::db::models::TrySessionStatus::Kept
        );
        assert_eq!(
            conary_core::generation::mount::current_generation(&fixture.root)?,
            Some(outcome.try_generation_id)
        );
        let installed: String = fixture.open().query_row(
            "SELECT name FROM troves WHERE name = 'try-keep'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(installed, "try-keep");
        Ok(())
    }

    #[test]
    fn namespace_keep_removes_stale_sidecars_before_promoted_db_reopen() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package(
            "try-keep-sidecars",
            CcsManifest::new_minimal("try-keep-sidecars", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;
        std::fs::write(sqlite_sidecar_path(&outcome.copied_db_path, "-wal"), b"")?;
        std::fs::write(sqlite_sidecar_path(&outcome.copied_db_path, "-shm"), b"")?;
        std::fs::write(sqlite_sidecar_path(&fixture.db_path, "-wal"), b"")?;
        std::fs::write(sqlite_sidecar_path(&fixture.db_path, "-shm"), b"")?;

        keep_active_try_session(&fixture.db_path_string)?;

        assert!(!sqlite_sidecar_path(&outcome.copied_db_path, "-wal").exists());
        assert!(!sqlite_sidecar_path(&outcome.copied_db_path, "-shm").exists());
        assert!(!sqlite_sidecar_path(&fixture.db_path, "-wal").exists());
        assert!(!sqlite_sidecar_path(&fixture.db_path, "-shm").exists());
        assert!(conary_core::db::open(&fixture.db_path).is_ok());
        Ok(())
    }

    #[test]
    fn db_promotion_syncs_parent_after_quarantine_and_final_rename() -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir()?;
        let live_db = temp.path().join("conary.db");
        let copied_db = temp.path().join("try/conary.db");
        let sync_log = temp.path().join("sync-parent.log");
        std::fs::create_dir_all(copied_db.parent().unwrap())?;
        std::fs::write(&live_db, b"live")?;
        std::fs::write(sqlite_sidecar_path(&live_db, "-wal"), b"wal")?;
        std::fs::write(sqlite_sidecar_path(&live_db, "-shm"), b"shm")?;
        std::fs::write(&copied_db, b"copy")?;
        let _sync_guard = EnvVarGuard::set("CONARY_TEST_TRY_SYNC_PARENT_LOG", &sync_log);

        replace_live_db_with_session_copy(&live_db, &copied_db)?;

        assert_eq!(std::fs::read(&live_db)?, b"copy");
        assert!(!copied_db.exists());
        let synced = std::fs::read_to_string(sync_log)?
            .lines()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        assert_eq!(synced.len(), 4, "{synced:?}");
        let synced_names = synced
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(synced_names[0].starts_with("conary.db.try-promote-"));
        assert!(synced_names[0].ends_with(".old"));
        assert!(synced_names[1].starts_with("conary.db-wal.try-promote-"));
        assert!(synced_names[1].ends_with(".old"));
        assert!(synced_names[2].starts_with("conary.db-shm.try-promote-"));
        assert!(synced_names[2].ends_with(".old"));
        assert_eq!(synced[3], live_db);
        Ok(())
    }

    #[test]
    fn namespace_keep_holds_runtime_lock_until_session_is_marked() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package(
            "try-keep-lock",
            CcsManifest::new_minimal("try-keep-lock", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;

        keep_active_try_session_with_probe(&fixture.db_path_string, || {
            let mut config =
                build_try_transaction_config(&fixture.runtime_root(), fixture.db_path.clone());
            config.lock_timeout_secs = 0;
            let mut engine = TransactionEngine::new(config).unwrap();
            assert!(
                engine.begin().is_err(),
                "namespace keep must still hold the live runtime lock while marking the session"
            );
        })?;

        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::Kept
        );
        Ok(())
    }

    #[test]
    fn activated_keep_holds_runtime_lock_while_marking_kept() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 11);
        let package = fixture.write_package(
            "try-activated-keep-lock",
            CcsManifest::new_minimal("try-activated-keep-lock", "1.0.0"),
        );
        let outcome = begin_activated_try(&fixture, &package)?;

        keep_active_try_session_with_probe(&fixture.db_path_string, || {
            let mut config =
                build_try_transaction_config(&fixture.runtime_root(), fixture.db_path.clone());
            config.lock_timeout_secs = 0;
            let mut engine = TransactionEngine::new(config).unwrap();
            assert!(
                engine.begin().is_err(),
                "activated keep must hold the runtime mutation lock while marking the session"
            );
        })?;

        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::Kept
        );
        Ok(())
    }

    #[test]
    fn namespace_keep_restores_live_db_after_post_backup_failure() -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _fail_guard =
            EnvVarGuard::set("CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP", "after-db-promote");
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package(
            "try-restore-live-db",
            CcsManifest::new_minimal("try-restore-live-db", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;

        let err = keep_active_try_session(&fixture.db_path_string)
            .expect_err("forced post-backup failure should abort keep");
        let error_chain = format!("{err:#}");
        assert!(
            error_chain.contains("forced try keep failure"),
            "{error_chain}"
        );
        assert!(
            error_chain.contains("restored live DB checkpoint"),
            "{error_chain}"
        );

        let conn = fixture.open();
        let installed_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'try-restore-live-db'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(installed_count, 0, "live DB must be restored from backup");
        let session = TrySession::find_by_id(&conn, &outcome.session_id)?.unwrap();
        assert_eq!(
            session.status,
            conary_core::db::models::TrySessionStatus::Active
        );
        Ok(())
    }

    #[test]
    fn namespace_keep_restores_current_link_after_post_link_failure() -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _fail_guard = EnvVarGuard::set(
            "CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP",
            "after-current-link",
        );
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 7);
        let package = fixture.write_package(
            "try-restore-current-link",
            CcsManifest::new_minimal("try-restore-current-link", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;
        assert_ne!(
            conary_core::generation::mount::current_generation(&fixture.root)?,
            Some(outcome.try_generation_id)
        );

        let err = keep_active_try_session(&fixture.db_path_string)
            .expect_err("forced post-link failure should abort keep");
        let error_chain = format!("{err:#}");
        assert!(
            error_chain.contains("forced try keep failure"),
            "{error_chain}"
        );
        assert!(
            error_chain.contains("restored live DB checkpoint"),
            "{error_chain}"
        );

        assert_eq!(
            conary_core::generation::mount::current_generation(&fixture.root)?,
            Some(7),
            "current generation link must be restored after post-link keep failure"
        );
        let conn = fixture.open();
        let installed_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'try-restore-current-link'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(installed_count, 0);
        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::Active
        );
        Ok(())
    }

    #[test]
    fn namespace_keep_fails_when_declarative_hook_effect_is_not_promotable() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package("try-hook-verify", manifest_with_declarative_hook());
        let outcome = begin_namespace_try(&fixture, &package)?;
        std::fs::remove_dir_all(fixture.root.join(format!(
            "etc-state/{}/var/lib/declarative",
            outcome.try_generation_id
        )))?;

        let err = keep_active_try_session(&fixture.db_path_string)
            .expect_err("keep should reject missing promotable hook effects");
        let message = err.to_string();
        assert!(message.contains("hook effects"), "{message}");
        assert!(message.contains("rollback"), "{message}");
        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::Active
        );
        Ok(())
    }

    #[test]
    fn keep_time_hook_verification_checks_user_group_effects() -> anyhow::Result<()> {
        let fixture = TryRuntimeFixture::new();
        let runtime_root = fixture.runtime_root();
        let package =
            fixture.write_package("try-user-group-verify", manifest_with_user_group_hooks());
        let package_path = package.to_string_lossy().into_owned();
        let session = TrySession {
            id: "try-user-group-session".to_string(),
            package_path,
            package_name: Some("user-group-hooks".to_string()),
            package_version: Some("1.0.0".to_string()),
            previous_generation_id: None,
            try_generation_id: Some(42),
            launcher_pid: None,
            launcher_boot_id: None,
            status: conary_core::db::models::TrySessionStatus::Active,
            mode: TrySessionMode::Namespace,
            work_dir: fixture.root.join("try/work").to_string_lossy().into_owned(),
            last_error: None,
            started_at: None,
            updated_at: None,
            completed_at: None,
        };

        let err = verify_namespace_try_hook_effects(&session, &runtime_root, 42)
            .expect_err("missing user/group hook effects should fail keep verification");
        let message = err.to_string();
        assert!(message.contains("hook effects"), "{message}");
        assert!(message.contains("rollback"), "{message}");

        let etc = fixture.root.join("etc-state/42/etc");
        std::fs::create_dir_all(&etc)?;
        std::fs::write(etc.join("group"), "trygroup:x:999:\n")?;
        std::fs::write(
            etc.join("passwd"),
            "tryuser:x:999:999::/nonexistent:/usr/sbin/nologin\n",
        )?;

        verify_namespace_try_hook_effects(&session, &runtime_root, 42)?;
        Ok(())
    }
}
