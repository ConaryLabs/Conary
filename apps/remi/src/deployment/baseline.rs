// apps/remi/src/deployment/baseline.rs

//! Constant-cardinality deployment fencing evidence.

use super::{
    DeploymentProfileRefreshState, configured_public_profiles, refresh_diagnostics,
    require_plain_file,
};
use crate::server::config::RemiConfig;
use crate::server::repository_manifest::RepositoryManifest;
use anyhow::{Context, Result, bail};
use conary_core::db::schema::{SCHEMA_EPOCH, SCHEMA_VERSION};
use nix::sys::resource::{Usage, UsageWho, getrusage};
use nix::sys::time::TimeValLike;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

const BASELINE_SCHEMA_VERSION: u32 = 1;
const BASELINE_BUSY_TIMEOUT: Duration = Duration::from_millis(500);

thread_local! {
    static BASELINE_SQLITE_STATEMENTS: Cell<u64> = const { Cell::new(0) };
}

/// Read-only state used only to fence a later deployment result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentBaseline {
    pub baseline_schema_version: u32,
    pub schema_epoch: &'static str,
    pub schema_revision: i32,
    pub configured_profiles: usize,
    pub candidate_profiles: usize,
    pub candidates: Vec<DeploymentBaselineCandidateState>,
    pub measurement: DeploymentBaselineMeasurement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentBaselineCandidateState {
    pub profile: String,
    pub configured_sources: usize,
    pub identity: Option<DeploymentBaselineCandidateIdentity>,
    pub latest_refresh: Option<DeploymentProfileRefreshState>,
}

/// The all-or-absent private-candidate identity recorded before mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentBaselineCandidateIdentity {
    pub profile_revision_sha256: String,
    pub run_id: String,
    pub completed_at: i64,
}

/// Bounded work evidence for the baseline itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentBaselineMeasurement {
    pub wall_time_micros: u64,
    pub user_cpu_micros: u64,
    pub system_cpu_micros: u64,
    pub max_rss_bytes: u64,
    pub sqlite_statements: u64,
    pub sqlite_page_cache_misses: u64,
    pub sqlite_logical_read_bytes: u64,
    pub catalog_file_opens: u64,
    pub catalog_bytes_read: u64,
    pub output_bytes: u64,
}

impl DeploymentBaseline {
    /// Render stable pretty JSON and bind the exact emitted byte count.
    pub fn into_pretty_json(mut self) -> Result<String> {
        for _ in 0..4 {
            let rendered = serde_json::to_string_pretty(&self)?;
            let output_bytes = u64::try_from(rendered.len())?
                .checked_add(1)
                .context("deployment baseline output size overflow")?;
            if self.measurement.output_bytes == output_bytes {
                return Ok(rendered);
            }
            self.measurement.output_bytes = output_bytes;
        }
        bail!("deployment baseline output size did not converge")
    }
}

/// Read the exact pre-transition fencing state without opening immutable catalogs.
pub fn inspect_baseline(config_path: &Path) -> Result<DeploymentBaseline> {
    let started = Instant::now();
    let usage_before = getrusage(UsageWho::RUSAGE_SELF)?;

    require_plain_file(config_path, "Remi config")?;
    let config = RemiConfig::load(config_path)?;
    config.validate()?;
    let repository_manifest_path = config
        .repository_manifest
        .as_deref()
        .context("Remi config does not declare repository_manifest")?;
    require_plain_file(repository_manifest_path, "repository manifest")?;
    let repository_manifest = RepositoryManifest::load(repository_manifest_path)?;
    let configured = configured_public_profiles(&repository_manifest)?;

    let db_path = config.storage_root().join("metadata/conary.db");
    require_plain_file(&db_path, "Remi database")?;
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BASELINE_BUSY_TIMEOUT)?;
    let page_size =
        u64::try_from(conn.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?)
            .context("deployment baseline SQLite page size is negative")?;
    sqlite_cache_misses(&conn, true)?;
    let statement_counter = SqliteStatementCounter::start(&conn);

    conary_core::db::schema::require_current(&conn)?;
    repository_manifest.verify_reconciled(&conn)?;
    let candidates = inspect_candidates(&conn, &configured)?;

    let sqlite_statements = statement_counter.finish();
    let sqlite_page_cache_misses = sqlite_cache_misses(&conn, false)?;
    let sqlite_logical_read_bytes = sqlite_page_cache_misses
        .checked_mul(page_size)
        .context("deployment baseline SQLite read-byte count overflow")?;
    let usage_after = getrusage(UsageWho::RUSAGE_SELF)?;
    let wall_time_micros = u64::try_from(started.elapsed().as_micros())?;
    let candidate_profiles = candidates
        .iter()
        .filter(|candidate| candidate.identity.is_some())
        .count();

    Ok(DeploymentBaseline {
        baseline_schema_version: BASELINE_SCHEMA_VERSION,
        schema_epoch: SCHEMA_EPOCH,
        schema_revision: SCHEMA_VERSION,
        configured_profiles: candidates.len(),
        candidate_profiles,
        candidates,
        measurement: DeploymentBaselineMeasurement {
            wall_time_micros,
            user_cpu_micros: usage_delta_micros(
                usage_before.user_time().num_microseconds(),
                usage_after.user_time().num_microseconds(),
                "user CPU",
            )?,
            system_cpu_micros: usage_delta_micros(
                usage_before.system_time().num_microseconds(),
                usage_after.system_time().num_microseconds(),
                "system CPU",
            )?,
            max_rss_bytes: max_rss_bytes(&usage_after)?,
            sqlite_statements,
            sqlite_page_cache_misses,
            sqlite_logical_read_bytes,
            catalog_file_opens: 0,
            catalog_bytes_read: 0,
            output_bytes: 0,
        },
    })
}

