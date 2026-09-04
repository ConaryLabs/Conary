// apps/conary/src/commands/try_session/mod.rs
//! Try-session policy helpers.

use anyhow::{Context, Result};
use conary_core::ccs::verify::TrustPolicy;
use conary_core::db::models::TrySession;
use std::path::{Path, PathBuf};
use thiserror::Error;

mod executor;
mod install;
mod namespace;
mod package_verification;
mod session;
mod util;
mod validation;
mod watch;
mod watch_source;

pub(crate) use session::refresh_try_session;
pub(crate) use session::{
    activated_try_session_is_live, begin_try_session, current_boot_id,
    namespace_try_session_is_decision_pending, rollback_active_try_session,
};
#[derive(Debug, Clone, Copy)]
pub(crate) struct TryStartRequest<'a> {
    pub db_path: &'a str,
    pub package_path: &'a Path,
    pub trust_policy: &'a TrustPolicy,
    pub activate: bool,
    pub command: Option<&'a [&'a str]>,
    pub watch_marker: Option<TryWatchMarkerRequest<'a>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TryWatchMarkerRequest<'a> {
    pub(crate) operation_id: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TryRefreshRequest<'a> {
    pub(crate) db_path: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) expected_try_generation_id: i64,
    pub(crate) package_path: &'a Path,
    pub(crate) trust_policy: &'a TrustPolicy,
}

#[derive(Debug, Error)]
#[error("try watch session {session_id} diverged from watcher-owned state")]
pub(crate) struct TryRefreshSessionDiverged {
    session_id: String,
}

impl TryRefreshSessionDiverged {
    pub(crate) fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TryRefreshOutcome {
    pub(crate) previous_generation_id: i64,
    pub(crate) try_generation_id: i64,
    pub(crate) namespace_root: PathBuf,
    pub(crate) copied_package_path: PathBuf,
    pub(crate) cleanup_error: Option<String>,
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

pub(crate) fn cmd_try_package(
    db_path: &str,
    package_path: &Path,
    trust_policy_path: &Path,
    activate: bool,
    run: &[String],
) -> Result<()> {
    let trust_policy = TrustPolicy::from_file(trust_policy_path).with_context(|| {
        format!(
            "failed to load try trust policy {}",
            trust_policy_path.display()
        )
    })?;
    let command = run.iter().map(String::as_str).collect::<Vec<_>>();
    let outcome = begin_try_session(TryStartRequest {
        db_path,
        package_path,
        trust_policy: &trust_policy,
        activate,
        command: if command.is_empty() {
            None
        } else {
            Some(command.as_slice())
        },
        watch_marker: None,
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

pub(crate) fn cmd_try_status(db_path: &str) -> Result<()> {
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

pub(crate) fn cmd_try_rollback(db_path: &str) -> Result<()> {
    rollback_active_try_session(db_path)?;
    println!("Try session rolled back");
    Ok(())
}

pub(crate) fn cmd_try_keep(db_path: &str) -> Result<()> {
    session::keep_active_try_session(db_path)?;
    println!("Try session kept");
    Ok(())
}

pub(crate) async fn cmd_try_watch(
    db_path: &str,
    target: &str,
    recipe: Option<&str>,
    signing_key_path: &Path,
    isolated: bool,
    json: bool,
) -> Result<()> {
    watch::cmd_try_watch(watch::TryWatchOptions {
        db_path,
        target,
        recipe,
        signing_key_path,
        isolated,
        json,
    })
    .await
}

#[cfg(test)]
#[path = "tests/test_support.rs"]
pub(super) mod test_support;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
