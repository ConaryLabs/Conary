// src/commands/try_session.rs
//! Try-session policy helpers.

use anyhow::{Context, Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::manifest::{CcsManifest, HookExecutionRoot};
use conary_core::db::backup::{CheckpointReason, create_checkpoint};
use conary_core::db::models::{CreateTrySession, TrySession, TrySessionMode};
use conary_core::packages::traits::PackageFormat;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::commands::install::{
    CcsTransactionInstallOptions, ComponentSelection, LegacyReplayOptions,
    install_ccs_package_transactionally_with_config,
};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TryExecutionRoot {
    Namespace,
    Generation,
    Host,
}

impl TryExecutionRoot {
    fn hook_execution_root(self) -> HookExecutionRoot {
        match self {
            Self::Namespace => HookExecutionRoot::TryRoot,
            Self::Generation => HookExecutionRoot::GenerationRoot,
            Self::Host => HookExecutionRoot::HostRoot,
        }
    }
}

#[allow(dead_code)]
pub(crate) fn validate_try_package_policy(
    package: &CcsPackage,
    execution_root: TryExecutionRoot,
    allow_irreversible: bool,
    activated: bool,
) -> Result<()> {
    validate_try_manifest_policy(
        package.manifest(),
        execution_root,
        allow_irreversible,
        activated,
    )
}

#[allow(dead_code)]
pub(crate) fn validate_try_manifest_policy(
    manifest: &CcsManifest,
    execution_root: TryExecutionRoot,
    allow_irreversible: bool,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;

    if hooks.has_script_hooks() {
        bail!("{}", script_hook_policy_error(activated));
    }

    if manifest.legacy_scriptlets.is_some() {
        if activated {
            bail!(
                "legacy scriptlet bundles are not supported in activated M1b try sessions; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!(
            "legacy scriptlet bundles are not supported in M1b try sessions; \
             replay against try roots requires a reviewed lifecycle helper"
        );
    }

    if hooks.has_service_hooks() {
        if activated {
            bail!(
                "service lifecycle is not generation-scoped in activated M1b try sessions; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!(
            "service lifecycle is not generation-scoped in M1b try sessions; \
            hooks.services cannot run during try"
        );
    }

    validate_m1b_try_declarative_hook_support(manifest, activated)?;

    if matches!(execution_root, TryExecutionRoot::Host) && hooks.has_declarative_hooks() {
        if activated {
            bail!(
                "try hooks cannot execute against the host root; \
                 host-root lifecycle helper is M2 work"
            );
        }
        bail!("try hooks cannot execute against the host root");
    }

    if hooks.has_irreversible_hooks_for_try_root(execution_root.hook_execution_root())
        && !allow_irreversible
    {
        bail!(
            "try package contains irreversible hooks for the planned execution root; \
             pass --allow-irreversible only after review"
        );
    }

    Ok(())
}

fn validate_m1b_try_declarative_hook_support(
    manifest: &CcsManifest,
    activated: bool,
) -> Result<()> {
    let hooks = &manifest.hooks;
    if !hooks.systemd.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.systemd", activated)
        );
    }
    if !hooks.tmpfiles.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.tmpfiles", activated)
        );
    }
    if !hooks.sysctl.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.sysctl", activated)
        );
    }
    if !hooks.alternatives.is_empty() {
        bail!(
            "{}",
            unsupported_declarative_hook_error("hooks.alternatives", activated)
        );
    }
    Ok(())
}

fn unsupported_declarative_hook_error(hook_class: &str, activated: bool) -> String {
    if activated {
        format!(
            "{hook_class} are not supported in activated M1b try sessions; \
             generation-scoped effect verification for this hook class is M2 work"
        )
    } else {
        format!(
            "{hook_class} are not supported in M1b try sessions; \
             promotable try-root effect verification for this hook class is M2 work"
        )
    }
}

fn script_hook_policy_error(activated: bool) -> &'static str {
    if activated {
        "script hooks are not supported in activated M1b try sessions; \
         host-root lifecycle helper is M2 work"
    } else {
        "script hooks are not supported in M1b try sessions; \
         scripts cannot run against the host root"
    }
}

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

#[derive(Debug, Clone)]
pub(crate) struct TryInstallPlan {
    pub install_root: PathBuf,
    pub copied_db_path: PathBuf,
    pub transaction_config: TransactionConfig,
    pub no_scripts: bool,
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

fn install_try_package(
    conn: &mut rusqlite::Connection,
    package: &CcsPackage,
    plan: &TryInstallPlan,
) -> Result<()> {
    let db_path_string = plan.copied_db_path.to_string_lossy().into_owned();
    let root_string = plan.install_root.to_string_lossy().into_owned();
    install_ccs_package_transactionally_with_config(
        conn,
        package,
        CcsTransactionInstallOptions {
            db_path: &db_path_string,
            root: &root_string,
            dry_run: false,
            defer_generation: true,
            no_scripts: plan.no_scripts,
            sandbox_mode: conary_core::scriptlet::SandboxMode::None,
            allow_downgrade: false,
            reinstall: false,
            selection_reason: Some("conary try"),
            component_selection: ComponentSelection::All,
            selected_manifest_components: None,
            repository_provenance: None,
            legacy_replay: LegacyReplayOptions::default(),
        },
        plan.transaction_config.clone(),
    )?;
    Ok(())
}

pub(crate) fn build_try_install_plan(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_db_path: PathBuf,
    _mode: TrySessionMode,
) -> TryInstallPlan {
    TryInstallPlan {
        install_root: work_dir.join("root"),
        copied_db_path: copied_db_path.clone(),
        transaction_config: build_try_transaction_config(runtime_root, copied_db_path),
        no_scripts: true,
    }
}

pub(crate) fn build_try_transaction_config(
    runtime_root: &ConaryRuntimeRoot,
    copied_db_path: PathBuf,
) -> TransactionConfig {
    TransactionConfig {
        root: runtime_root.root().to_path_buf(),
        db_path: copied_db_path,
        objects_dir: runtime_root.objects_dir(),
        generations_dir: runtime_root.generations_dir(),
        etc_state_dir: runtime_root.etc_state_dir(),
        mount_point: runtime_root.mount_dir(),
        hash_algorithm: conary_core::hash::HashAlgorithm::Sha256,
        lock_timeout_secs: TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS,
    }
}

pub(crate) fn apply_declarative_try_hooks(manifest: &CcsManifest, root: &Path) -> Result<()> {
    if root == Path::new("/") {
        bail!("refusing to execute try hooks against the host root");
    }
    if !manifest.hooks.has_declarative_hooks() {
        return Ok(());
    }

    let mut executor = conary_core::ccs::HookExecutor::new(root);
    executor
        .execute_pre_hooks(&manifest.hooks)
        .context("failed to execute try declarative pre-hooks")?;
    let results = executor.execute_post_hooks_with_results(&manifest.hooks);
    let failures = results
        .failures()
        .map(|failure| {
            format!(
                "{} '{}' failed: {}",
                failure.hook_type,
                failure.name,
                failure.error.as_deref().unwrap_or("unknown error")
            )
        })
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        bail!(
            "failed to execute try declarative post-hooks: {}",
            failures.join("; ")
        );
    }
    Ok(())
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

