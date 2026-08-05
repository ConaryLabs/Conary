// apps/conary/src/commands/try_session/session.rs
//! Try-session lifecycle orchestration and liveness policy.

mod watch_marker;

use anyhow::{Context, Result, bail};
use conary_core::ccs::verify::TrustPolicy;
use conary_core::db::backup::{CheckpointReason, create_checkpoint};
use conary_core::db::models::{CreateTrySession, SystemState, TrySession, TrySessionMode};
use conary_core::packages::traits::PackageFormat;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::executor::run_try_command_for_session;
use super::install::{build_try_install_plan, build_try_transaction_config, install_try_package};
use super::namespace::{
    self, apply_declarative_try_hooks, expose_try_namespace_root, hook_account_entry_exists,
    promotable_try_hook_root, root_relative_path, teardown_try_namespace_mounts,
};
use super::package_verification::load_verified_try_package;
use super::util::{remove_dir_if_exists, remove_path_if_exists};
use super::validation::{TryExecutionRoot, validate_try_package_policy};
use super::{
    TryRefreshOutcome, TryRefreshRequest, TryRefreshSessionDiverged, TryStartOutcome,
    TryStartRequest,
};
use watch_marker::{is_watch_created_try_session, write_try_watch_marker};

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
    let install_root = work_dir.join("selected-root-session/root");
    let copied_package_path = work_dir.join("package.ccs");
    let copied_db_path = work_dir.join("conary.db");
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create try work directory {}", work_dir.display()))?;
    std::fs::copy(request.package_path, &copied_package_path).with_context(|| {
        format!(
            "failed to copy try package {} to {}",
            request.package_path.display(),
            copied_package_path.display()
        )
    })?;

    let copied_package_path_string = copied_package_path.to_string_lossy().into_owned();
    let verified =
        load_verified_try_package(&copied_package_path, request.trust_policy, "try package")?;
    let package = verified.package;
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
    validate_try_package_policy(&package, execution_root, request.activate)?;
    if let Some(marker) = request.watch_marker {
        write_try_watch_marker(&work_dir, marker)?;
    }

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
            package_signing_key: &verified.signing_key,
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
    let captured_root = install_try_package(&mut copied_conn, &package, &install_plan)?;

    let summary = format!("Try {}-{}", package.name(), package.version());
    let built = crate::commands::composefs_ops::build_inactive_generation_for_runtime(
        &copied_conn,
        &runtime_root,
        &summary,
        captured_root,
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

pub(crate) fn refresh_try_session(request: TryRefreshRequest<'_>) -> Result<TryRefreshOutcome> {
    let mut refresh_committed = false;
    let mut refresh_dir: Option<PathBuf> = None;
    let mut staged_namespace_cleanup: Option<namespace::StagedNamespaceExposure> = None;
    let result = (|| -> Result<TryRefreshOutcome> {
        let live_conn = conary_core::db::open(request.db_path)
            .with_context(|| format!("failed to open Conary DB {}", request.db_path))?;
        let session = TrySession::find_by_id(&live_conn, request.session_id)?.ok_or_else(|| {
            refresh_session_diverged(request.session_id, "try watch session is missing")
        })?;
        if session.status != conary_core::db::models::TrySessionStatus::Active {
            return Err(refresh_session_diverged(
                request.session_id,
                "try watch session is no longer active",
            ));
        }
        if session.try_generation_id != Some(request.expected_try_generation_id) {
            return Err(refresh_session_diverged(
                request.session_id,
                "try watch session generation no longer matches the watcher",
            ));
        }
        if session.mode != TrySessionMode::Namespace {
            return Err(refresh_session_diverged(
                request.session_id,
                "try watch session is no longer a namespace session",
            ));
        }
        if !is_watch_created_try_session(&session) {
            return Err(refresh_session_diverged(
                request.session_id,
                "try watch session no longer has watcher-owned identity",
            ));
        }

        let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(request.db_path));
        let work_dir = PathBuf::from(&session.work_dir);
        let staging_dir =
            namespace::refresh_staging_dir(&work_dir, request.expected_try_generation_id + 1);
        refresh_dir = Some(staging_dir.clone());
        let copied_package_path = staging_dir.join("package.ccs");
        let copied_db_path = staging_dir.join("conary.db");
        std::fs::create_dir_all(&staging_dir)?;
        std::fs::copy(request.package_path, &copied_package_path)?;

        let verified = load_verified_try_package(
            &copied_package_path,
            request.trust_policy,
            "refreshed try package",
        )?;
        let package = verified.package;
        validate_try_package_policy(&package, TryExecutionRoot::Namespace, false)?;

        vacuum_db_into(&live_conn, &copied_db_path)?;
        let mut copied_conn = conary_core::db::open(&copied_db_path)?;
        ensure_staging_db_generation_floor(
            &copied_conn,
            request.expected_try_generation_id,
            &session,
        )?;
        let install_plan = build_try_install_plan(
            &runtime_root,
            &staging_dir,
            copied_db_path.clone(),
            TrySessionMode::Namespace,
        );
        let captured_root = install_try_package(&mut copied_conn, &package, &install_plan)?;

        let summary = format!("Try {}-{}", package.name(), package.version());
        let built = crate::commands::composefs_ops::build_inactive_generation_for_runtime(
            &copied_conn,
            &runtime_root,
            &summary,
            captured_root,
        )?;
        let hook_upperdir = promotable_try_hook_root(&runtime_root, built.generation_number)?;
        let staged_namespace = namespace::expose_staged_try_namespace_root(
            &runtime_root,
            &work_dir,
            &copied_conn,
            built.generation_number,
            &hook_upperdir,
        )?;
        staged_namespace_cleanup = Some(staged_namespace.clone());
        apply_declarative_try_hooks(package.manifest(), &staged_namespace.next_namespace_root)?;

        let stable_package_path = work_dir.join("package.ccs");
        let stable_db_path = work_dir.join("conary.db");
        let stable_package_path_string = stable_package_path.to_string_lossy().into_owned();
        let copied_session =
            TrySession::find_by_id(&copied_conn, request.session_id)?.ok_or_else(|| {
                refresh_session_diverged(request.session_id, "copied try watch session is missing")
            })?;
        if !copied_session.replace_active_try_generation(
            &copied_conn,
            request.expected_try_generation_id,
            &stable_package_path_string,
            &verified.signing_key,
            built.generation_number,
        )? {
            return Err(refresh_session_diverged(
                request.session_id,
                "copied try watch session generation no longer matches the watcher",
            ));
        }

        let prepared_files = prepare_stable_try_files(
            &stable_package_path,
            &stable_db_path,
            &copied_package_path,
            &copied_db_path,
        )?;

        let stable_namespace_root = work_dir.join("namespace-root");
        let namespace_switch = namespace::switch_stable_namespace_root(
            staged_namespace.clone(),
            request.expected_try_generation_id,
        )?;
        let file_switch = match prepared_files.switch() {
            Ok(file_switch) => file_switch,
            Err(error) => {
                let _ = namespace_switch.restore();
                return Err(error
                    .context("failed to switch stable try package files after namespace switch"));
            }
        };
        let replaced = session.replace_active_try_generation(
            &live_conn,
            request.expected_try_generation_id,
            &stable_package_path_string,
            &verified.signing_key,
            built.generation_number,
        )?;
        if !replaced {
            let _ = namespace_switch.restore();
            let _ = file_switch.restore();
            return Err(refresh_session_diverged(
                request.session_id,
                "live try watch session generation no longer matches the watcher",
            ));
        }

        refresh_committed = true;
        staged_namespace_cleanup = None;
        let mut cleanup_errors = Vec::new();
        if let Err(error) = file_switch.commit() {
            cleanup_errors.push(format!(
                "failed to clean previous stable try files: {error:#}"
            ));
        }
        if let Err(error) = namespace_switch.commit() {
            cleanup_errors.push(format!("failed to clean previous try namespace: {error:#}"));
        }
        if let Err(error) = remove_dir_if_exists(staging_dir) {
            cleanup_errors.push(format!(
                "failed to clean refresh staging directory: {error:#}"
            ));
        }
        let cleanup_error = (!cleanup_errors.is_empty()).then(|| cleanup_errors.join("; "));

        Ok(TryRefreshOutcome {
            previous_generation_id: request.expected_try_generation_id,
            try_generation_id: built.generation_number,
            namespace_root: stable_namespace_root,
            copied_package_path: stable_package_path,
            cleanup_error,
        })
    })();

    if result.is_err() && !refresh_committed {
        if let Some(staged_namespace) = &staged_namespace_cleanup {
            let _ = namespace::teardown_staged_namespace_exposure(staged_namespace);
        }
        if let Some(staging_dir) = refresh_dir {
            remove_dir_if_exists(staging_dir)?;
        }
    }
    result
}