pub(super) fn inspect_candidates(
    conn: &Connection,
    configured: &[(String, usize)],
) -> Result<Vec<DeploymentBaselineCandidateState>> {
    let mut candidates = Vec::with_capacity(configured.len());
    for (profile, configured_sources) in configured {
        let latest_refresh = refresh_diagnostics::latest_profile_refresh(conn, profile)?;
        let candidate = conary_core::repository::current_profile_sync_candidate(conn, profile)?;
        let identity = candidate
            .map(|candidate| {
                conary_core::db::models::verify_private_profile_candidate_authority(
                    conn,
                    profile,
                    &candidate.profile_revision_sha256,
                    &candidate.run_id,
                )
                .with_context(|| {
                    format!("verify private profile '{profile}' repository authority")
                })?;
                let candidate_sources = usize::try_from(conn.query_row(
                    "SELECT COUNT(*) FROM repository_sync_run_members WHERE run_id = ?1",
                    [&candidate.run_id],
                    |row| row.get::<_, i64>(0),
                )?)
                .context("private candidate source count is negative")?;
                if candidate_sources != *configured_sources {
                    bail!(
                        "private profile '{profile}' contains {candidate_sources} sources; configured authority contains {configured_sources}"
                    );
                }
                Ok(DeploymentBaselineCandidateIdentity {
                    profile_revision_sha256: candidate.profile_revision_sha256,
                    run_id: candidate.run_id,
                    completed_at: candidate.completed_at,
                })
            })
            .transpose()?;
        candidates.push(DeploymentBaselineCandidateState {
            profile: profile.clone(),
            configured_sources: *configured_sources,
            identity,
            latest_refresh,
        });
    }
    Ok(candidates)
}

fn usage_delta_micros(before: i64, after: i64, description: &str) -> Result<u64> {
    let delta = after
        .checked_sub(before)
        .with_context(|| format!("deployment baseline {description} counter underflow"))?;
    u64::try_from(delta)
        .with_context(|| format!("deployment baseline {description} counter is negative"))
}

fn max_rss_bytes(usage: &Usage) -> Result<u64> {
    let max_rss =
        u64::try_from(usage.max_rss()).context("deployment baseline maximum RSS is negative")?;
    max_rss_to_bytes(max_rss)
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn max_rss_to_bytes(max_rss: u64) -> Result<u64> {
    Ok(max_rss)
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn max_rss_to_bytes(max_rss: u64) -> Result<u64> {
    max_rss
        .checked_mul(1024)
        .context("deployment baseline maximum RSS byte count overflow")
}

fn sqlite_cache_misses(conn: &Connection, reset: bool) -> Result<u64> {
    let mut current = 0_i32;
    let mut highwater = 0_i32;
    // SAFETY: the handle is used synchronously while `conn` remains alive, and
    // sqlite3_db_status neither stores the pointer nor transfers ownership.
    let status = unsafe {
        rusqlite::ffi::sqlite3_db_status(
            conn.handle(),
            rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
            &mut current,
            &mut highwater,
            i32::from(reset),
        )
    };
    if status != rusqlite::ffi::SQLITE_OK {
        bail!("sqlite3_db_status(CACHE_MISS) failed with code {status}");
    }
    u64::try_from(current).context("deployment baseline SQLite cache misses are negative")
}

fn count_statement(event: TraceEvent<'_>) {
    if matches!(event, TraceEvent::Stmt(_, _)) {
        BASELINE_SQLITE_STATEMENTS.with(|statements| {
            statements.set(statements.get().saturating_add(1));
        });
    }
}

struct SqliteStatementCounter<'connection> {
    connection: &'connection Connection,
    active: bool,
}

impl<'connection> SqliteStatementCounter<'connection> {
    fn start(connection: &'connection Connection) -> Self {
        BASELINE_SQLITE_STATEMENTS.with(|statements| statements.set(0));
        connection.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(count_statement));
        Self {
            connection,
            active: true,
        }
    }

    fn finish(mut self) -> u64 {
        self.connection.trace_v2(TraceEventCodes::empty(), None);
        self.active = false;
        BASELINE_SQLITE_STATEMENTS.with(Cell::take)
    }
}

impl Drop for SqliteStatementCounter<'_> {
    fn drop(&mut self) {
        if self.active {
            self.connection.trace_v2(TraceEventCodes::empty(), None);
            BASELINE_SQLITE_STATEMENTS.with(|statements| statements.set(0));
        }
    }
}