        let promotion_result = (|| -> Result<()> {
            replace_live_db_with_session_copy(Path::new(db_path), &copied_db_path)?;
            maybe_force_try_keep_post_backup_failure("after-db-promote")?;
            let promoted_conn = conary_core::db::open(db_path)?;
            crate::commands::composefs_ops::publish_generation_link(db_path, try_generation_id)?;
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
        let relative = hook_effect_relative_path(&directory.path)?;
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

fn hook_effect_relative_path(path: &str) -> Result<PathBuf> {
    root_relative_path(path)
}

fn root_relative_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix("/").unwrap_or(path)
    } else {
        path
    };
    if relative.as_os_str().is_empty() {
        bail!("empty try root path {path:?}");
    }
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("unsafe try hook effects path {path:?}");
    }
    Ok(relative.to_path_buf())
}

fn hook_account_entry_exists(
    generation_root: &Path,
    etc_state_root: &Path,
    relative_file: &str,
    name: &str,
) -> bool {
    [generation_root, etc_state_root]
        .iter()
        .any(|root| passwd_like_file_contains_name(&root.join(relative_file), name))
}

fn passwd_like_file_contains_name(path: &Path, name: &str) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    contents
        .lines()
        .any(|line| line.split(':').next() == Some(name))
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

fn promotable_try_hook_root(
    runtime_root: &ConaryRuntimeRoot,
    try_generation_id: i64,
) -> Result<PathBuf> {
    let root = runtime_root
        .etc_state_dir()
        .join(try_generation_id.to_string());
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create try hook root {}", root.display()))?;
    Ok(root)
}

fn expose_try_namespace_root(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_conn: &rusqlite::Connection,
    try_generation_id: i64,
    hook_upperdir: &Path,
) -> Result<PathBuf> {
    let namespace_root = work_dir.join("namespace-root");
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        materialize_test_try_namespace_root(copied_conn, runtime_root, hook_upperdir)?;
        recreate_path_symlink(hook_upperdir, &namespace_root)?;
        return Ok(namespace_root);
    }

    let generation_dir = runtime_root.generation_path(try_generation_id);
    let metadata =
        conary_core::generation::metadata::GenerationMetadata::read_from(&generation_dir)
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| {
                format!(
                    "failed to read try generation metadata from {}",
                    generation_dir.display()
                )
            })?;
    let lower_root = work_dir.join("generation-root");
    let overlay_workdir = work_dir.join("namespace-work");
    std::fs::create_dir_all(&lower_root)
        .with_context(|| format!("failed to create try lower root {}", lower_root.display()))?;
    std::fs::create_dir_all(&namespace_root).with_context(|| {
        format!(
            "failed to create try namespace root {}",
            namespace_root.display()
        )
    })?;
    std::fs::create_dir_all(&overlay_workdir).with_context(|| {
        format!(
            "failed to create try namespace overlay workdir {}",
            overlay_workdir.display()
        )
    })?;

    let mount_options = conary_core::generation::mount::MountOptions {
        image_path: generation_dir.join(conary_core::generation::metadata::EROFS_IMAGE_NAME),
        basedir: runtime_root.objects_dir(),
        mount_point: lower_root.clone(),
        verity: metadata.fsverity_enabled,
        digest: metadata
            .fsverity_enabled
            .then(|| metadata.erofs_verity_digest.clone())
            .flatten(),
        upperdir: None,
        workdir: None,
    };
    conary_core::generation::mount::mount_generation(&mount_options)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| {
            format!(
                "failed to mount try generation {} at {}",
                try_generation_id,
                lower_root.display()
            )
        })?;
    if let Err(error) = mount_try_namespace_overlay(
        &lower_root,
        hook_upperdir,
        &overlay_workdir,
        &namespace_root,
    ) {
        let _ = conary_core::generation::mount::unmount_generation(&lower_root);
        return Err(error);
    }

    Ok(namespace_root)
}