fn refresh_session_diverged(session_id: &str, diagnostic: &'static str) -> anyhow::Error {
    anyhow::Error::new(TryRefreshSessionDiverged::new(session_id)).context(diagnostic)
}

fn ensure_staging_db_generation_floor(
    conn: &rusqlite::Connection,
    expected_try_generation_id: i64,
    session: &TrySession,
) -> Result<()> {
    if SystemState::find_by_number(conn, expected_try_generation_id)?.is_some() {
        return Ok(());
    }
    let mut state = SystemState::new(
        expected_try_generation_id,
        format!("Previous try generation for {}", session.id),
    );
    state.insert(conn)?;
    Ok(())
}

struct PreparedStableTryFiles {
    package_tmp: PathBuf,
    package_path: PathBuf,
    package_backup: PathBuf,
    db_tmp: PathBuf,
    db_path: PathBuf,
    db_backup: PathBuf,
}

struct StableTryFileSwitch {
    package_path: PathBuf,
    package_backup: PathBuf,
    db_path: PathBuf,
    db_backup: PathBuf,
}

impl StableTryFileSwitch {
    fn commit(self) -> Result<()> {
        remove_path_if_exists(&self.package_backup)?;
        remove_path_if_exists(&self.db_backup)?;
        Ok(())
    }