fn mount_try_namespace_overlay(
    lower_root: &Path,
    hook_upperdir: &Path,
    overlay_workdir: &Path,
    namespace_root: &Path,
) -> Result<()> {
    let options = format!(
        "lowerdir={},upperdir={},workdir={}",
        lower_root.display(),
        hook_upperdir.display(),
        overlay_workdir.display()
    );
    let status = std::process::Command::new("mount")
        .args([
            "-t",
            "overlay",
            "overlay",
            "-o",
            &options,
            &namespace_root.to_string_lossy(),
        ])
        .status()
        .context("failed to execute try namespace overlay mount")?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "failed to mount try namespace overlay at {} with lower {} and upper {}",
        namespace_root.display(),
        lower_root.display(),
        hook_upperdir.display()
    )
}

fn teardown_try_namespace_mounts(work_dir: &Path) -> Result<()> {
    unmount_try_path_if_mounted(&work_dir.join("namespace-root"))?;
    unmount_try_path_if_mounted(&work_dir.join("generation-root"))?;
    Ok(())
}

fn unmount_try_path_if_mounted(path: &Path) -> Result<()> {
    if !try_path_is_mounted(path)? {
        return Ok(());
    }
    run_try_unmount(path)
}

fn try_path_is_mounted(path: &Path) -> Result<bool> {
    let mountinfo = read_try_mountinfo()?;
    Ok(mountinfo.lines().any(|line| {
        line.split_whitespace()
            .nth(4)
            .map(decode_mountinfo_path)
            .as_deref()
            == Some(path)
    }))
}

fn read_try_mountinfo() -> Result<String> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("CONARY_TEST_TRY_MOUNTINFO_PATH") {
        return std::fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to read try mountinfo {}",
                Path::new(&path).display()
            )
        });
    }

    std::fs::read_to_string("/proc/self/mountinfo").context("failed to read /proc/self/mountinfo")
}

fn decode_mountinfo_path(raw: &str) -> PathBuf {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1].is_ascii_digit()
            && bytes[index + 2].is_ascii_digit()
            && bytes[index + 3].is_ascii_digit()
        {
            let value = ((bytes[index + 1] - b'0') << 6)
                | ((bytes[index + 2] - b'0') << 3)
                | (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(decoded))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&decoded).into_owned())
    }
}

fn run_try_unmount(path: &Path) -> Result<()> {
    #[cfg(test)]
    if let Some(fail_path) = std::env::var_os("CONARY_TEST_TRY_UMOUNT_FAIL")
        && Path::new(&fail_path) == path
    {
        bail!(
            "forced try namespace unmount failure for {}",
            path.display()
        );
    }

    #[cfg(test)]
    if let Some(log_path) = std::env::var_os("CONARY_TEST_TRY_UMOUNT_LOG") {
        use std::io::Write as _;

        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| {
                format!(
                    "failed to open try unmount log {}",
                    Path::new(&log_path).display()
                )
            })?;
        writeln!(log, "{}", path.display())?;
        return Ok(());
    }

    conary_core::generation::mount::unmount_generation(path)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("failed to unmount try namespace path {}", path.display()))
}

fn materialize_test_try_namespace_root(
    copied_conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
    hook_upperdir: &Path,
) -> Result<()> {
    std::fs::create_dir_all(hook_upperdir).with_context(|| {
        format!(
            "failed to create test try namespace root {}",
            hook_upperdir.display()
        )
    })?;
    for entry in conary_core::db::models::FileEntry::find_all_ordered(copied_conn)
        .map_err(|error| anyhow::anyhow!(error))?
    {
        if conary_core::generation::metadata::is_excluded(&entry.path) {
            continue;
        }
        let relative = root_relative_path(&entry.path)?;
        let destination = hook_upperdir.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create parent directory for test try namespace file {}",
                    destination.display()
                )
            })?;
        }
        remove_path_if_exists(&destination)?;
        if let Some(target) = &entry.symlink_target {
            create_symlink(target.as_ref(), &destination)?;
            continue;
        }

        let object_path =
            conary_core::filesystem::object_path(&runtime_root.objects_dir(), &entry.sha256_hash)
                .map_err(|error| anyhow::anyhow!(error))
                .with_context(|| format!("failed to locate CAS object {}", entry.sha256_hash))?;
        if let Err(_error) = std::fs::hard_link(&object_path, &destination) {
            std::fs::copy(&object_path, &destination).with_context(|| {
                format!(
                    "failed to copy CAS object {} to test try namespace file {}",
                    object_path.display(),
                    destination.display()
                )
            })?;
        }
        set_file_mode(&destination, entry.permissions)?;
    }

    for (link, target) in conary_core::generation::metadata::ROOT_SYMLINKS {
        let link_path = hook_upperdir.join(link);
        if link_path.exists() || std::fs::symlink_metadata(&link_path).is_ok() {
            continue;
        }
        create_symlink((*target).as_ref(), &link_path)?;
    }

    Ok(())
}

fn recreate_path_symlink(target: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_if_exists(link)?;
    create_symlink(target, link)
}

fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).with_context(|| {
            format!(
                "failed to create symlink {} -> {}",
                link.display(),
                target.display()
            )
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        let _ = link;
        bail!("try namespace root materialization requires symlink support")
    }
}

fn set_file_mode(path: &Path, permissions: i32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = (permissions as u32) & 0o7777;
        let mut file_permissions = std::fs::metadata(path)?.permissions();
        file_permissions.set_mode(mode);
        std::fs::set_permissions(path, file_permissions)
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = permissions;
    }
    Ok(())
}

struct RunningTryCommand {
    child: std::process::Child,
    pid: i64,
    boot_id: String,
    label: &'static str,
}

fn run_try_command_for_session(
    command: &[&str],
    namespace_root: &Path,
    activated: bool,
    live_conn: &rusqlite::Connection,
    copied_conn: &rusqlite::Connection,
    live_session: &TrySession,
    copied_session: &TrySession,
) -> Result<()> {
    let mut running = spawn_try_command(command, namespace_root, activated)?;
    let record_result = (|| -> Result<()> {
        live_session.set_launcher(live_conn, running.pid, &running.boot_id)?;
        copied_session.set_launcher(copied_conn, running.pid, &running.boot_id)?;
        Ok(())
    })();
    if let Err(error) = record_result {
        let _ = running.child.kill();
        let _ = running.child.wait();
        return Err(error.context("failed to record try launcher liveness before waiting"));
    }

    let wait_result = wait_try_command(&mut running);
    let clear_result = clear_try_launcher(live_conn, &live_session.id)
        .and_then(|()| clear_try_launcher(copied_conn, &copied_session.id));

    match wait_result {
        Ok(()) => clear_result,
        Err(error) => {
            if let Err(clear_error) = clear_result {
                return Err(error.context(format!(
                    "also failed to clear try launcher liveness after exit: {clear_error}"
                )));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
fn launch_try_command(
    command: &[&str],
    namespace_root: &Path,
    activated: bool,
) -> Result<(i64, String)> {
    let mut running = spawn_try_command(command, namespace_root, activated)?;
    let pid = running.pid;
    let boot_id = running.boot_id.clone();
    wait_try_command(&mut running)?;
    Ok((pid, boot_id))
}

fn spawn_try_command(
    command: &[&str],
    namespace_root: &Path,
    activated: bool,
) -> Result<RunningTryCommand> {
    if command.is_empty() {
        bail!("try launcher command cannot be empty");
    }
    let boot_id = current_boot_id();
    if let Some(test_launcher) = std::env::var_os("CONARY_TEST_TRY_LAUNCHER") {
        let child = std::process::Command::new(test_launcher)
            .arg(namespace_root)
            .args(command)
            .spawn()
            .context("failed to start CONARY_TEST_TRY_LAUNCHER")?;
        return Ok(running_try_command(
            child,
            boot_id,
            "CONARY_TEST_TRY_LAUNCHER",
        ));
    }
    if activated {
        let child = std::process::Command::new(command[0])
            .args(&command[1..])
            .spawn()
            .with_context(|| format!("failed to start activated try command {}", command[0]))?;
        return Ok(running_try_command(child, boot_id, "activated try command"));
    }
    let Some(bwrap) = find_command("bwrap") else {
        bail!(
            "bubblewrap is required for namespace try; `conary try --activate` is the M1b fallback for host-global testing and mutates the host-global current generation"
        );
    };
    let child = std::process::Command::new(bwrap)
        .arg("--unshare-all")
        .arg("--die-with-parent")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--ro-bind")
        .arg(namespace_root)
        .arg("/")
        .arg("--chdir")
        .arg("/")
        .arg("--")
        .args(command)
        .spawn()
        .context("failed to start bubblewrap namespace try launcher")?;
    Ok(running_try_command(
        child,
        boot_id,
        "bubblewrap namespace try launcher",
    ))
}

fn running_try_command(
    child: std::process::Child,
    boot_id: String,
    label: &'static str,
) -> RunningTryCommand {
    RunningTryCommand {
        pid: i64::from(child.id()),
        child,
        boot_id,
        label,
    }
}

fn wait_try_command(running: &mut RunningTryCommand) -> Result<()> {
    let status = running
        .child
        .wait()
        .with_context(|| format!("failed to wait for {}", running.label))?;
    if !status.success() {
        bail!("{} exited with status {status}", running.label);
    }
    Ok(())
}

fn clear_try_launcher(conn: &rusqlite::Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE try_sessions
         SET launcher_pid = NULL,
             launcher_boot_id = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE id = ?1
           AND status IN ('active', 'orphaned')",
        [session_id],
    )?;
    Ok(())
}

fn record_activated_try_boot(
    conn: &rusqlite::Connection,
    session_id: &str,
    boot_id: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE try_sessions
         SET launcher_pid = NULL,
             launcher_boot_id = ?1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
         WHERE id = ?2
           AND status IN ('active', 'orphaned')",
        rusqlite::params![boot_id, session_id],
    )?;
    Ok(())
}

fn current_boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown-boot".to_string())
}

fn find_command(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

fn remove_dir_if_exists(path: PathBuf) -> Result<()> {
    #[cfg(test)]
    if let Some(fail_path) = std::env::var_os("CONARY_TEST_TRY_REMOVE_DIR_FAIL")
        && Path::new(&fail_path) == path
    {
        bail!(
            "forced try directory removal failure for {}",
            path.display()
        );
    }

    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("failed to remove {}", path.display()))
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
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::{
        AlternativeHook, CcsManifest, DirectoryHook, GroupHook, ScriptHook, Service, ServiceAction,
        SysctlHook, SystemdHook, TmpfilesHook, UserHook,
    };
    use conary_core::ccs::{BuildResult, CcsPackage, ComponentData, FileEntry, FileType};
    use conary_core::db::models::{TrySession, TrySessionMode};
    use conary_core::packages::traits::PackageFormat;
    use conary_core::runtime_root::ConaryRuntimeRoot;

    use super::*;

    fn validate_manifest(
        manifest: &CcsManifest,
        execution_root: TryExecutionRoot,
        allow_irreversible: bool,
        activated: bool,
    ) -> anyhow::Result<()> {
        validate_try_manifest_policy(manifest, execution_root, allow_irreversible, activated)
    }

    fn assert_policy_error_contains(
        manifest: &CcsManifest,
        execution_root: TryExecutionRoot,
        allow_irreversible: bool,
        activated: bool,
        expected: &str,
    ) {
        let err = validate_manifest(manifest, execution_root, allow_irreversible, activated)
            .expect_err("policy should reject package");
        let message = err.to_string();
        assert!(
            message.contains(expected),
            "expected error to contain {expected:?}, got {message:?}"
        );
    }

    fn minimal_package(manifest: CcsManifest) -> anyhow::Result<CcsPackage> {
        let temp_dir = tempfile::tempdir()?;
        let package_path = temp_dir.path().join("try-policy.ccs");
        let content = b"try package".to_vec();
        let hash = conary_core::hash::sha256(&content);
        let files = vec![FileEntry {
            path: "/usr/bin/try-policy".to_string(),
            hash: hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }];
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: content.len() as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(hash, content)]),
            total_size: 11,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path)?;
        <CcsPackage as PackageFormat>::parse(&package_path.to_string_lossy())
            .map_err(|error| anyhow::anyhow!(error))
    }

    struct TryRuntimeFixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        db_path: PathBuf,
        db_path_string: String,
    }

    impl TryRuntimeFixture {
        fn new() -> Self {
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

        fn runtime_root(&self) -> ConaryRuntimeRoot {
            ConaryRuntimeRoot::from_db_path(self.db_path.clone())
        }

        fn write_package(&self, name: &str, manifest: CcsManifest) -> PathBuf {
            write_try_package(self.root.join(format!("{name}.ccs")), manifest)
        }

        fn open(&self) -> rusqlite::Connection {
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

    fn begin_namespace_try(
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

    fn begin_activated_try(
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

    fn stored_session(fixture: &TryRuntimeFixture, id: &str) -> TrySession {
        TrySession::find_by_id(&fixture.open(), id)
            .unwrap()
            .expect("stored try session")
    }

    fn create_current_generation_link(root: &Path, generation: i64) {
        std::fs::create_dir_all(root.join(format!("generations/{generation}"))).unwrap();
        conary_core::generation::mount::update_current_symlink(root, generation).unwrap();
    }

    #[test]
    fn activated_no_command_session_records_boot_without_launcher_pid() -> anyhow::Result<()> {
        let fixture = TryRuntimeFixture::new();
        let conn = fixture.open();
        let session = TrySession::create_active(
            &conn,
            CreateTrySession {
                id: "try-activated-no-command",
                package_path: "/tmp/demo.ccs",
                package_name: Some("demo"),
                package_version: Some("1.0.0"),
                previous_generation_id: Some(1),
                mode: TrySessionMode::Activated,
                work_dir: "/tmp/try-activated-no-command",
            },
        )?;

        record_activated_try_boot(&conn, &session.id, "boot-a")?;

        let stored = stored_session(&fixture, &session.id);
        assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-a"));
        assert_eq!(stored.launcher_pid, None);
        Ok(())
    }

    fn has_cas_object(root: &Path) -> bool {
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

    fn write_try_mountinfo(path: &Path, mounted_paths: &[&Path]) -> anyhow::Result<()> {
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

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
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

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn manifest_with_post_install_script() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("script-post", "1.0.0");
        manifest.hooks.post_install = Some(ScriptHook {
            script: "echo post-install".to_string(),
            reversible: None,
        });
        manifest
    }

    fn manifest_with_pre_remove_script() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("script-pre", "1.0.0");
        manifest.hooks.pre_remove = Some(ScriptHook {
            script: "echo pre-remove".to_string(),
            reversible: None,
        });
        manifest
    }

    fn manifest_with_declarative_hook() -> CcsManifest {
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

    fn manifest_with_user_group_hooks() -> CcsManifest {
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

    fn manifest_with_systemd_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("systemd-hook", "1.0.0");
        manifest.hooks.systemd.push(SystemdHook {
            unit: "try-systemd.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_tmpfiles_hook() -> CcsManifest {
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

    fn manifest_with_sysctl_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("sysctl-hook", "1.0.0");
        manifest.hooks.sysctl.push(SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "0".to_string(),
            only_if_lower: false,
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_alternative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("alternative-hook", "1.0.0");
        manifest.hooks.alternatives.push(AlternativeHook {
            name: "try-editor".to_string(),
            path: "/usr/bin/try-editor".to_string(),
            priority: 50,
            reversible: Some(true),
        });
        manifest
    }

    fn manifest_with_service_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("service-hook", "1.0.0");
        manifest.hooks.services.push(Service {
            name: "service-hook.service".to_string(),
            action: ServiceAction::Restart,
            reversible: None,
        });
        manifest
    }

    fn manifest_with_legacy_scriptlet_bundle() -> CcsManifest {
        let body = "ldconfig";
        let body_sha256 = conary_core::hash::sha256_prefixed(body.as_bytes());
        let toml = format!(
            r#"
[package]
name = "legacy-scriptlets"
version = "1.0.0"
description = "legacy scriptlets"

[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
source_format = "rpm"
source_family = "fedora-rhel"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "legacy-scriptlets"
source_version = "1.0.0-1.fc44"
source_checksum = "sha256:3333333333333333333333333333333333333333333333333333333333333333"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.8.0"
conversion_policy = "safe-or-legacy"
target_compatibility = "source-native"
allowed_targets = ["rpm/fedora/44/x86_64"]
foreign_replay_policy = "deny"
publication_policy = "public-if-no-blocked"
publication_status = "private-review"
scriptlet_fidelity = "legacy-replay"

[legacy_scriptlets.decision_counts]
legacy = 1

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
lifecycle_paths = ["install:first"]
interpreter = "/bin/sh"
interpreter_args = ["-e"]
body_sha256 = "{body_sha256}"
body = "{body}"
native_invocation = {{ args = ["1"], environment = ["RPM_INSTALL_PREFIX=/"], stdin = "none", chroot = "install-root" }}
transaction_order = {{ position = "after-payload", after = ["payload"] }}
timeout_ms = 30000
decision = "legacy"
reason_code = "protected-replay-required"

[[legacy_scriptlets.entries.effects]]
kind = "ldconfig"
source = "static-signal"
confidence = "declared"
replacement = "complete"
"#
        );

        CcsManifest::parse(&toml).expect("parse legacy scriptlet fixture")
    }

    #[test]
    fn package_with_no_hooks_is_allowed() -> anyhow::Result<()> {
        let manifest = CcsManifest::new_minimal("no-hooks", "1.0.0");
        validate_manifest(&manifest, TryExecutionRoot::Namespace, false, false)?;
        validate_manifest(&manifest, TryExecutionRoot::Generation, false, false)?;
        validate_manifest(&manifest, TryExecutionRoot::Host, false, true)?;

        let package = minimal_package(manifest)?;
        validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)
    }

    #[test]
    fn declarative_hooks_are_allowed_only_for_try_or_generation_roots() {
        let manifest = manifest_with_declarative_hook();

        validate_manifest(&manifest, TryExecutionRoot::Namespace, false, false)
            .expect("namespace-root declarative hooks should be allowed");
        validate_manifest(&manifest, TryExecutionRoot::Generation, false, false)
            .expect("generation-root declarative hooks should be allowed");
        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Host,
            false,
            false,
            "try hooks cannot execute against the host root",
        );
    }

    #[test]
    fn post_install_script_hooks_are_rejected_by_default() {
        let manifest = manifest_with_post_install_script();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            false,
            "scripts cannot run against the host root",
        );
    }

    #[test]
    fn pre_remove_script_hooks_are_rejected_by_default() {
        let manifest = manifest_with_pre_remove_script();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            false,
            "scripts cannot run against the host root",
        );
    }

    #[test]
    fn legacy_scriptlet_bundles_are_rejected_by_default() {
        let manifest = manifest_with_legacy_scriptlet_bundle();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            false,
            "legacy scriptlet bundles are not supported in M1b try sessions",
        );
    }

    #[test]
    fn service_hooks_are_rejected_in_m1b() {
        let manifest = manifest_with_service_hook();

        assert_policy_error_contains(
            &manifest,
            TryExecutionRoot::Namespace,
            false,
            false,
            "service lifecycle is not generation-scoped",
        );
    }

    #[test]
    fn unsupported_declarative_hook_classes_are_rejected_in_m1b_try_policy() {
        for (manifest, expected) in [
            (manifest_with_systemd_hook(), "hooks.systemd"),
            (manifest_with_tmpfiles_hook(), "hooks.tmpfiles"),
            (manifest_with_sysctl_hook(), "hooks.sysctl"),
            (manifest_with_alternative_hook(), "hooks.alternatives"),
        ] {
            assert_policy_error_contains(
                &manifest,
                TryExecutionRoot::Namespace,
                true,
                false,
                expected,
            );
            assert_policy_error_contains(&manifest, TryExecutionRoot::Generation, true, true, "M2");
        }
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
    fn package_round_trip_preserves_service_hooks_for_policy() -> anyhow::Result<()> {
        let package = minimal_package(manifest_with_service_hook())?;

        let err = validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)
            .expect_err("package service hook should be rejected after round trip");

        assert!(
            err.to_string()
                .contains("service lifecycle is not generation-scoped"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn package_round_trip_preserves_declarative_reversibility_for_policy() -> anyhow::Result<()> {
        let mut manifest = manifest_with_declarative_hook();
        manifest.hooks.directories[0].reversible = Some(false);
        let package = minimal_package(manifest)?;

        let err = validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)
            .expect_err("irreversible declarative hook should be rejected after round trip");
        assert!(
            err.to_string()
                .contains("try package contains irreversible hooks"),
            "unexpected error: {err}"
        );

        validate_try_package_policy(&package, TryExecutionRoot::Namespace, true, false)?;
        validate_try_package_policy(&package, TryExecutionRoot::Generation, true, false)
    }

    #[test]
    fn allow_irreversible_does_not_permit_scripts_legacy_or_services() {
        assert_policy_error_contains(
            &manifest_with_post_install_script(),
            TryExecutionRoot::Namespace,
            true,
            false,
            "scripts cannot run against the host root",
        );
        assert_policy_error_contains(
            &manifest_with_pre_remove_script(),
            TryExecutionRoot::Namespace,
            true,
            true,
            "host-root lifecycle helper is M2 work",
        );
        assert_policy_error_contains(
            &manifest_with_legacy_scriptlet_bundle(),
            TryExecutionRoot::Namespace,
            true,
            false,
            "legacy scriptlet bundles are not supported in M1b try sessions",
        );
        assert_policy_error_contains(
            &manifest_with_legacy_scriptlet_bundle(),
            TryExecutionRoot::Namespace,
            true,
            true,
            "host-root lifecycle helper is M2 work",
        );
        assert_policy_error_contains(
            &manifest_with_service_hook(),
            TryExecutionRoot::Generation,
            true,
            false,
            "service lifecycle is not generation-scoped",
        );
        assert_policy_error_contains(
            &manifest_with_service_hook(),
            TryExecutionRoot::Generation,
            true,
            true,
            "host-root lifecycle helper is M2 work",
        );
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
    fn declarative_try_hooks_refuse_host_root() {
        let manifest = manifest_with_declarative_hook();

        let err = apply_declarative_try_hooks(&manifest, Path::new("/"))
            .expect_err("try hooks must not run against host root");

        assert!(err.to_string().contains("host root"));
    }

    #[test]
    fn namespace_declarative_hooks_write_to_live_etc_state_not_workdir() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package("try-hooks", manifest_with_declarative_hook());

        let outcome = begin_namespace_try(&fixture, &package)?;

        assert!(
            fixture
                .root
                .join(format!(
                    "etc-state/{}/var/lib/declarative",
                    outcome.try_generation_id
                ))
                .is_dir(),
            "declarative hook effects must land in live etc-state upperdir"
        );
        assert!(
            !outcome.work_dir.join("root/var/lib/declarative").exists(),
            "throwaway install scratch root must not be the only hook effect location"
        );
        Ok(())
    }

    #[test]
    fn namespace_command_sees_generation_files_and_hook_upperdir() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir()?;
        let launcher = temp.path().join("launcher.sh");
        let seen_root = temp.path().join("seen-root");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nroot=\"$1\"\nif [ ! -f \"$root/usr/bin/try-launch-root\" ]; then echo missing package file >&2; exit 43; fi\nif [ ! -d \"$root/var/lib/declarative\" ]; then echo missing hook dir >&2; exit 44; fi\nprintf '%s\\n' \"$root\" > \"$TRY_SEEN_ROOT_FILE\"\n",
        )?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&launcher)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&launcher, permissions)?;
        }
        let _launcher_guard = EnvVarGuard::set("CONARY_TEST_TRY_LAUNCHER", &launcher);
        let _seen_guard = EnvVarGuard::set("TRY_SEEN_ROOT_FILE", &seen_root);
        let fixture = TryRuntimeFixture::new();
        let mut manifest = CcsManifest::new_minimal("try-launch-root", "1.0.0");
        manifest.hooks.directories.push(DirectoryHook {
            path: "/var/lib/declarative".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: None,
        });
        let package = fixture.write_package("try-launch-root", manifest);
        let command = ["/usr/bin/try-launch-root"];

        let outcome = begin_try_session(TryStartRequest {
            db_path: &fixture.db_path_string,
            package_path: &package,
            activate: false,
            allow_irreversible: false,
            command: Some(&command),
        })?;

        let launcher_root = PathBuf::from(std::fs::read_to_string(seen_root)?.trim());
        assert_eq!(launcher_root, outcome.namespace_root);
        assert_ne!(outcome.namespace_root, outcome.install_root);
        assert!(
            outcome
                .namespace_root
                .join("usr/bin/try-launch-root")
                .is_file(),
            "namespace root must expose installed package files"
        );
        assert!(
            fixture
                .root
                .join(format!(
                    "etc-state/{}/var/lib/declarative",
                    outcome.try_generation_id
                ))
                .is_dir(),
            "hook writes must land in the live etc-state upperdir"
        );
        Ok(())
    }

    #[test]
    fn activated_declarative_hooks_use_promotable_etc_state_before_publish() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 3);
        let package =
            fixture.write_package("try-activated-hooks", manifest_with_declarative_hook());

        let outcome = begin_activated_try(&fixture, &package)?;

        assert!(
            fixture
                .root
                .join(format!(
                    "etc-state/{}/var/lib/declarative",
                    outcome.try_generation_id
                ))
                .is_dir(),
            "activated declarative hooks must use the promotable generation upperdir"
        );
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
    fn namespace_rollback_unmounts_namespace_before_generation_root() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 2);
        let package = fixture.write_package(
            "try-rollback-unmount",
            CcsManifest::new_minimal("try-rollback-unmount", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;
        let mountinfo = fixture.root.join("try-mountinfo");
        let unmount_log = fixture.root.join("try-unmount.log");
        let namespace_root = outcome.work_dir.join("namespace-root");
        let generation_root = outcome.work_dir.join("generation-root");
        write_try_mountinfo(&mountinfo, &[&namespace_root, &generation_root])?;
        let _mountinfo_guard = EnvVarGuard::set("CONARY_TEST_TRY_MOUNTINFO_PATH", &mountinfo);
        let _unmount_guard = EnvVarGuard::set("CONARY_TEST_TRY_UMOUNT_LOG", &unmount_log);

        rollback_active_try_session(&fixture.db_path_string)?;

        let unmounted = std::fs::read_to_string(unmount_log)?
            .lines()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        assert_eq!(unmounted, vec![namespace_root, generation_root]);
        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::RolledBack
        );
        assert!(
            !outcome.work_dir.exists(),
            "rollback must remove try work dir after unmounting"
        );
        Ok(())
    }

    #[test]
    fn namespace_rollback_leaves_session_retryable_when_unmount_fails() -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = TryRuntimeFixture::new();
        create_current_generation_link(&fixture.root, 2);
        let package = fixture.write_package(
            "try-rollback-unmount-fail",
            CcsManifest::new_minimal("try-rollback-unmount-fail", "1.0.0"),
        );
        let outcome = begin_namespace_try(&fixture, &package)?;
        let mountinfo = fixture.root.join("try-mountinfo");
        let unmount_log = fixture.root.join("try-unmount.log");
        let namespace_root = outcome.work_dir.join("namespace-root");
        let generation_root = outcome.work_dir.join("generation-root");
        write_try_mountinfo(&mountinfo, &[&namespace_root, &generation_root])?;
        let _mountinfo_guard = EnvVarGuard::set("CONARY_TEST_TRY_MOUNTINFO_PATH", &mountinfo);
        let _unmount_guard = EnvVarGuard::set("CONARY_TEST_TRY_UMOUNT_LOG", &unmount_log);
        let _fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_UMOUNT_FAIL", &namespace_root);

        let err = rollback_active_try_session(&fixture.db_path_string)
            .expect_err("rollback should fail before marking rolled_back when unmount fails");
        let message = format!("{err:#}");
        assert!(
            message.contains("forced try namespace unmount failure"),
            "{message}"
        );
        assert!(message.contains("namespace-root"), "{message}");
        assert_eq!(
            stored_session(&fixture, &outcome.session_id).status,
            conary_core::db::models::TrySessionStatus::Active
        );
        assert!(
            outcome.work_dir.exists(),
            "failed cleanup must leave work dir for retry"
        );
        assert!(
            fixture
                .root
                .join(format!("generations/{}", outcome.try_generation_id))
                .exists(),
            "failed cleanup must leave generation artifacts for retry"
        );
        Ok(())
    }

    #[test]
    fn namespace_rollback_leaves_session_retryable_when_work_dir_removal_fails()
    -> anyhow::Result<()> {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _env_lock = ENV_LOCK.lock().unwrap();
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

    #[test]
    fn namespace_launcher_executes_bubblewrap_when_available() -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir()?;
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        let bwrap = bin_dir.join("bwrap");
        let args_file = temp.path().join("bwrap.args");
        let pid_file = temp.path().join("bwrap.pid");
        std::fs::write(
            &bwrap,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$BWRAP_PID_FILE\"\nprintf '%s\\n' \"$@\" > \"$BWRAP_ARGS_FILE\"\n",
        )?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&bwrap)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bwrap, permissions)?;
        }
        let _path_guard = EnvVarGuard::set("PATH", &bin_dir);
        let _args_guard = EnvVarGuard::set("BWRAP_ARGS_FILE", &args_file);
        let _pid_guard = EnvVarGuard::set("BWRAP_PID_FILE", &pid_file);
        let namespace_root = temp.path().join("namespace-root");
        std::fs::create_dir_all(&namespace_root)?;

        let (pid, _) = launch_try_command(&["/bin/echo", "hello"], &namespace_root, false)?;

        let args = std::fs::read_to_string(args_file)?;
        assert!(args.contains("--ro-bind"), "{args}");
        assert!(
            args.contains(&namespace_root.display().to_string()),
            "{args}"
        );
        assert!(args.contains("/bin/echo"), "{args}");
        assert!(args.contains("hello"), "{args}");
        let child_pid: i64 = std::fs::read_to_string(pid_file)?.trim().parse()?;
        assert_eq!(pid, child_pid, "launcher must return the spawned child PID");
        assert_ne!(
            pid,
            i64::from(std::process::id()),
            "launcher must not record the conary parent process PID"
        );
        Ok(())
    }

    #[test]
    fn try_command_records_child_liveness_before_wait_and_clears_after_exit() -> anyhow::Result<()>
    {
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir()?;
        let launcher = temp.path().join("launcher.sh");
        let pid_file = temp.path().join("launcher.pid");
        let release_file = temp.path().join("release");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$TRY_PID_FILE\"\nwhile [ ! -f \"$TRY_RELEASE_FILE\" ]; do sleep 0.05; done\n",
        )?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&launcher)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&launcher, permissions)?;
        }
        let _launcher_guard = EnvVarGuard::set("CONARY_TEST_TRY_LAUNCHER", &launcher);
        let _pid_guard = EnvVarGuard::set("TRY_PID_FILE", &pid_file);
        let _release_guard = EnvVarGuard::set("TRY_RELEASE_FILE", &release_file);
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package(
            "try-launch-liveness",
            CcsManifest::new_minimal("try-launch-liveness", "1.0.0"),
        );
        let db_path_string = fixture.db_path_string.clone();
        let package_for_thread = package.clone();

        let handle = std::thread::spawn(move || {
            let command = ["/bin/true"];
            begin_try_session(TryStartRequest {
                db_path: &db_path_string,
                package_path: package_for_thread.as_path(),
                activate: false,
                allow_irreversible: false,
                command: Some(&command),
            })
        });

        let child_pid = poll_until(std::time::Duration::from_secs(5), || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|value| value.trim().parse::<i64>().ok())
        })
        .ok_or_else(|| anyhow::anyhow!("launcher did not write child PID"))?;

        let live_session = poll_until(std::time::Duration::from_secs(5), || {
            TrySession::find_active_or_orphaned(&fixture.open())
                .ok()
                .flatten()
                .filter(|session| session.launcher_pid.is_some())
        })
        .ok_or_else(|| anyhow::anyhow!("live DB never recorded launcher liveness"))?;
        assert_eq!(live_session.launcher_pid, Some(child_pid));
        assert_ne!(
            live_session.launcher_pid,
            Some(i64::from(std::process::id()))
        );
        assert!(live_session.launcher_boot_id.is_some());

        let copied_db_path = PathBuf::from(&live_session.work_dir).join("conary.db");
        let copied_session = poll_until(std::time::Duration::from_secs(5), || {
            conary_core::db::open(&copied_db_path)
                .ok()
                .and_then(|conn| {
                    TrySession::find_by_id(&conn, &live_session.id)
                        .ok()
                        .flatten()
                        .filter(|session| session.launcher_pid == Some(child_pid))
                })
        })
        .ok_or_else(|| anyhow::anyhow!("copied DB never recorded launcher liveness"))?;
        assert_eq!(
            copied_session.launcher_boot_id,
            live_session.launcher_boot_id
        );

        std::fs::write(&release_file, b"release")?;
        let outcome = handle
            .join()
            .map_err(|_| anyhow::anyhow!("try launcher thread panicked"))??;

        let live_after = stored_session(&fixture, &outcome.session_id);
        assert_eq!(live_after.launcher_pid, None);
        assert_eq!(live_after.launcher_boot_id, None);
        let copied = conary_core::db::open(&outcome.copied_db_path)?;
        let copied_after = TrySession::find_by_id(&copied, &outcome.session_id)?.unwrap();
        assert_eq!(copied_after.launcher_pid, None);
        assert_eq!(copied_after.launcher_boot_id, None);
        Ok(())
    }

    fn poll_until<T>(
        timeout: std::time::Duration,
        mut probe: impl FnMut() -> Option<T>,
    ) -> Option<T> {
        let start = std::time::Instant::now();
        loop {
            if let Some(value) = probe() {
                return Some(value);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