    fn restore(self) -> Result<()> {
        remove_path_if_exists(&self.package_path)?;
        remove_path_if_exists(&self.db_path)?;
        if self.package_backup.exists() {
            std::fs::rename(&self.package_backup, &self.package_path)?;
        }
        if self.db_backup.exists() {
            std::fs::rename(&self.db_backup, &self.db_path)?;
        }
        Ok(())
    }
}

impl PreparedStableTryFiles {
    fn switch(self) -> Result<StableTryFileSwitch> {
        if self.package_path.exists() {
            std::fs::rename(&self.package_path, &self.package_backup)?;
        }
        if self.db_path.exists() {
            std::fs::rename(&self.db_path, &self.db_backup)?;
        }
        if let Err(error) = std::fs::rename(&self.package_tmp, &self.package_path) {
            let _ = restore_prepared_try_files(&self);
            anyhow::bail!("failed to publish stable try package: {error}");
        }
        if let Err(error) = std::fs::rename(&self.db_tmp, &self.db_path) {
            let _ = restore_prepared_try_files(&self);
            anyhow::bail!("failed to publish stable try DB: {error}");
        }

        Ok(StableTryFileSwitch {
            package_path: self.package_path,
            package_backup: self.package_backup,
            db_path: self.db_path,
            db_backup: self.db_backup,
        })
    }
}

fn prepare_stable_try_files(
    stable_package_path: &Path,
    stable_db_path: &Path,
    staged_package_path: &Path,
    staged_db_path: &Path,
) -> Result<PreparedStableTryFiles> {
    let switch_id = uuid::Uuid::new_v4();
    let package_tmp = stable_package_path.with_extension(format!("{switch_id}.ccs.next"));
    let db_tmp = stable_db_path.with_extension(format!("{switch_id}.db.next"));
    let package_backup = stable_package_path.with_extension(format!("{switch_id}.ccs.previous"));
    let db_backup = stable_db_path.with_extension(format!("{switch_id}.db.previous"));

    std::fs::copy(staged_package_path, &package_tmp)?;
    std::fs::copy(staged_db_path, &db_tmp)?;

    Ok(PreparedStableTryFiles {
        package_tmp,
        package_path: stable_package_path.to_path_buf(),
        package_backup,
        db_tmp,
        db_path: stable_db_path.to_path_buf(),
        db_backup,
    })
}

fn restore_prepared_try_files(files: &PreparedStableTryFiles) -> Result<()> {
    remove_path_if_exists(&files.package_path)?;
    remove_path_if_exists(&files.db_path)?;
    if files.package_backup.exists() {
        std::fs::rename(&files.package_backup, &files.package_path)?;
    }
    if files.db_backup.exists() {
        std::fs::rename(&files.db_backup, &files.db_path)?;
    }
    remove_path_if_exists(&files.package_tmp)?;
    remove_path_if_exists(&files.db_tmp)?;
    Ok(())
}

pub(crate) fn rollback_active_try_session(db_path: &str) -> Result<()> {
    let live_conn = conary_core::db::open(db_path)?;
    let session = TrySession::find_active_or_orphaned(&live_conn)?
        .ok_or_else(|| anyhow::anyhow!("no active or orphaned try session found"))?;
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let work_dir = PathBuf::from(&session.work_dir);

    if session.mode == TrySessionMode::Activated {
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

pub(super) fn keep_active_try_session(db_path: &str) -> Result<()> {
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
    if is_watch_created_try_session(&session) {
        bail!(
            "cannot keep watch-created try session {}; stop watch or run `conary try rollback`",
            session.id
        );
    }

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
            let db_restore_result =
                restore_live_db_from_checkpoint(Path::new(db_path), &backup.backup_path);
            let link_restore_result = restore_previous_current_generation_link(
                db_path,
                &runtime_root,
                previous_current_generation,
            );
            match (db_restore_result, link_restore_result) {
                (Ok(()), Ok(())) => {
                    return Err(error.context(
                        "try keep promotion failed after backup; restored live DB checkpoint and current generation link",
                    ));
                }
                (Ok(()), Err(link_error)) => {
                    return Err(error.context(format!(
                        "try keep promotion failed after backup; restored live DB checkpoint but failed to restore current generation link: {link_error}"
                    )));
                }
                (Err(restore_error), Ok(())) => {
                    return Err(error.context(format!(
                        "try keep promotion failed after backup; failed to restore live DB checkpoint {}: {restore_error}; restored current generation link",
                        backup.backup_path.display()
                    )));
                }
                (Err(restore_error), Err(link_error)) => {
                    return Err(error.context(format!(
                        "try keep promotion failed after backup; failed to restore live DB checkpoint {}: {restore_error}; failed to restore current generation link: {link_error}",
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
    let trust_policy = TrustPolicy::strict(vec![session.package_signing_key.clone()]);
    let package_path = Path::new(&session.package_path);
    let verified = load_verified_try_package(
        package_path,
        &trust_policy,
        "copied try package for keep-time hook verification",
    )?;
    let package = verified.package;
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
    #[cfg(test)]
    if std::env::var("CONARY_TEST_TRY_RESTORE_DB_FAIL").as_deref() == Ok("1") {
        bail!("forced try DB checkpoint restore failure");
    }

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
    let promote_tmp = live_db_path.with_extension("try-promote.tmp");
    if promote_tmp.exists() {
        std::fs::remove_file(&promote_tmp)
            .with_context(|| format!("failed to remove {}", promote_tmp.display()))?;
    }
    std::fs::copy(copied_db_path, &promote_tmp).with_context(|| {
        format!(
            "failed to copy try DB {} to promotion temp {}",
            copied_db_path.display(),
            promote_tmp.display()
        )
    })?;
    std::fs::File::open(&promote_tmp)?.sync_all()?;
    std::fs::rename(&promote_tmp, live_db_path).with_context(|| {
        format!(
            "failed to promote temp DB {} to {}",
            promote_tmp.display(),
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

pub(super) fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
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

pub(crate) fn namespace_try_session_is_decision_pending(
    session: &TrySession,
    current_boot_id: &str,
) -> bool {
    if session
        .launcher_boot_id
        .as_deref()
        .is_some_and(|boot_id| boot_id != current_boot_id)
    {
        return false;
    }

    session.launcher_pid.is_none_or(try_launcher_pid_is_alive)
}

pub(crate) fn activated_try_session_is_live(
    session: &TrySession,
    current_boot_id: &str,
    current_generation: Option<i64>,
) -> bool {
    session.launcher_boot_id.as_deref() == Some(current_boot_id)
        && session.try_generation_id.is_some()
        && current_generation == session.try_generation_id
        && session.launcher_pid.is_none_or(try_launcher_pid_is_alive)
}

fn try_launcher_pid_is_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;
